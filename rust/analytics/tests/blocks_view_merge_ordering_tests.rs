//! Offline (no live DB) regression tests for `tasks/blocks_view_ordered_merges_plan.md`:
//! - `make_partitioned_execution_plan` under `OrderingBounds::InsertTime` (§1): non-overlapping
//!   insert-time ranges elide the redundant `Sort`, overlapping ranges are rejected loudly.
//! - `QueryMerger::execute_merge_query`'s ordered path (§1/§4): a declared `insert_time` scan
//!   ordering over disjoint blocks-view partitions elides the merge's own `Sort` node -- including
//!   the single-non-empty-file shape -- and the plan-shape check fails open (`ordering_honored:
//!   false`, not an `Err`) when elision doesn't happen but the plan is still single-partition.

use chrono::{DateTime, TimeDelta, Utc};
use datafusion::physical_optimizer::PhysicalOptimizerRule;
use datafusion::physical_optimizer::enforce_sorting::EnforceSorting;
use datafusion::physical_plan::{ExecutionPlan, displayable};
use datafusion::prelude::SessionContext;
use micromegas_analytics::lakehouse::blocks_view::blocks_view_schema;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::merge::{PartitionMerger, QueryMerger};
use micromegas_analytics::lakehouse::metadata_cache::MetadataCache;
use micromegas_analytics::lakehouse::partition::Partition;
use micromegas_analytics::lakehouse::partition_cache::PartitionCache;
use micromegas_analytics::lakehouse::partitioned_execution_plan::{
    OrderingBounds, make_partitioned_execution_plan,
};
use micromegas_analytics::lakehouse::reader_factory::ReaderFactory;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view::{ScanSortColumn, ViewMetadata};
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

fn make_insert_time_partition(
    file_path: &str,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    sort_order: Option<Vec<String>>,
) -> Partition {
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new("blocks".to_owned()),
            view_instance_id: Arc::new("global".to_owned()),
            file_schema_hash: vec![3],
        },
        insert_time_range: TimeRange::new(begin_insert, end_insert),
        event_time_range: Some(TimeRange::new(begin_insert, end_insert)),
        updated: Utc::now(),
        file_path: Some(file_path.to_owned()),
        file_size: 1024,
        source_data_hash: vec![0],
        num_rows: 10,
        sort_order,
    }
}

fn make_empty_partition(begin_insert: DateTime<Utc>, end_insert: DateTime<Utc>) -> Partition {
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new("blocks".to_owned()),
            view_instance_id: Arc::new("global".to_owned()),
            file_schema_hash: vec![3],
        },
        insert_time_range: TimeRange::new(begin_insert, end_insert),
        event_time_range: None,
        updated: Utc::now(),
        file_path: None,
        file_size: 0,
        source_data_hash: vec![0],
        num_rows: 0,
        sort_order: None,
    }
}

fn make_reader_factory() -> Arc<ReaderFactory> {
    Arc::new(ReaderFactory::new(
        Arc::new(object_store::memory::InMemory::new()),
        Arc::new(MetadataCache::new(1024 * 1024)),
    ))
}

fn insert_time_ascending() -> Vec<ScanSortColumn> {
    vec![ScanSortColumn {
        column: Arc::new("insert_time".to_owned()),
        descending: false,
    }]
}

#[tokio::test]
async fn insert_time_overlapping_partitions_are_rejected() {
    let schema = Arc::new(blocks_view_schema());
    let t0 = Utc::now();
    let part_a = make_insert_time_partition("a.parquet", t0, t0 + TimeDelta::seconds(10), None);
    // part_b's begin_insert_time (t0+5s) is before part_a's end_insert_time (t0+10s): overlap.
    let part_b = make_insert_time_partition(
        "b.parquet",
        t0 + TimeDelta::seconds(5),
        t0 + TimeDelta::seconds(20),
        None,
    );
    let ctx = SessionContext::new();
    let state = ctx.state();
    let result = make_partitioned_execution_plan(
        schema,
        make_reader_factory(),
        &state,
        None,
        &[],
        None,
        Arc::new(vec![part_a, part_b]),
        &insert_time_ascending(),
        OrderingBounds::InsertTime,
    );
    assert!(
        result.is_err(),
        "overlapping insert-time partitions must be rejected instead of silently \
         mis-declaring the ordering"
    );
}

