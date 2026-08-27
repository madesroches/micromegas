use super::{
    batch_update::PartitionCreationStrategy,
    dataframe_time_bounds::{DataFrameTimeBounds, NamedColumnsTimeBounds},
    lakehouse_context::LakehouseContext,
    merge::{MergeQueryResult, PartitionMerger, QueryMerger},
    metadata_partition_spec::fetch_metadata_partition_spec,
    partition::Partition,
    partition_cache::PartitionCache,
    partitioned_execution_plan::{OrderingBounds, ScanOrdering},
    session_configurator::NoOpSessionConfigurator,
    view::{PartitionSpec, ScanSortColumn, View, ViewMetadata},
    view_factory::ViewFactory,
};
use crate::audience::{audience_column_mismatch, coalesced_audience_column};
use crate::time::{TimeRange, datetime_to_scalar};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::{DataType, Field, Schema, TimeUnit},
    logical_expr::{Expr, col},
    prelude::*,
};
use std::sync::Arc;

const VIEW_SET_NAME: &str = "blocks";
const VIEW_INSTANCE_ID: &str = "global";
lazy_static::lazy_static! {
    static ref BEGIN_TIME_COLUMN: Arc<String> = Arc::new( String::from("begin_time"));
    static ref INSERT_TIME_COLUMN: Arc<String> = Arc::new( String::from("insert_time"));
}

/// The single sort guarantee this view ever records or declares -- see the plan's Trade-offs
/// section for why `block_id` is deliberately not part of it.
fn insert_time_sort_order() -> Vec<String> {
    vec![String::from("insert_time")]
}

/// The NULL-tolerant audience-mismatch *keep* predicate (Design §4): true for a block whose own
/// `audience` stamp does not disagree with either row it joins to (a NULL on either side always
/// passes through -- see `audience_column_mismatch`'s doc comment for why). Built from
/// `audience_column_mismatch` so this predicate, `source_count_query`, and the audience-mismatch
/// diagnostic query (`MetadataPartitionSpec::write`, §4) -- and the hourly
/// `block_audience_mismatch_rows` counter (`maintenance.rs`, §5) -- can never drift
/// independently.
fn audience_mismatch_keep_predicate() -> String {
    format!(
        "NOT ({} OR {})",
        audience_column_mismatch("blocks", "streams"),
        audience_column_mismatch("blocks", "processes"),
    )
}

/// True when every partition in `partitions_to_merge` either contributes no rows or already
/// carries the exact `sort_order` this view can trust (Design §1/§4): only then can a merge
/// safely declare and record the `insert_time` guarantee.
fn all_inputs_ordered_or_empty(partitions_to_merge: &[Partition]) -> bool {
    let wanted = insert_time_sort_order();
    partitions_to_merge
        .iter()
        .all(|p| p.is_empty() || p.sort_order.as_ref() == Some(&wanted))
}

/// A view of the `blocks` table, providing access to telemetry block metadata.
#[derive(Debug)]
pub struct BlocksView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    data_sql: Arc<String>,
    /// Bound as `data_sql`'s `$3` -- the audience a never-stamped block materializes under
    /// (AbAC Stage 5b, #1518, §4: a NULL `blocks.audience`, i.e. a legacy pre-v8 row). Carried
    /// from `LakehouseContext::default_audience` by every constructor call.
    default_audience: Arc<str>,
    ordered_merger: Arc<dyn PartitionMerger>,
    plain_merger: Arc<dyn PartitionMerger>,
}

impl BlocksView {
    /// `default_audience` is the deployment's `MICROMEGAS_DEFAULT_AUDIENCE`, sourced from
    /// `LakehouseContext::default_audience`. It is the first of #1482's three read sites and the
    /// only one whose resolved value is *baked into a partition*: every row of one materialization
    /// sees the same bound value, so a never-stamped process is labelled consistently within a
    /// partition. Two partitions materialized under different configured defaults can disagree --
    /// changing the default requires regenerating the six views over the affected range.
    pub fn new(default_audience: Arc<str>) -> Result<Self> {
        let keep_predicate = audience_mismatch_keep_predicate();
        let data_sql = Arc::new(format!(
            r#"SELECT block_id, streams.stream_id, processes.process_id, blocks.begin_time, blocks.begin_ticks, blocks.end_time, blocks.end_ticks, blocks.nb_objects, blocks.object_offset, blocks.payload_size, blocks.insert_time,
           streams.dependencies_metadata, streams.objects_metadata, streams.tags, streams.properties, streams.insert_time as stream_insert_time, streams.format,
           processes.start_time, processes.start_ticks, processes.tsc_frequency, processes.exe, processes.username, processes.realname, processes.computer, processes.distro, processes.cpu_brand, processes.insert_time as process_insert_time, processes.parent_process_id, processes.properties as process_properties,
           {audience_column} AS audience
         FROM blocks, streams, processes
         WHERE blocks.stream_id = streams.stream_id
         AND blocks.process_id = processes.process_id
         AND blocks.insert_time >= $1
         AND blocks.insert_time < $2
         AND {keep_predicate}
         ORDER BY blocks.insert_time, blocks.block_id
         ;"#,
            audience_column = coalesced_audience_column("blocks", 3),
        ));
        let empty_view_factory = Arc::new(ViewFactory::new(vec![]));
        let schema = Arc::new(blocks_view_schema());
        let ordered_merger: Arc<dyn PartitionMerger> = Arc::new(
            QueryMerger::new(
                empty_view_factory.clone(),
                Arc::new(NoOpSessionConfigurator),
                schema.clone(),
                Arc::new(String::from("SELECT * FROM source ORDER BY insert_time;")),
            )
            .with_merge_scan_ordering(ScanOrdering::Concatenated {
                columns: vec![ScanSortColumn {
                    column: Arc::new(String::from("insert_time")),
                    descending: false,
                }],
                bounds: OrderingBounds::InsertTime,
            }),
        );
        let plain_merger: Arc<dyn PartitionMerger> = Arc::new(QueryMerger::new(
            empty_view_factory,
            Arc::new(NoOpSessionConfigurator),
            schema,
            Arc::new(String::from("SELECT * FROM source;")),
        ));
        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(String::from(VIEW_INSTANCE_ID)),
            data_sql,
            default_audience,
            ordered_merger,
            plain_merger,
        })
    }
}

