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

/// Queries the global blocks view for a block's metadata and constructs a `StreamMetadata`.
/// Returns `None` if the block is not found.
async fn fetch_block_metadata(
    lakehouse: Arc<LakehouseContext>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
    block_id_str: &str,
) -> anyhow::Result<Option<(Uuid, i64, StreamMetadata)>> {
    let ctx = super::query::make_session_context(
        lakehouse,
        part_provider,
        query_range,
        view_factory,
        Arc::new(NoOpSessionConfigurator),
    )
    .await?;

    let sql = format!(
        "SELECT block_id, stream_id, process_id, object_offset,
                \"streams.dependencies_metadata\", \"streams.objects_metadata\"
         FROM blocks
         WHERE block_id = '{block_id_str}'"
    );
    let df = ctx.sql(&sql).await?;
    let batches = df.collect().await?;

    if batches.is_empty() || batches[0].num_rows() == 0 {
        return Ok(None);
    }

    let batch = &batches[0];

    let block_id_col = string_column_by_name(batch, "block_id")?;
    let stream_id_col = string_column_by_name(batch, "stream_id")?;
    let process_id_col = string_column_by_name(batch, "process_id")?;
    let object_offset_col: &Int64Array = typed_column_by_name(batch, "object_offset")?;

    let block_id = Uuid::parse_str(block_id_col.value(0)?)?;
    let stream_id = Uuid::parse_str(stream_id_col.value(0)?)?;
    let process_id = Uuid::parse_str(process_id_col.value(0)?)?;
    let object_offset = object_offset_col.value(0);

    let deps_col = batch
        .column_by_name("streams.dependencies_metadata")
        .context("streams.dependencies_metadata column not found")?;
    let deps_binary: &datafusion::arrow::array::BinaryArray = deps_col
        .as_any()
        .downcast_ref()
        .context("failed to cast dependencies_metadata to BinaryArray")?;
    let deps_bytes = deps_binary.value(0);
    let dependencies_metadata: Vec<UserDefinedType> =
        ciborium::from_reader(deps_bytes).context("decoding dependencies_metadata")?;

    let objs_col = batch
        .column_by_name("streams.objects_metadata")
        .context("streams.objects_metadata column not found")?;
    let objs_binary: &datafusion::arrow::array::BinaryArray = objs_col
        .as_any()
        .downcast_ref()
        .context("failed to cast objects_metadata to BinaryArray")?;
    let objs_bytes = objs_binary.value(0);
    let objects_metadata: Vec<UserDefinedType> =
        ciborium::from_reader(objs_bytes).context("decoding objects_metadata")?;

    let stream_metadata = StreamMetadata {
        process_id,
        stream_id,
        dependencies_metadata,
        objects_metadata,
        tags: vec![],
        properties: Arc::new(vec![]),
    };

    Ok(Some((block_id, object_offset, stream_metadata)))
}

/// Parses transit objects from a block payload and returns them as a RecordBatch.
fn parse_block_objects(
    stream_metadata: &StreamMetadata,
    payload: &micromegas_telemetry::block_wire_format::BlockPayload,
    object_offset: i64,
    early_limit: Option<usize>,
) -> anyhow::Result<RecordBatch> {
    let mut index_builder = Int64Builder::new();
    let mut name_builder = StringBuilder::new();
    let mut value_builder = BinaryBuilder::new();
    let mut local_index: i64 = 0;
    let mut nb_objects: usize = 0;

    parse_block(stream_metadata, payload, |value| {
        if let TransitValue::Object(obj) = &value {
            let jsonb_val = transit_value_to_jsonb(&value);
            let mut buf = Vec::new();
            jsonb_val.write_to_vec(&mut buf);

            index_builder.append_value(object_offset + local_index);
            name_builder.append_value(obj.type_name.as_ref());
            value_builder.append_value(&buf);
            nb_objects += 1;
        } else {
            warn!(
                "parse_block: skipping non-Object value at index {}",
                object_offset + local_index
            );
        }
        local_index += 1;

        if let Some(lim) = early_limit {
            Ok(nb_objects < lim)
        } else {
            Ok(true)
        }
    })?;

    Ok(RecordBatch::try_new(
        output_schema(),
        vec![
            Arc::new(index_builder.finish()),
            Arc::new(name_builder.finish()),
            Arc::new(value_builder.finish()),
        ],
    )?)
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

        let Some((block_id, object_offset, stream_metadata)) = fetch_block_metadata(
            self.lakehouse.clone(),
            self.part_provider.clone(),
            self.query_range,
            self.view_factory.clone(),
            block_id_str,
        )
        .await
        .map_err(|e| DataFusionError::External(e.into()))?
        else {
            let source = MemorySourceConfig::try_new(
                &[vec![]],
                self.schema(),
                projection.map(|v| v.to_owned()),
            )?;
            return Ok(DataSourceExec::from_data_source(source));
        };

        // Fetch and parse the block payload
        let blob_storage = self.lakehouse.lake().blob_storage.clone();
        let payload = fetch_block_payload(
            blob_storage,
            sqlx::types::Uuid::from_bytes(*stream_metadata.process_id.as_bytes()),
            sqlx::types::Uuid::from_bytes(*stream_metadata.stream_id.as_bytes()),
            sqlx::types::Uuid::from_bytes(*block_id.as_bytes()),
        )
        .await
        .map_err(|e| DataFusionError::External(e.into()))?;

        // Parse transit objects and convert to JSONB
        let early_limit = if filters.is_empty() { limit } else { None };
        let rb = parse_block_objects(&stream_metadata, &payload, object_offset, early_limit)
            .with_context(|| format!("parsing block {block_id_str}"))
            .map_err(|e| DataFusionError::External(e.into()))?;

        let source = MemorySourceConfig::try_new(
            &[vec![rb]],
            self.schema(),
            projection.map(|v| v.to_owned()),
        )?;
        Ok(DataSourceExec::from_data_source(source))
    }
}
