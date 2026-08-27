//! Offline (no live DB) regression test for `log_stats`' adoption of order-preserving k-way
//! merges (`tasks/completed/1392_kway_merge_sorted_partitions_plan.md` Design §7, Testing Strategy item 6):
//! unlike `per_file_scan_ordering_tests.rs` and `sql_batch_view_merge_ordering_tests.rs`, which use
//! fabricated views, this pins the *shipped* `log_stats` view definition, so a later edit to its
//! `ORDER BY`/`GROUP BY` that silently breaks the streaming contract fails CI instead of quietly
//! degrading to the unordered merge.
//!
//! It plans the shipped merge query itself over the view's own declared scan ordering rather than
//! going through `View::merge_partitions`: the plan shape is what's under test, and executing the
//! merge would only hide it behind a stream. It also plans the shipped *extract* query and checks
//! its output ordering, pinning the other half of the streaming contract: the top-level `ORDER BY`
//! that `SqlPartitionSpec::execute_extract_query` requires before it will record a fresh
//! partition's `sort_order` guarantee.

use chrono::{TimeDelta, Utc};
use datafusion::physical_plan::displayable;
use datafusion::prelude::{SessionConfig, SessionContext};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::log_stats_view::make_log_stats_view;
use micromegas_analytics::lakehouse::log_view::LogViewMaker;
use micromegas_analytics::lakehouse::metadata_cache::MetadataCache;
use micromegas_analytics::lakehouse::partition::Partition;
use micromegas_analytics::lakehouse::partition_cache::NullPartitionProvider;
use micromegas_analytics::lakehouse::partitioned_execution_plan::make_lex_ordering;
use micromegas_analytics::lakehouse::partitioned_table_provider::PartitionedTableProvider;
use micromegas_analytics::lakehouse::query::make_session_context;
use micromegas_analytics::lakehouse::read_scope::CallerContext;
use micromegas_analytics::lakehouse::reader_factory::ReaderFactory;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view::{ScanSortColumn, View, ViewMetadata};
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, ViewMaker};
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

/// Builds an offline `LakehouseContext` (in-memory object store, lazily-connected -- never
/// actually queried -- Postgres pool) sufficient to build the shipped `log_stats` view without
/// touching a real database.
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
    Arc::new(LakehouseContext::new(lake, runtime).expect("LakehouseContext::new"))
}

fn make_certifying_partition(file_path: &str, index: i64) -> Partition {
    let t0 = Utc::now() + TimeDelta::days(index);
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new("log_stats".to_owned()),
            view_instance_id: Arc::new("global".to_owned()),
            file_schema_hash: vec![0],
        },
        insert_time_range: TimeRange::new(t0, t0 + TimeDelta::hours(1)),
        event_time_range: Some(TimeRange::new(t0, t0 + TimeDelta::hours(1))),
        updated: t0,
        file_path: Some(file_path.to_owned()),
        file_size: 1024,
        source_data_hash: vec![0],
        num_rows: 10,
        sort_order: Some(vec![
            "time_bin".to_owned(),
            "process_id".to_owned(),
            "level".to_owned(),
            "target".to_owned(),
        ]),
        max_sort_key_time: None,
    }
}

#[tokio::test]
async fn log_stats_merge_query_stays_a_streaming_kway_merge() {
    let lakehouse = make_offline_lakehouse_context().await;

    // log_stats' extract query reads FROM log_entries, so make_log_stats_view's own schema
    // resolution (SqlBatchView::new) needs that table resolvable -- mirrors how
    // view_factory::default_view_factory wires log_stats up in production.
    let log_view_maker = LogViewMaker {};
    let log_entries_view = log_view_maker
        .make_view("global")
        .expect("log_entries global view");
    let mut view_factory = ViewFactory::new(vec![log_entries_view]);
    view_factory.add_view_set(String::from("log_entries"), Arc::new(LogViewMaker {}));

    let view = make_log_stats_view(
        lakehouse.runtime().clone(),
        lakehouse.lake().clone(),
        Arc::new(view_factory),
    )
    .await
    .expect("make_log_stats_view");

    let partitions = vec![
        make_certifying_partition("a.parquet", 0),
        make_certifying_partition("b.parquet", 1),
        make_certifying_partition("c.parquet", 2),
    ];
    assert_eq!(
        view.get_merged_partition_sort_order(&partitions),
        Some(vec![
            "time_bin".to_owned(),
            "process_id".to_owned(),
            "level".to_owned(),
            "target".to_owned(),
        ]),
        "log_stats must record the sort_order its certifying inputs carry -- the guarantee the plan \
         shape below has to back up"
    );

    // The same source shape QueryMerger's PerFile branch registers: one file group per partition,
    // all declaring the view's own scan ordering. target_partitions is pinned above the partition
    // count so the assertions don't silently no-op on a low-core-count CI runner.
    let reader_factory = Arc::new(ReaderFactory::new(
        Arc::new(object_store::memory::InMemory::new()),
        Arc::new(MetadataCache::new(1024 * 1024)),
    ));
    let source = PartitionedTableProvider::with_scan_ordering(
        view.get_file_schema(),
        reader_factory,
        Arc::new(partitions),
        view.get_scan_output_ordering(),
    );
    let ctx = SessionContext::new_with_config(SessionConfig::new().with_target_partitions(8));
    ctx.register_table("source", Arc::new(source))
        .expect("register_table");
    let plan = ctx
        .sql(
            &view
                .get_merge_partitions_query()
                .replace("{source}", "source"),
        )
        .await
        .expect("planning log_stats' shipped merge query")
        .create_physical_plan()
        .await
        .expect("create_physical_plan");
    let plan_str = displayable(plan.as_ref()).indent(true).to_string();

    // `GROUP BY` gained a fifth key, `audience`, which isn't part of the declared scan ordering,
    // so DataFusion now plans this as `PartiallySorted` rather than fully `Sorted`. Both are
    // still a streaming aggregation with no blocking sort -- the assertion that actually matters
    // is the absence of `SortExec` below.
    assert!(
        plan_str.contains("ordering_mode=Sorted")
            || plan_str.contains("ordering_mode=PartiallySorted"),
        "expected order-aware aggregation over the declared (time_bin, process_id, level, target) \
         ordering (Sorted or PartiallySorted, given the added `audience` GROUP BY key), got:\n{plan_str}"
    );
    assert!(
        !plan_str.contains("SortExec"),
        "log_stats' shipped merge query must stay a streaming k-way merge with no blocking sort, \
         got:\n{plan_str}"
    );
}

