use super::{
    audience_guard::{AudienceGuard, IdKind},
    block_object_decoder::{BlockObjectDecoderMap, ObjectVisitor, default_block_object_decoders},
    lakehouse_context::LakehouseContext,
    partition_cache::QueryPartitionProvider,
    read_scope::CallerContext,
    session_configurator::NoOpSessionConfigurator,
    view_factory::ViewFactory,
};
use crate::{
    dfext::{string_column_accessor::string_column_by_name, typed_column::typed_column_by_name},
    metadata::StreamMetadata,
    payload::fetch_block_payload,
    time::TimeRange,
};
use anyhow::Context;
use async_trait::async_trait;
use datafusion::{
    arrow::{
        array::{BinaryBuilder, Int64Array, Int64Builder, RecordBatch, StringBuilder},
        datatypes::{DataType, Field, Schema, SchemaRef},
    },
    catalog::{Session, TableFunctionArgs, TableFunctionImpl, TableProvider},
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
use micromegas_transit::{UserDefinedType, value::Value as TransitValue};
use std::{borrow::Cow, collections::BTreeMap, sync::Arc};
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
pub fn transit_value_to_jsonb(value: TransitValue<'_>) -> JsonbValue<'_> {
    match value {
        TransitValue::String(s) => JsonbValue::String(Cow::Borrowed(s)),
        TransitValue::Object(obj) => {
            let mut map = BTreeMap::new();
            map.insert(
                "__type".to_string(),
                JsonbValue::String(Cow::Borrowed(obj.type_name)),
            );
            for &(name, val) in obj.members {
                map.insert(name.to_string(), transit_value_to_jsonb(val));
            }
            JsonbValue::Object(map)
        }
        TransitValue::U8(v) => JsonbValue::Number(jsonb::Number::UInt64(u64::from(v))),
        TransitValue::U32(v) => JsonbValue::Number(jsonb::Number::UInt64(u64::from(v))),
        TransitValue::U64(v) => JsonbValue::Number(jsonb::Number::UInt64(v)),
        TransitValue::I64(v) => JsonbValue::Number(jsonb::Number::Int64(v)),
        TransitValue::F64(v) => JsonbValue::Number(jsonb::Number::Float64(v)),
        TransitValue::None => JsonbValue::Null,
        TransitValue::Bytes(b) => {
            JsonbValue::String(Cow::Owned(format!("<binary {} bytes>", b.len())))
        }
    }
}

/// Queries the global blocks view for a block's metadata and constructs a `StreamMetadata`.
/// Returns `None` if the block is not found in `blocks` for the queried range.
async fn fetch_block_metadata(
    lakehouse: Arc<LakehouseContext>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
    block_id: Uuid,
    caller: CallerContext,
) -> anyhow::Result<Option<(i64, String, StreamMetadata)>> {
    // Runs under the witness's internal caller (`caller`, threaded in from `scan` after
    // `AudienceGuard::authorize` succeeds), not the caller's own scope: the query below is
    // server-constructed and confined to the single, already-authorized `block_id` -- if that
    // block's process is readable, everything this statement can reach is readable too. A
    // deliberate deviation from naive scope inheritance; see
    // `tasks/1371_udtf_udf_guards_plan.md` §6 for the full argument.
    let ctx = super::query::make_session_context(
        lakehouse,
        part_provider,
        query_range,
        view_factory,
        Arc::new(NoOpSessionConfigurator),
        caller,
    )
    .await?;

    // Interpolate the canonical hyphenated rendering (not the caller's original
    // string) so a valid-but-non-canonical form (braced, URN, bare-hex) still
    // matches the `blocks` view's canonical rendering.
    let sql = format!(
        "SELECT stream_id, process_id, object_offset,
                \"streams.dependencies_metadata\", \"streams.objects_metadata\", \"streams.format\"
         FROM blocks
         WHERE block_id = '{block_id}'"
    );
    let df = ctx.sql(&sql).await?;
    let batches = df.collect().await?;

    if batches.is_empty() || batches[0].num_rows() == 0 {
        return Ok(None);
    }

    let batch = &batches[0];

    let stream_id_col = string_column_by_name(batch, "stream_id")?;
    let process_id_col = string_column_by_name(batch, "process_id")?;
    let object_offset_col: &Int64Array = typed_column_by_name(batch, "object_offset")?;
    let format_col = string_column_by_name(batch, "streams.format")?;
    let format = format_col.value(0)?.to_string();

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

    Ok(Some((object_offset, format, stream_metadata)))
}

/// `ObjectVisitor` that owns the Arrow builders, the running `object_index`,
/// and the early-limit check for `parse_block` — row construction stays in one
/// place regardless of which `BlockObjectDecoder` drives it.
struct ParseBlockRowBuilder {
    index_builder: Int64Builder,
    name_builder: StringBuilder,
    value_builder: BinaryBuilder,
    object_offset: i64,
    local_index: i64,
    nb_objects: usize,
    early_limit: Option<usize>,
}

impl ParseBlockRowBuilder {
    fn new(object_offset: i64, early_limit: Option<usize>) -> Self {
        Self {
            index_builder: Int64Builder::new(),
            name_builder: StringBuilder::new(),
            value_builder: BinaryBuilder::new(),
            object_offset,
            local_index: 0,
            nb_objects: 0,
            early_limit,
        }
    }

    /// Whether iteration should continue, based on rows emitted so far
    /// (`nb_objects`) — mirrors the pre-registry `parse_block_objects` check,
    /// which applied to both emitted and skipped entries alike.
    fn continue_iterating(&self) -> bool {
        match self.early_limit {
            Some(lim) => self.nb_objects < lim,
            None => true,
        }
    }

    fn finish(mut self) -> anyhow::Result<RecordBatch> {
        Ok(RecordBatch::try_new(
            output_schema(),
            vec![
                Arc::new(self.index_builder.finish()),
                Arc::new(self.name_builder.finish()),
                Arc::new(self.value_builder.finish()),
            ],
        )?)
    }
}

impl ObjectVisitor for ParseBlockRowBuilder {
    fn visit(&mut self, type_name: &str, value: &[u8]) -> anyhow::Result<bool> {
        self.index_builder
            .append_value(self.object_offset + self.local_index);
        self.name_builder.append_value(type_name);
        self.value_builder.append_value(value);
        self.nb_objects += 1;
        self.local_index += 1;
        Ok(self.continue_iterating())
    }

    fn skip(&mut self) -> anyhow::Result<bool> {
        self.local_index += 1;
        Ok(self.continue_iterating())
    }
}

/// A DataFusion `TableFunctionImpl` that parses a block's payload — through the
/// `BlockObjectDecoder` registered for its `streams.format` — and returns each
/// decoded object as a row with its type name and full content as JSONB.
#[derive(Debug)]
pub struct ParseBlockTableFunction {
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    decoders: Arc<BlockObjectDecoderMap>,
    guard: Arc<AudienceGuard>,
}

impl ParseBlockTableFunction {
    pub fn new(
        lakehouse: Arc<LakehouseContext>,
        view_factory: Arc<ViewFactory>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        query_range: Option<TimeRange>,
        guard: Arc<AudienceGuard>,
    ) -> Self {
        Self {
            lakehouse,
            view_factory,
            part_provider,
            query_range,
            decoders: default_block_object_decoders(),
            guard,
        }
    }
}

impl TableFunctionImpl for ParseBlockTableFunction {
    fn call_with_args(
        &self,
        args: TableFunctionArgs,
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        let exprs = args.exprs();
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
            decoders: self.decoders.clone(),
            guard: self.guard.clone(),
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
    decoders: Arc<BlockObjectDecoderMap>,
    guard: Arc<AudienceGuard>,
}

#[async_trait]
impl TableProvider for ParseBlockProvider {
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

        let Ok(block_id) = Uuid::parse_str(block_id_str) else {
            return plan_err!("parse_block: '{block_id_str}' is not a valid block id");
        };

        let authorized = self
            .guard
            .authorize(block_id, IdKind::Block, "parse_block")
            .await?;

        let Some((object_offset, format, stream_metadata)) = fetch_block_metadata(
            self.lakehouse.clone(),
            self.part_provider.clone(),
            self.query_range,
            self.view_factory.clone(),
            block_id,
            authorized.internal_caller(),
        )
        .await
        .map_err(|e| DataFusionError::External(e.into()))?
        else {
            let message = match self.query_range {
                Some(range) => format!(
                    "parse_block: block '{block_id_str}' not found in `blocks` for the queried \
                     range [{}, {}]. The block may be outside the query's time range — widen \
                     the range to include it.",
                    range.begin.to_rfc3339(),
                    range.end.to_rfc3339()
                ),
                None => format!("parse_block: block '{block_id_str}' not found in `blocks`."),
            };
            return plan_err!("{message}");
        };

        let Some(decoder) = self.decoders.get(format.as_str()) else {
            let mut known: Vec<&str> = self.decoders.keys().copied().collect();
            known.sort_unstable();
            return plan_err!(
                "parse_block: no decoder for streams.format='{format}' (known formats: {})",
                known.join(", ")
            );
        };

        // Fetch and decode the block payload
        let blob_storage = self.lakehouse.lake().blob_storage.clone();
        let payload = fetch_block_payload(
            blob_storage,
            sqlx::types::Uuid::from_bytes(*stream_metadata.process_id.as_bytes()),
            sqlx::types::Uuid::from_bytes(*stream_metadata.stream_id.as_bytes()),
            sqlx::types::Uuid::from_bytes(*block_id.as_bytes()),
        )
        .await
        .map_err(|e| DataFusionError::External(e.into()))?;

        let early_limit = if filters.is_empty() { limit } else { None };
        let mut builder = ParseBlockRowBuilder::new(object_offset, early_limit);
        decoder
            .decode(&stream_metadata, &payload, &mut builder)
            .with_context(|| format!("parsing block {block_id_str}"))
            .map_err(|e| DataFusionError::External(e.into()))?;
        let rb = builder
            .finish()
            .with_context(|| format!("building record batch for block {block_id_str}"))
            .map_err(|e| DataFusionError::External(e.into()))?;

        let source = MemorySourceConfig::try_new(
            &[vec![rb]],
            self.schema(),
            projection.map(|v| v.to_owned()),
        )?;
        Ok(DataSourceExec::from_data_source(source))
    }
}