async fn build_plan_wrapped_in_insert_time_sort(
    partitions: Vec<Partition>,
) -> Arc<dyn ExecutionPlan> {
    let schema = Arc::new(blocks_view_schema());
    let ctx = SessionContext::new();
    let state = ctx.state();
    let plan = make_partitioned_execution_plan(
        schema.clone(),
        make_reader_factory(),
        &state,
        None,
        &[],
        None,
        Arc::new(partitions),
        &insert_time_ascending(),
        OrderingBounds::InsertTime,
    )
    .expect("plan should build");

    let sort_expr = datafusion::physical_expr::PhysicalSortExpr::new(
        Arc::new(
            datafusion::physical_expr::expressions::Column::new_with_schema("insert_time", &schema)
                .expect("insert_time column"),
        ),
        datafusion::arrow::compute::SortOptions {
            descending: false,
            nulls_first: false,
        },
    );
    let lex =
        datafusion::physical_expr::LexOrdering::new(vec![sort_expr]).expect("non-empty ordering");
    Arc::new(datafusion::physical_plan::sorts::sort::SortExec::new(
        lex, plan,
    ))
}

#[tokio::test]
async fn insert_time_non_overlapping_partitions_elide_redundant_sort() {
    let t0 = Utc::now();
    // Handed in reverse (non-begin_insert_time) order, to exercise the file-group sort as well
    // as the statistics/ordering declaration.
    let part_later = make_insert_time_partition(
        "later.parquet",
        t0 + TimeDelta::hours(2),
        t0 + TimeDelta::hours(2) + TimeDelta::seconds(10),
        Some(vec!["insert_time".to_owned()]),
    );
    let part_earlier = make_insert_time_partition(
        "earlier.parquet",
        t0,
        t0 + TimeDelta::seconds(10),
        Some(vec!["insert_time".to_owned()]),
    );
    let sorted_plan = build_plan_wrapped_in_insert_time_sort(vec![part_later, part_earlier]).await;
    let optimized = EnforceSorting::new()
        .optimize(sorted_plan, &Default::default())
        .expect("EnforceSorting should not fail");
    let plan_str = displayable(optimized.as_ref()).indent(false).to_string();
    assert!(
        !plan_str.contains("SortExec"),
        "expected the redundant Sort to be elided once the insert_time ordering is declared \
         and file statistics are attached, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn undeclared_insert_time_ordering_keeps_sort_negative_control() {
    let schema = Arc::new(blocks_view_schema());
    let t0 = Utc::now();
    let part_a = make_insert_time_partition("a.parquet", t0, t0 + TimeDelta::seconds(10), None);
    let part_b = make_insert_time_partition(
        "b.parquet",
        t0 + TimeDelta::seconds(10),
        t0 + TimeDelta::seconds(20),
        None,
    );
    let ctx = SessionContext::new();
    let state = ctx.state();
    let plan = make_partitioned_execution_plan(
        schema.clone(),
        make_reader_factory(),
        &state,
        None,
        &[],
        None,
        Arc::new(vec![part_a, part_b]),
        &[], // no declared ordering
        OrderingBounds::InsertTime,
    )
    .expect("plan should build");
    let sort_expr = datafusion::physical_expr::PhysicalSortExpr::new(
        Arc::new(
            datafusion::physical_expr::expressions::Column::new_with_schema("insert_time", &schema)
                .expect("insert_time column"),
        ),
        datafusion::arrow::compute::SortOptions {
            descending: false,
            nulls_first: false,
        },
    );
    let lex =
        datafusion::physical_expr::LexOrdering::new(vec![sort_expr]).expect("non-empty ordering");
    let sorted_plan: Arc<dyn ExecutionPlan> = Arc::new(
        datafusion::physical_plan::sorts::sort::SortExec::new(lex, plan),
    );
    let optimized = EnforceSorting::new()
        .optimize(sorted_plan, &Default::default())
        .expect("EnforceSorting should not fail");
    let plan_str = displayable(optimized.as_ref()).indent(false).to_string();
    assert!(
        plan_str.contains("SortExec"),
        "a scan that does not declare an ordering must still sort, got:\n{plan_str}"
    );
}

/// Builds an offline `LakehouseContext` (in-memory object store, lazily-connected -- never
/// actually queried -- Postgres pool) sufficient to run `QueryMerger::execute_merge_query`'s
/// planning without touching a real database or reading real Parquet files (the returned stream
/// is never polled in these tests, so the fabricated, nonexistent `file_path`s are never opened).
async fn make_offline_lakehouse_context() -> Arc<LakehouseContext> {
    let db_pool = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
        .expect("connect_lazy should not touch the network");
    let object_store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let blob_storage = Arc::new(BlobStorage::new(
        object_store,
        object_store::path::Path::from("lakehouse"),
    ));
    let lake = Arc::new(DataLakeConnection::new(db_pool, blob_storage));
    let runtime = Arc::new(make_runtime_env().expect("make_runtime_env"));
    Arc::new(LakehouseContext::new(lake, runtime))
}

fn make_ordered_merger(query: &str) -> QueryMerger {
    QueryMerger::new(
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        Arc::new(blocks_view_schema()),
        Arc::new(String::from(query)),
    )
    .with_merge_scan_ordering(insert_time_ascending())
}

#[tokio::test]
async fn ordered_merge_elides_sort_for_disjoint_multi_file_partitions() {
    let lakehouse = make_offline_lakehouse_context().await;
    let t0 = Utc::now();
    let part_a = make_insert_time_partition(
        "a.parquet",
        t0,
        t0 + TimeDelta::seconds(10),
        Some(vec!["insert_time".to_owned()]),
    );
    let part_b = make_insert_time_partition(
        "b.parquet",
        t0 + TimeDelta::seconds(10),
        t0 + TimeDelta::seconds(20),
        Some(vec!["insert_time".to_owned()]),
    );
    let insert_range = TimeRange::new(t0, t0 + TimeDelta::seconds(20));
    let merger = make_ordered_merger("SELECT * FROM source ORDER BY insert_time;");
    let result = merger
        .execute_merge_query(
            lakehouse,
            Arc::new(vec![part_a, part_b]),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("execute_merge_query should succeed");
    assert!(
        result.ordering_honored,
        "expected the declared insert_time ordering to be elided for a two-file group"
    );
}

#[tokio::test]
async fn ordered_merge_elides_sort_for_single_non_empty_file() {
    // Empty partitions are dropped before the scan is built (make_partitioned_execution_plan),
    // leaving exactly one file in the scan's file group -- the shape that, without
    // repartition_file_scans disabled, DataFusion 54's FileGroupPartitioner would otherwise
    // byte-range-split via repartition_evenly_by_size, requiring a SortPreservingMergeExec.
    let lakehouse = make_offline_lakehouse_context().await;
    let t0 = Utc::now();
    let part_a = make_insert_time_partition(
        "a.parquet",
        t0,
        t0 + TimeDelta::seconds(10),
        Some(vec!["insert_time".to_owned()]),
    );
    let empty_before = make_empty_partition(t0 - TimeDelta::seconds(10), t0);
    let empty_after =
        make_empty_partition(t0 + TimeDelta::seconds(10), t0 + TimeDelta::seconds(20));
    let insert_range = TimeRange::new(t0 - TimeDelta::seconds(10), t0 + TimeDelta::seconds(20));
    let merger = make_ordered_merger("SELECT * FROM source ORDER BY insert_time;");
    let result = merger
        .execute_merge_query(
            lakehouse,
            Arc::new(vec![empty_before, part_a, empty_after]),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("execute_merge_query should succeed");
    assert!(
        result.ordering_honored,
        "expected a single-non-empty-file merge to elide the sort as a plain concatenation, \
         not fall back to repartition_evenly_by_size + SortPreservingMergeExec"
    );
}

#[tokio::test]
async fn defeated_elision_reports_ordering_not_honored_without_erroring() {
    // A query direction (DESC) that contradicts the declared ascending ordering forces a real
    // Sort node to remain even with repartition_file_scans disabled (single partition, so no
    // SortPreservingMergeExec either) -- the fail-open regression signal from Design §1/Trade-offs:
    // the merge must still succeed and report ordering_honored: false, not error out.
    let lakehouse = make_offline_lakehouse_context().await;
    let t0 = Utc::now();
    let part_a = make_insert_time_partition(
        "a.parquet",
        t0,
        t0 + TimeDelta::seconds(10),
        Some(vec!["insert_time".to_owned()]),
    );
    let part_b = make_insert_time_partition(
        "b.parquet",
        t0 + TimeDelta::seconds(10),
        t0 + TimeDelta::seconds(20),
        Some(vec!["insert_time".to_owned()]),
    );
    let insert_range = TimeRange::new(t0, t0 + TimeDelta::seconds(20));
    let merger = make_ordered_merger("SELECT * FROM source ORDER BY insert_time DESC;");
    let result = merger
        .execute_merge_query(
            lakehouse,
            Arc::new(vec![part_a, part_b]),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("a defeated elision must not fail the merge");
    assert!(
        !result.ordering_honored,
        "expected the mismatched sort direction to prevent elision (ordering_honored: false)"
    );
}