/// Pins the other half of the streaming contract: the *extract* query's declared top-level
/// `ORDER BY` (Design §3 of `tasks/completed/1392_kway_merge_sorted_partitions_plan.md`). This is
/// what `SqlPartitionSpec::execute_extract_query` (`sql_partition_spec.rs`) checks before recording
/// a fresh partition's `sort_order`: a single-partition physical plan whose output ordering
/// satisfies the declared `(time_bin, process_id, level, target)` columns. If line 43 of
/// `log_stats_view.rs` (`ORDER BY time_bin, process_id, level, target`) were dropped or reordered,
/// that check would `anyhow::bail!` and every fresh `log_stats` materialization would fail -- this
/// test catches that offline, without needing a live database.
#[tokio::test]
async fn log_stats_extract_query_satisfies_its_declared_sort_order() {
    let lakehouse = make_offline_lakehouse_context().await;

    let log_view_maker = LogViewMaker {};
    let log_entries_view = log_view_maker
        .make_view("global")
        .expect("log_entries global view");
    let mut view_factory = ViewFactory::new(vec![log_entries_view]);
    view_factory.add_view_set(String::from("log_entries"), Arc::new(LogViewMaker {}));
    let view_factory = Arc::new(view_factory);

    let view = make_log_stats_view(
        lakehouse.runtime().clone(),
        lakehouse.lake().clone(),
        view_factory.clone(),
    )
    .await
    .expect("make_log_stats_view");

    // Mirrors how `SqlBatchView::make_batch_partition_spec` plans the extract query: an ordinary
    // session context with `log_entries` (empty, via `NullPartitionProvider`) registered as a
    // table, no live database involved.
    let ctx = make_session_context(
        lakehouse.clone(),
        Arc::new(NullPartitionProvider {}),
        None,
        view_factory,
        Arc::new(NoOpSessionConfigurator),
        CallerContext::maintenance(),
    )
    .await
    .expect("make_session_context");

    let now_str = Utc::now().to_rfc3339();
    let sql = view
        .get_extract_query()
        .replace("{begin}", &now_str)
        .replace("{end}", &now_str);
    let plan = ctx
        .sql(&sql)
        .await
        .expect("planning log_stats' shipped extract query")
        .create_physical_plan()
        .await
        .expect("create_physical_plan");

    let partition_count = plan.properties().output_partitioning().partition_count();
    assert_eq!(
        partition_count, 1,
        "a declared sort_order requires the extract query's physical plan to be single-partition, \
         got {partition_count}"
    );

    let declared_columns = ["time_bin", "process_id", "level", "target"].map(|c| ScanSortColumn {
        column: Arc::new(c.to_owned()),
        descending: false,
    });
    let lex = make_lex_ordering(&plan.schema(), &declared_columns)
        .expect("building the declared extract-query ordering")
        .expect("declared sort_order columns must be non-empty");
    let ordering_satisfied = plan
        .properties()
        .equivalence_properties()
        .ordering_satisfy(lex)
        .expect("checking extract query plan output ordering");
    assert!(
        ordering_satisfied,
        "log_stats' shipped extract query must produce output already ordered by \
         (time_bin, process_id, level, target); check for a missing or reordered top-level ORDER BY"
    );
}