#[async_trait]
impl View for BlocksView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        lakehouse: Arc<LakehouseContext>,
        _existing_partitions: Arc<PartitionCache>,
        insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>> {
        let view_meta = ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
        };
        // `count` is the kept side of the audience-mismatch comparison (rows surviving
        // `keep_predicate`, i.e. this partition's `record_count`); `unfiltered` is the same join
        // and insert-time range with no predicate applied at all. One query gives
        // `fetch_metadata_partition_spec` both, so `MetadataPartitionSpec::write` never needs a
        // second round trip to recover the unfiltered count (§4) -- see `unfiltered_count`'s doc
        // comment on that struct.
        let source_count_query = format!(
            "SELECT COUNT(*) FILTER (WHERE {keep_predicate}) AS count,
                    COUNT(*) AS unfiltered
             FROM blocks, streams, processes
             WHERE blocks.stream_id = streams.stream_id
             AND blocks.process_id = processes.process_id
             AND blocks.insert_time >= $1
             AND blocks.insert_time < $2
             ;",
            keep_predicate = audience_mismatch_keep_predicate(),
        );
        let spec = fetch_metadata_partition_spec(
            &lakehouse.lake().db_pool,
            &source_count_query,
            self.data_sql.clone(),
            view_meta,
            self.get_file_schema(),
            insert_range,
            self.get_time_bounds(),
            Some(insert_time_sort_order()),
            Some(self.default_audience.clone()),
        )
        .await
        .with_context(|| "fetch_metadata_partition_spec")?;
        Ok(Arc::new(spec))
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        blocks_file_schema_hash()
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(blocks_view_schema())
    }

    async fn jit_update(
        &self,
        _lakehouse: Arc<LakehouseContext>,
        _query_range: Option<TimeRange>,
    ) -> Result<()> {
        if *self.view_instance_id == "global" {
            // this view instance is updated using the deamon
            return Ok(());
        }
        anyhow::bail!("not supported");
    }

    fn make_time_filter(&self, begin: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Expr>> {
        Ok(vec![
            col("begin_time").lt_eq(lit(datetime_to_scalar(end))),
            col("insert_time").gt_eq(lit(datetime_to_scalar(begin))),
        ])
    }

    fn get_time_bounds(&self) -> Arc<dyn DataFrameTimeBounds> {
        //todo: make more robust, by changing to [ min(begin, insert), max(end, insert) ]
        Arc::new(NamedColumnsTimeBounds::new(
            BEGIN_TIME_COLUMN.clone(),
            INSERT_TIME_COLUMN.clone(),
        ))
    }

    fn get_update_group(&self) -> Option<i32> {
        Some(1000)
    }

    fn get_max_partition_time_delta(&self, strategy: &PartitionCreationStrategy) -> TimeDelta {
        match strategy {
            PartitionCreationStrategy::Abort | PartitionCreationStrategy::CreateFromSource => {
                TimeDelta::hours(1)
            }
            PartitionCreationStrategy::MergeExisting(_partitions) => TimeDelta::days(1),
        }
    }

    async fn merge_partitions(
        &self,
        lakehouse: Arc<LakehouseContext>,
        partitions_to_merge: Arc<Vec<Partition>>,
        partitions_all_views: Arc<PartitionCache>,
        insert_range: TimeRange,
    ) -> Result<MergeQueryResult> {
        // An all-empty source scans as an EmptyExec, whose SortExec is never elided -- taking the
        // ordered path there would trip the plan-shape check's memory-regression warning on every
        // quiet-day retry. So the ordered path additionally requires at least one non-empty input.
        let any_non_empty = partitions_to_merge.iter().any(|p| !p.is_empty());
        let merger = if any_non_empty && all_inputs_ordered_or_empty(&partitions_to_merge) {
            &self.ordered_merger
        } else {
            &self.plain_merger
        };
        merger
            .execute_merge_query(
                lakehouse,
                partitions_to_merge,
                partitions_all_views,
                insert_range,
            )
            .await
    }

    fn get_merged_partition_sort_order(
        &self,
        partitions_to_merge: &[Partition],
    ) -> Option<Vec<String>> {
        if all_inputs_ordered_or_empty(partitions_to_merge) {
            Some(insert_time_sort_order())
        } else {
            None
        }
    }
}

