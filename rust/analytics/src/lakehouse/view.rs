use super::{
    batch_update::PartitionCreationStrategy,
    dataframe_time_bounds::DataFrameTimeBounds,
    lakehouse_context::LakehouseContext,
    materialized_view::MaterializedView,
    merge::{MergeQueryResult, PartitionMerger, QueryMerger},
    partition::Partition,
    partition_cache::PartitionCache,
    partitioned_execution_plan::ScanOrdering,
    session_configurator::NoOpSessionConfigurator,
    view_factory::ViewFactory,
};
use crate::{response_writer::Logger, time::TimeRange};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{arrow::datatypes::Schema, logical_expr::Expr, prelude::*, sql::TableReference};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::fmt::Debug;
use std::sync::Arc;

/// A trait for defining a partition specification.
#[async_trait]
pub trait PartitionSpec: Send + Sync + Debug {
    /// Returns true if the partition is empty.
    fn is_empty(&self) -> bool;
    /// Returns a hash of the source data.
    fn get_source_data_hash(&self) -> Vec<u8>;
    /// Writes the partition to the data lake.
    async fn write(&self, lake: Arc<DataLakeConnection>, logger: Arc<dyn Logger>) -> Result<()>;
}

/// Metadata about a view.
#[derive(Debug, Clone)]
pub struct ViewMetadata {
    pub view_set_name: Arc<String>,
    pub view_instance_id: Arc<String>,
    pub file_schema_hash: Vec<u8>,
}

/// A column an ordering is expressed over (ascending unless `descending`).
#[derive(Clone, Debug)]
pub struct ScanSortColumn {
    pub column: Arc<String>,
    pub descending: bool,
}

/// A trait for defining a view.
#[async_trait]
pub trait View: std::fmt::Debug + Send + Sync {
    /// name of the table from the user's perspective
    fn get_view_set_name(&self) -> Arc<String>;

    /// get_view_instance_id can be a process_id, a stream_id or 'global'.
    fn get_view_instance_id(&self) -> Arc<String>;

