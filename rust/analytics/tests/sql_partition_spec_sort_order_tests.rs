//! Offline (no live DB) regression test for the fresh-write (extract) path's sort-applying
//! helper, `plan_sorted_extract`: when a `sort_order` is declared, it applies the declared
//! columns to the extract query's `DataFrame` as a `DataFrame::sort` before planning it, so the
//! resulting physical plan's output is already in the declared order even though the extract
//! query itself carries no `ORDER BY` -- the same discipline `QueryMerger::execute_sorted_merge`
//! already applies to the merge query.
//!
//! This runs entirely offline: planning and executing the extract query never touches Postgres.

use chrono::{TimeDelta, Utc};
use datafusion::arrow::array::{Array, RecordBatch, StringArray};
use datafusion::physical_plan::execute_stream;
use futures::TryStreamExt;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::NullPartitionProvider;
use micromegas_analytics::lakehouse::query::make_session_context;
use micromegas_analytics::lakehouse::read_scope::CallerContext;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::sql_partition_spec::plan_sorted_extract;
use micromegas_analytics::lakehouse::view::ScanSortColumn;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
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
    Arc::new(LakehouseContext::new(lake, runtime).expect("LakehouseContext::new"))
}

#[tokio::test]
async fn extract_query_without_an_order_by_satisfies_the_declared_sort_order() {
    let lakehouse = make_offline_lakehouse_context().await;
    let ctx = make_session_context(
        lakehouse.clone(),
        Arc::new(NullPartitionProvider {}),
        None,
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        CallerContext::maintenance(),
    )
    .await
    .expect("make_session_context");

    // Two distinct, out-of-order rows and no ORDER BY at all: unlike a single literal row (whose
    // columns DataFusion's equivalence properties treat as trivially-ordered constants), this
    // genuinely requires plan_sorted_extract to apply the declared sort_order for the output to
    // come out ordered.
    let extract_query = "SELECT * FROM (VALUES \
         ('b', TIMESTAMP '1970-01-01 00:00:01', 1), \
         ('a', TIMESTAMP '1970-01-01 00:00:00', 2)) AS t(name, time_bin, measure)";
    let df = ctx
        .sql(extract_query)
        .await
        .expect("planning the extract query");

    let declared_columns = ["name", "time_bin"].map(|c| ScanSortColumn {
        column: Arc::new(c.to_owned()),
        descending: false,
    });
    let insert_range = TimeRange::new(Utc::now(), Utc::now() + TimeDelta::hours(1));
    let (plan, task_ctx) = plan_sorted_extract(
        df,
        Some(&declared_columns),
        "extract query for test_metrics",
        insert_range,
    )
    .await
    .expect("plan_sorted_extract should apply the declared sort and plan cleanly");

    // plan_sorted_extract itself already returns Err when the plan isn't single-partition and
    // ordering-satisfying, so re-checking those properties on the plan it returned here would
    // only prove it returned Ok. Executing the plan and checking the emitted row order instead
    // proves the applied sort actually reorders the rows plan_sorted_extract is handed.
    let stream = execute_stream(plan, task_ctx).expect("executing the extract query plan");
    let batches: Vec<RecordBatch> = stream
        .try_collect()
        .await
        .expect("collecting the extract query stream");
    let names: Vec<String> = batches
        .iter()
        .flat_map(|batch| {
            let column = batch.column_by_name("name").expect("name column");
            let names = column
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("name should be a StringArray");
            (0..names.len())
                .map(|i| names.value(i).to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        names,
        vec!["a".to_owned(), "b".to_owned()],
        "plan_sorted_extract must apply the declared (name, time_bin) sort_order even though the \
         extract query itself carries no ORDER BY"
    );
}