/// Returns the Arrow schema for the blocks view.
pub fn blocks_view_schema() -> Schema {
    Schema::new(vec![
        Field::new("block_id", DataType::Utf8, false),
        Field::new("stream_id", DataType::Utf8, false),
        Field::new("process_id", DataType::Utf8, false),
        Field::new(
            "begin_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("begin_ticks", DataType::Int64, false),
        Field::new(
            "end_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("end_ticks", DataType::Int64, false),
        Field::new("nb_objects", DataType::Int32, false),
        Field::new("object_offset", DataType::Int64, false),
        Field::new("payload_size", DataType::Int64, false),
        Field::new(
            "insert_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("streams.dependencies_metadata", DataType::Binary, false),
        Field::new("streams.objects_metadata", DataType::Binary, false),
        Field::new(
            "streams.tags",
            DataType::List(Arc::new(Field::new("tag", DataType::Utf8, false))),
            true,
        ),
        Field::new(
            "streams.properties",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Binary)),
            false,
        ),
        Field::new(
            "streams.insert_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("streams.format", DataType::Utf8, false),
        Field::new(
            "processes.start_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("processes.start_ticks", DataType::Int64, false),
        Field::new("processes.tsc_frequency", DataType::Int64, false),
        Field::new("processes.exe", DataType::Utf8, false),
        Field::new("processes.username", DataType::Utf8, false),
        Field::new("processes.realname", DataType::Utf8, false),
        Field::new("processes.computer", DataType::Utf8, false),
        Field::new("processes.distro", DataType::Utf8, false),
        Field::new("processes.cpu_brand", DataType::Utf8, false),
        Field::new(
            "processes.insert_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        // Nullable, not because the Postgres column is (no `processes`/`streams`/`blocks` column
        // is `NOT NULL`) but because it is `NULL` in practice for every OTLP process and every
        // root native process (`parent_process_id: Option<Uuid>`). The write path's nullability
        // guard (`write_partition.rs`) checks declared-non-nullable columns against the batch, so
        // a wrongly-`false` declaration here would reject essentially every fresh partition.
        Field::new("processes.parent_process_id", DataType::Utf8, true),
        Field::new(
            "processes.properties",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Binary)),
            false,
        ),
        // Appended last (#1482; per-row since AbAC Stage 5b, #1518): the block's own stamp --
        // the credential that wrote *this block*, never derived from the `process_id`/
        // `stream_id` it joins to. Non-nullable because `data_sql` wraps the extraction in
        // `COALESCE(blocks.audience, $3)` (see `audience.rs`), so a block that was never stamped
        // (a legacy, pre-v8 row) materializes under the deployment default rather than as a
        // NULL. A block whose own stamp disagrees with the `streams`/`processes` row it joins to
        // never reaches this column at all -- `data_sql`'s NULL-tolerant mismatch predicate
        // excludes it from materialization entirely (§4). Dictionary-encoded like every other
        // view's audience column (`log_entries_table.rs`, `metrics_table.rs`,
        // `log_stats_view.rs`): one distinct value per partition in practice. `sql_arrow_bridge`
        // delivers it as plain `Utf8` (its mapping is keyed on the Postgres type name), so
        // `metadata_partition_spec::cast_to_file_schema` casts it to this declared type before
        // the write.
        Field::new(
            "audience",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
    ])
}

/// Returns the file schema hash for the blocks view.
///
/// Stays `vec![5]` for AbAC Stage 5b (#1518, §4) -- deliberately not bumped. The Arrow schema is
/// unchanged: `audience` keeps its name, type, and position, so today's partitions remain
/// byte-identical and valid to read under it. Existing partitions keep whatever `audience`
/// values they were materialized with under the old per-process-property query (sourced from the
/// owning process's `micromegas.audience` property rather than the block's own stamp) and
/// predate the mismatch predicate, so a partition may contain a row the predicate would now
/// exclude -- a consistency gap under the governing premise that all lake data before this stage
/// is public, not a confidentiality one. `regenerate_partitions` over `blocks` gives an operator
/// who wants uniform, per-row semantics sooner than the retention window a way to get it.
pub fn blocks_file_schema_hash() -> Vec<u8> {
    vec![5] // Bumped from vec![3] for the dictionary-encoded `audience` column (#1482)
}
