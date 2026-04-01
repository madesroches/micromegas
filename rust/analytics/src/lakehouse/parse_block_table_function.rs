use super::{
    lakehouse_context::LakehouseContext, partition_cache::QueryPartitionProvider,
    session_configurator::NoOpSessionConfigurator, view_factory::ViewFactory,
};
use crate::{
    dfext::{string_column_accessor::string_column_by_name, typed_column::typed_column_by_name},
    metadata::StreamMetadata,
    payload::{fetch_block_payload, parse_block},
    time::TimeRange,
};
use anyhow::Context;
use async_trait::async_trait;
use datafusion::{
    arrow::{
        array::{BinaryBuilder, Int64Array, Int64Builder, RecordBatch, StringBuilder},
        datatypes::{DataType, Field, Schema, SchemaRef},
    },
    catalog::{Session, TableFunctionImpl, TableProvider},
    common::plan_err,
    datasource::{
        TableType,
        memory::{DataSourceExec, MemorySourceConfig},
    },
    error::DataFusionError,
    physical_plan::ExecutionPlan,
    prelude::Expr,
};
use jsonb::Value as JsonbValue;
use micromegas_tracing::prelude::*;
use micromegas_transit::{UserDefinedType, value::Value as TransitValue};
use std::{any::Any, borrow::Cow, collections::BTreeMap, sync::Arc};
use uuid::Uuid;

use crate::dfext::expressions::exp_to_string;

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("object_index", DataType::Int64, false),
        Field::new("type_name", DataType::Utf8, false),
        Field::new("value", DataType::Binary, false),
    ]))
}

/// Converts a `transit::Value` to a `jsonb::Value`.
pub fn transit_value_to_jsonb(value: &TransitValue) -> JsonbValue<'_> {
    match value {
        TransitValue::String(s) => JsonbValue::String(Cow::Borrowed(s.as_str())),
        TransitValue::Object(obj) => {
            let mut map = BTreeMap::new();
            map.insert(
                "__type".to_string(),
                JsonbValue::String(Cow::Borrowed(obj.type_name.as_str())),
            );
            for (name, val) in &obj.members {
                map.insert(name.as_ref().clone(), transit_value_to_jsonb(val));
            }
            JsonbValue::Object(map)
        }
        TransitValue::U8(v) => JsonbValue::Number(jsonb::Number::UInt64(u64::from(*v))),
        TransitValue::U32(v) => JsonbValue::Number(jsonb::Number::UInt64(u64::from(*v))),
        TransitValue::U64(v) => JsonbValue::Number(jsonb::Number::UInt64(*v)),
        TransitValue::I64(v) => JsonbValue::Number(jsonb::Number::Int64(*v)),
        TransitValue::F64(v) => JsonbValue::Number(jsonb::Number::Float64(*v)),
        TransitValue::None => JsonbValue::Null,
    }
}

/// A DataFusion `TableFunctionImpl` that parses a block's transit-serialized objects
/// and returns each object as a row with its type name and full content as JSONB.
#[derive(Debug)]
pub struct ParseBlockTableFunction {
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
}

impl ParseBlockTableFunction {
    pub fn new(
        lakehouse: Arc<LakehouseContext>,
        view_factory: Arc<ViewFactory>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        query_range: Option<TimeRange>,
    ) -> Self {
        Self {
            lakehouse,
            view_factory,
            part_provider,
            query_range,
        }
    }
}

impl TableFunctionImpl for ParseBlockTableFunction {
    fn call(&self, exprs: &[Expr]) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        let arg = exprs.first().map(exp_to_string);
        let Some(Ok(block_id)) = arg else {
            return plan_err!(
                "First argument to parse_block must be a string (the block ID), given {:?}",
                arg
            );
        };
        Ok(Arc::new(ParseBlockProvider {
            block_id,
            lakehouse: self.lakehouse.clone(),
            view_factory: self.view_factory.clone(),
            part_provider: self.part_provider.clone(),
            query_range: self.query_range,
        }))
    }
}

#[derive(Debug)]
struct ParseBlockProvider {
    block_id: String,
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
}