    /// make_batch_partition_spec determines what should be found in an up to date partition.
    /// The resulting PartitionSpec can be used to validate existing partitions are create a new one.
    async fn make_batch_partition_spec(
        &self,
        lakehouse: Arc<LakehouseContext>,
        existing_partitions: Arc<PartitionCache>,
        insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>>;

    /// get_file_schema_hash returns a hash (can be a version number, version string, etc.) that allows
    /// to identify out of date partitions.
    fn get_file_schema_hash(&self) -> Vec<u8>;

    /// get_file_schema returns the schema of the partition file in object storage
    fn get_file_schema(&self) -> Arc<Schema>;

    /// jit_update creates or updates process-specific partitions before a query
    async fn jit_update(
        &self,
        lakehouse: Arc<LakehouseContext>,
        query_range: Option<TimeRange>,
    ) -> Result<()>;

    /// make_time_filter returns a set of expressions that will filter out the rows of the partition
    /// outside the time range requested.
    fn make_time_filter(&self, _begin: DateTime<Utc>, _end: DateTime<Utc>) -> Result<Vec<Expr>>;

    // a view must provide a way to compute the time bounds of a DataFrame corresponding to its schema
    fn get_time_bounds(&self) -> Arc<dyn DataFrameTimeBounds>;

    /// register the table in the SessionContext
    async fn register_table(&self, ctx: &SessionContext, table: MaterializedView) -> Result<()> {
        let view_set_name = self.get_view_set_name().to_string();
        ctx.register_table(
            TableReference::Bare {
                table: view_set_name.into(),
            },
            Arc::new(table),
        )?;
        Ok(())
    }

    async fn merge_partitions(
        &self,
        lakehouse: Arc<LakehouseContext>,
        partitions_to_merge: Arc<Vec<Partition>>,
        partitions_all_views: Arc<PartitionCache>,
        insert_range: TimeRange,
    ) -> Result<MergeQueryResult> {
        let merge_query = Arc::new(String::from("SELECT * FROM source;"));
        let empty_view_factory = Arc::new(ViewFactory::new(vec![]));
        let merger = QueryMerger::new(
            empty_view_factory,
            Arc::new(NoOpSessionConfigurator),
            self.get_file_schema(),
            merge_query,
        );
        merger
            .execute_merge_query(
                lakehouse,
                partitions_to_merge,
                partitions_all_views,
                insert_range,
            )
            .await
    }

    /// Returns the sort guarantee a merge of `partitions_to_merge` will actually produce, to be
    /// recorded as the resulting partition's `Partition::sort_order` (see
    /// `merge::create_merged_partition`).
    ///
    /// This is a distinct concept from `get_scan_output_ordering()`: that one is a trusted
    /// scan-ordering declaration consumed during physical planning, while this one is a record of
    /// what a specific merge actually produced, computed purely from the input partitions (before
    /// `merge_partitions` runs) and independent of whether DataFusion's elision optimization
    /// happened to succeed for this particular run.
    ///
    /// Default: `None` -- no guarantee recorded, ignoring the argument.
    fn get_merged_partition_sort_order(
        &self,
        _partitions_to_merge: &[Partition],
    ) -> Option<Vec<String>> {
        None
    }

    /// tells the daemon which view should be materialized and in what order
    fn get_update_group(&self) -> Option<i32>;

    /// allow the view to subdivide the requested partition
    fn get_max_partition_time_delta(&self, _strategy: &PartitionCreationStrategy) -> TimeDelta {
        TimeDelta::days(1)
    }

    /// Declares an ordering the view's partition scan *already* emits, letting DataFusion
    /// elide redundant `Sort` nodes for queries that `ORDER BY` these columns.
    ///
    /// **A large or memory-heavy merge is not, on its own, a reason to declare an ordering here.**
    /// Every `QueryMerger` merge scans its source partitions with a single sequential reader
    /// regardless of what this method returns (`make_merge_session_context`,
    /// `tasks/1491_merge_scan_memory_plan.md`), so a view with no natural sort contract gets a
    /// bounded-memory merge for free, with no ordering guarantee to maintain. Reach for a declared
    /// ordering only when a *query-side* need justifies it -- eliding a `Sort` for queries that
    /// already `ORDER BY` these columns, or (via `PerFile`) a streaming k-way merge that must
    /// preserve per-file order across an aggregation -- not to bound a merge's memory.
    ///
    /// Returning a non-`Unordered` value is a correctness contract the view must guarantee, and
    /// its shape depends on which variant is returned:
    /// - `Concatenated { columns, .. }`: rows within each partition file are already sorted by
    ///   `columns`, the leading column is the view's min-event-time column, and partition
    ///   event-time ranges are non-overlapping (so files concatenate in globally-sorted order).
    ///   For `ThreadSpansView`, the *rows-sorted-within-a-file* half is obtained from JIT
    ///   partitions being grouped under `jit_partitions::BlockOrder::EventTime`:
    ///   `group_blocks_into_partitions` sorts a segment's blocks by event time and only cuts at
    ///   insert-safe points (see its docs), so within a segment blocks land in event order rather
    ///   than relying on an unenforced assumption about registration order, and
    ///   `thread_spans_view::ensure_begin_non_decreasing` checks the resulting batch at write
    ///   time.
    ///
    ///   The *partition-ranges-don't-overlap* half is **not** established by that grouping alone --
    ///   a partition's declared event-time bounds come from its blocks' `begin_ticks`/`end_ticks`,
    ///   not from the rows -- but by design both producers' consecutive blocks now *touch* exactly:
    ///   - Both `micromegas_tracing::dispatch`'s four flush paths and the Unreal producer
    ///     (`MicromegasTracing/Private/Dispatch.cpp`) take a single shared timestamp per flush and
    ///     use it for both the outgoing block's close and the replacement block's `begin`, so
    ///     `block[k].end_ticks == block[k+1].begin_ticks` by construction. This is the design intent
    ///     the shared boundary stamp exists for: it lets call trees merge seamlessly across the cut
    ///     (`group_contiguous_block_chains` treats touching blocks as chain-connected).
    ///   - Data from `micromegas_tracing` versions predating that fix (or any other producer that
    ///     stamps two separate timestamps) can still strictly overlap at block boundaries -- this is
    ///     a legacy producer bug, not a supported producer shape, but the server tolerates it: the
    ///     view records `max_sort_key_time` (the true max leading-sort-column value, exact per
    ///     partition), and `partitioned_execution_plan::partition_bounds` reads that in preference
    ///     to `max_event_time` for the non-overlap check, which the swap-window argument in
    ///     `tasks/completed/thread_spans_segment_boundary_overlap_plan.md` shows can never trip for cuts at
    ///     block boundaries of either producer. Partitions written before that column existed (or by
    ///     a view that never populates it) fall back to the old, looser `max_event_time` bound, which
    ///     is self-healing rather than a permanent gap: `ThreadSpansView::SCHEMA_VERSION`'s bump makes
    ///     every pre-existing partition stale by schema hash, so it rebuilds -- carrying
    ///     `max_sort_key_time` -- automatically on its next query, no admin `retire_partitions` call
    ///     required.
    ///
    ///   Two residual caveats therefore remain, both backstopped by
    ///   `sort_and_check_non_overlapping` (`partitioned_execution_plan.rs`) failing the query
    ///   loudly rather than returning wrong rows: an insert-time inversion straddling a JIT
    ///   *segment* boundary (segments are still grouped independently, see
    ///   `generate_stream_jit_partitions`), which produces a genuine row-level overlap that the
    ///   check correctly rejects; and TSC-frequency re-estimation drift across materialization
    ///   epochs for `tsc_frequency == 0` processes, which can skew bounds written under different
    ///   converters.
    /// - `PerFile { columns }`: rows within each partition file are already sorted, ascending, by
    ///   `columns`, but partitions may overlap each other arbitrarily on those columns. A false
    ///   declaration here is not merely mis-ordered rows but, under order-aware aggregation, wrong
    ///   aggregate results (groups closed early, duplicate group keys) -- see
    ///   `SqlBatchView::with_merge_sort_order`. What makes declaring this safe is the
    ///   recorded-`sort_order` certification gate inside `make_partitioned_execution_plan`: every
    ///   non-empty partition must certify `columns` before the declaration reaches DataFusion at
    ///   all.
    ///
    /// Default: `Unordered` (no declared ordering — DataFusion sorts as usual).
    fn get_scan_output_ordering(&self) -> ScanOrdering {
        ScanOrdering::Unordered
    }
}

impl dyn View {
    pub fn get_meta(&self) -> ViewMetadata {
        ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
        }
    }
}
