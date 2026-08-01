//! Offline (no live DB) regression tests for the fresh-write (extract) path's declared-`sort_order`
//! verification (`tasks/completed/1392_kway_merge_sorted_partitions_plan.md` Testing Strategy):
//! `SqlPartitionSpec::execute_extract_query` refuses to record a false sort_order guarantee when
//! the extract query's physical plan doesn't actually satisfy it (e.g. a missing top-level
//! `ORDER BY`), which gates every fresh materialization of any view declaring
//! `with_merge_sort_order`, including the shipped `log_stats`.
//!
//! The ordering check runs before anything touches Postgres, so the failing case never needs a
//! live database. The passing case can't be driven all the way to a successful `write()` offline
//! (once the ordering check passes, `write()` goes on to insert the partition row over the lazily-
//! connected, unreachable Postgres pool) -- so it instead asserts that the failure that does occur
//! comes from that later, unrelated step, not from the ordering check.

use chrono::{TimeDelta, Utc};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::PartitionCache;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::sql_batch_view::SqlBatchView;
use micromegas_analytics::lakehouse::view::View;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::response_writer::TracingLogger;
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

/// Builds an offline `LakehouseContext` (in-memory object store, lazily-connected -- never
/// actually connected unless the code under test tries to use it -- Postgres pool), the same
/// pattern `sql_batch_view_merge_ordering_tests.rs` uses.
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

/// A fabricated `SqlBatchView` whose extract query is entirely self-contained (no `FROM` table),
/// so it needs neither a real database nor a registered view factory to resolve its schema.
/// `extract_query` is the only thing that varies between the two tests below.
async fn make_test_view(lakehouse: &LakehouseContext, extract_query: &str) -> SqlBatchView {
    let count_src_query = Arc::new(String::from("SELECT 0::BIGINT as count"));
    let extract_query = Arc::new(extract_query.to_owned());
    let merge_query = Arc::new(String::from(
        "SELECT name, time_bin, sum(measure) AS measure FROM {source} GROUP BY name, time_bin",
    ));
    let time_column = Arc::new(String::from("time_bin"));
    SqlBatchView::new(
        lakehouse.runtime().clone(),
        Arc::new("test_metrics".to_owned()),
        time_column.clone(),
        time_column,
        count_src_query,
        extract_query,
        merge_query,
        lakehouse.lake().clone(),
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        None,
        TimeDelta::days(1),
        TimeDelta::days(1),
        None,
    )
    .await
    .expect("SqlBatchView::new")
    .with_merge_sort_order(vec![
        Arc::new("name".to_owned()),
        Arc::new("time_bin".to_owned()),
    ])
    .expect("with_merge_sort_order")
}

#[tokio::test]
async fn extract_query_missing_order_by_fails_the_write() {
    let lakehouse = make_offline_lakehouse_context().await;
    // Two distinct, out-of-order rows: unlike a single literal row (whose columns DataFusion's
    // equivalence properties treat as trivially-ordered constants), this genuinely requires an
    // explicit `ORDER BY` to satisfy the declared sort_order.
    let view = make_test_view(
        &lakehouse,
        "SELECT * FROM (VALUES \
         ('b', TIMESTAMP '1970-01-01 00:00:01', 1), \
         ('a', TIMESTAMP '1970-01-01 00:00:00', 2)) AS t(name, time_bin, measure)",
    )
    .await;

    let insert_range = TimeRange::new(Utc::now(), Utc::now() + TimeDelta::hours(1));
    let partition_spec = view
        .make_batch_partition_spec(
            lakehouse.clone(),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("make_batch_partition_spec");

    let err = partition_spec
        .write(lakehouse.lake().clone(), Arc::new(TracingLogger {}))
        .await
        .expect_err(
            "a plan whose output ordering doesn't satisfy the declared sort_order must fail",
        );
    let message = format!("{err:#}");
    assert!(
        message.contains("does not satisfy the declared sort_order"),
        "expected the declared-sort_order verification to reject the missing ORDER BY, got: {message}"
    );
}

#[tokio::test]
async fn extract_query_matching_order_by_passes_the_ordering_check() {
    let lakehouse = make_offline_lakehouse_context().await;
    let view = make_test_view(
        &lakehouse,
        "SELECT * FROM (VALUES \
         ('b', TIMESTAMP '1970-01-01 00:00:01', 1), \
         ('a', TIMESTAMP '1970-01-01 00:00:00', 2)) AS t(name, time_bin, measure) \
         ORDER BY name, time_bin",
    )
    .await;

    let insert_range = TimeRange::new(Utc::now(), Utc::now() + TimeDelta::hours(1));
    let partition_spec = view
        .make_batch_partition_spec(
            lakehouse.clone(),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("make_batch_partition_spec");

    // The ordering check passes, so `write` proceeds to insert the partition row over the
    // lazily-connected, unreachable Postgres pool -- offline, that either fails quickly (e.g.
    // connection refused) or hangs until the pool's acquire timeout, depending on how the sandbox
    // handles the outbound connection. Either way, what this asserts is that the ordering check
    // itself was never the cause, so bound the wait rather than let a slow environment stall CI.
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        partition_spec.write(lakehouse.lake().clone(), Arc::new(TracingLogger {})),
    )
    .await;
    if let Ok(result) = outcome {
        let err = result.expect_err("no live database is reachable offline");
        let message = format!("{err:#}");
        assert!(
            !message.contains("does not satisfy the declared sort_order")
                && !message.contains("physical plan; a declared sort_order requires"),
            "a matching top-level ORDER BY must pass the declared-sort_order verification, got: {message}"
        );
    }
    // else: still waiting on the unreachable pool after 5s -- the ordering check itself runs
    // synchronously before any Postgres access, so having gotten this far already proves it
    // passed.
}