#[async_trait]
impl TableProvider for ParseBlockProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        output_schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        let block_id_str = &self.block_id;

        // 1. Query the global blocks view for metadata
        let ctx = super::query::make_session_context(
            self.lakehouse.clone(),
            self.part_provider.clone(),
            self.query_range,
            self.view_factory.clone(),
            Arc::new(NoOpSessionConfigurator),
        )
        .await
        .map_err(|e| DataFusionError::External(e.into()))?;

        let sql = format!(
            "SELECT block_id, stream_id, process_id, object_offset,
                    \"streams.dependencies_metadata\", \"streams.objects_metadata\"
             FROM blocks
             WHERE block_id = '{block_id_str}'"
        );
        let df = ctx
            .sql(&sql)
            .await
            .map_err(|e| DataFusionError::External(e.into()))?;
        let batches = df
            .collect()
            .await
            .map_err(|e| DataFusionError::External(e.into()))?;

        if batches.is_empty() || batches[0].num_rows() == 0 {
            // Block not found — return empty result
            let source = MemorySourceConfig::try_new(
                &[vec![]],
                self.schema(),
                projection.map(|v| v.to_owned()),
            )?;
            return Ok(DataSourceExec::from_data_source(source));
        }

        let batch = &batches[0];

        // 2. Extract metadata from the result row
        let block_id_col = string_column_by_name(batch, "block_id")
            .map_err(|e| DataFusionError::External(e.into()))?;
        let stream_id_col = string_column_by_name(batch, "stream_id")
            .map_err(|e| DataFusionError::External(e.into()))?;
        let process_id_col = string_column_by_name(batch, "process_id")
            .map_err(|e| DataFusionError::External(e.into()))?;
        let object_offset_col: &Int64Array = typed_column_by_name(batch, "object_offset")
            .map_err(|e| DataFusionError::External(e.into()))?;

        let block_id = Uuid::parse_str(
            block_id_col
                .value(0)
                .map_err(|e| DataFusionError::External(e.into()))?,
        )
        .map_err(|e| DataFusionError::External(e.into()))?;
        let stream_id = Uuid::parse_str(
            stream_id_col
                .value(0)
                .map_err(|e| DataFusionError::External(e.into()))?,
        )
        .map_err(|e| DataFusionError::External(e.into()))?;
        let process_id = Uuid::parse_str(
            process_id_col
                .value(0)
                .map_err(|e| DataFusionError::External(e.into()))?,
        )
        .map_err(|e| DataFusionError::External(e.into()))?;
        let object_offset = object_offset_col.value(0);

        // CBOR-decode dependencies_metadata and objects_metadata
        let deps_col = batch
            .column_by_name("streams.dependencies_metadata")
            .ok_or_else(|| {
                DataFusionError::Execution("streams.dependencies_metadata column not found".into())
            })?;
        let deps_binary: &datafusion::arrow::array::BinaryArray =
            deps_col.as_any().downcast_ref().ok_or_else(|| {
                DataFusionError::Execution(
                    "failed to cast dependencies_metadata to BinaryArray".into(),
                )
            })?;
        let deps_bytes = deps_binary.value(0);
        let dependencies_metadata: Vec<UserDefinedType> = ciborium::from_reader(deps_bytes)
            .map_err(|e| {
                DataFusionError::External(
                    anyhow::anyhow!("decoding dependencies_metadata: {e}").into(),
                )
            })?;

        let objs_col = batch
            .column_by_name("streams.objects_metadata")
            .ok_or_else(|| {
                DataFusionError::Execution("streams.objects_metadata column not found".into())
            })?;
        let objs_binary: &datafusion::arrow::array::BinaryArray =
            objs_col.as_any().downcast_ref().ok_or_else(|| {
                DataFusionError::Execution("failed to cast objects_metadata to BinaryArray".into())
            })?;
        let objs_bytes = objs_binary.value(0);
        let objects_metadata: Vec<UserDefinedType> =
            ciborium::from_reader(objs_bytes).map_err(|e| {
                DataFusionError::External(anyhow::anyhow!("decoding objects_metadata: {e}").into())
            })?;

        let stream_metadata = StreamMetadata {
            process_id,
            stream_id,
            dependencies_metadata,
            objects_metadata,
            tags: vec![],
            properties: Arc::new(vec![]),
        };

        // 3. Fetch and parse the block payload
        let blob_storage = self.lakehouse.lake().blob_storage.clone();
        let payload = fetch_block_payload(
            blob_storage,
            sqlx::types::Uuid::from_bytes(*process_id.as_bytes()),
            sqlx::types::Uuid::from_bytes(*stream_id.as_bytes()),
            sqlx::types::Uuid::from_bytes(*block_id.as_bytes()),
        )
        .await
        .map_err(|e| DataFusionError::External(e.into()))?;

        // 4. Parse transit objects and convert to JSONB
        let mut indices: Vec<i64> = Vec::new();
        let mut type_names: Vec<String> = Vec::new();
        let mut jsonb_values: Vec<Vec<u8>> = Vec::new();
        let mut local_index: i64 = 0;

        // Only apply early limit when there are no filters
        let early_limit = if filters.is_empty() { limit } else { None };

        parse_block(&stream_metadata, &payload, |value| {
            if let TransitValue::Object(obj) = &value {
                let jsonb_val = transit_value_to_jsonb(&value);
                let mut buf = Vec::new();
                jsonb_val.write_to_vec(&mut buf);

                indices.push(object_offset + local_index);
                type_names.push(obj.type_name.as_ref().clone());
                jsonb_values.push(buf);
            } else {
                warn!(
                    "parse_block: skipping non-Object value at index {}",
                    object_offset + local_index
                );
            }
            local_index += 1;

            if let Some(lim) = early_limit {
                Ok(indices.len() < lim)
            } else {
                Ok(true)
            }
        })
        .with_context(|| format!("parsing block {block_id_str}"))
        .map_err(|e| DataFusionError::External(e.into()))?;

        // 5. Build RecordBatch
        let mut index_builder = Int64Builder::with_capacity(indices.len());
        let mut name_builder = StringBuilder::with_capacity(type_names.len(), 0);
        let mut value_builder = BinaryBuilder::with_capacity(jsonb_values.len(), 0);

        for (i, idx) in indices.iter().enumerate() {
            index_builder.append_value(*idx);
            name_builder.append_value(&type_names[i]);
            value_builder.append_value(&jsonb_values[i]);
        }

        let rb = RecordBatch::try_new(
            self.schema(),
            vec![
                Arc::new(index_builder.finish()),
                Arc::new(name_builder.finish()),
                Arc::new(value_builder.finish()),
            ],
        )
        .map_err(|e| DataFusionError::External(e.into()))?;

        let source = MemorySourceConfig::try_new(
            &[vec![rb]],
            self.schema(),
            projection.map(|v| v.to_owned()),
        )?;
        Ok(DataSourceExec::from_data_source(source))
    }
}
