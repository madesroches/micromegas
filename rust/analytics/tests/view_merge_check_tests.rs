//! Offline (no live DB) tests for `check_merge_streaming` and the `check_view_merge` UDTF: the
//! check plans over synthetic partitions and never reads an object or queries Postgres.

use datafusion::arrow::array::{Array, BooleanArray, StringArray};
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::NullPartitionProvider;
use micromegas_analytics::lakehouse::query::{SessionRange, make_session_context};
use micromegas_analytics::lakehouse::read_scope::CallerContext;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view_ddl::{ViewDdl, parse_view_ddl};
use micromegas_analytics::lakehouse::view_definition::ViewDefinition;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::lakehouse::view_merge_check::check_merge_streaming;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

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

const EXTRACT: &str = "SELECT 'a' AS k1, 'b' AS k2, TIMESTAMP '1970-01-01 00:00:00' AS first_time, \
     make_array(1, 2) AS items";

fn ddl(merge_query: &str, sort_order: Option<&str>) -> String {
    let sort_option = sort_order
        .map(|s| format!(", merge_sort_order = '{s}'"))
        .unwrap_or_default();
    format!(
        "CREATE MATERIALIZED VIEW merge_check_probe WITH (\
         extract_query = $${EXTRACT}$$, \
         count_src_query = $$SELECT 0 AS count$$, \
         merge_partitions_query = $${merge_query}$$, \
         update_group = 1, \
         time_column = 'first_time'{sort_option})"
    )
}

fn definition(ddl_text: &str) -> ViewDefinition {
    match parse_view_ddl(ddl_text).expect("parse").expect("is ddl") {
        ViewDdl::Create { definition, .. } => *definition,
        ViewDdl::Drop { .. } => panic!("expected Create"),
    }
}

const STREAMING_MERGE: &str = "SELECT k1, k2, min(first_time) AS first_time, \
     first_value(items) AS items FROM {source} GROUP BY k1, k2";

const BUFFERING_MERGE: &str = "SELECT k1, k2, min(first_time) AS first_time, \
     flatten(array_agg(items ORDER BY first_time)) AS items FROM {source} GROUP BY k1, k2";

async fn check(
    merge_query: &str,
    sort_order: Option<&str>,
) -> Option<micromegas_analytics::lakehouse::view_merge_check::MergeStreamingReport> {
    let lakehouse = make_offline_lakehouse_context().await;
    let def = definition(&ddl(merge_query, sort_order));
    check_merge_streaming(
        &def,
        lakehouse,
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
    )
    .await
    .expect("check_merge_streaming")
}

#[tokio::test]
async fn ordered_group_by_streams() {
    let report = check(STREAMING_MERGE, Some("k1, k2"))
        .await
        .expect("a report");
    assert!(report.streams, "plan:\n{}", report.plan);
    assert!(report.sort_exec_line.is_none());
    assert_eq!(report.merge_sort_order, vec!["k1", "k2"]);
}

#[tokio::test]
async fn array_agg_with_inner_order_by_buffers() {
    let report = check(BUFFERING_MERGE, Some("k1, k2"))
        .await
        .expect("a report");
    assert!(!report.streams, "plan:\n{}", report.plan);
    assert!(
        report
            .sort_exec_line
            .as_deref()
            .is_some_and(|l| l.contains("SortExec")),
        "plan:\n{}",
        report.plan
    );
}

#[tokio::test]
async fn definition_without_merge_sort_order_yields_no_report() {
    assert!(check(BUFFERING_MERGE, None).await.is_none());
}

async fn run_udtf(ddl_text: &str) -> datafusion::error::Result<Vec<RecordBatch>> {
    let lakehouse = make_offline_lakehouse_context().await;
    let ctx = make_session_context(
        lakehouse,
        Arc::new(NullPartitionProvider {}),
        SessionRange::Snapshot,
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        CallerContext::maintenance(),
    )
    .await
    .expect("make_session_context");
    let escaped = ddl_text.replace('\'', "''");
    ctx.sql(&format!("SELECT * FROM check_view_merge('{escaped}')"))
        .await?
        .collect()
        .await
}

#[tokio::test]
async fn udtf_reports_the_buffering_merge() {
    let batches = run_udtf(&ddl(BUFFERING_MERGE, Some("k1, k2")))
        .await
        .expect("udtf");
    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 1);
    let schema = batch.schema();
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        names,
        [
            "view_set_name",
            "merge_sort_order",
            "streams",
            "sort_exec_line",
            "plan"
        ]
    );
    let streams = batch
        .column(2)
        .as_any()
        .downcast_ref::<BooleanArray>()
        .expect("streams");
    assert!(!streams.value(0));
    let order = batch
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("merge_sort_order");
    assert_eq!(order.value(0), "k1, k2");
    assert!(!batch.column(3).is_null(0));
}

#[tokio::test]
async fn udtf_returns_no_row_without_merge_sort_order() {
    let batches = run_udtf(&ddl(BUFFERING_MERGE, None)).await.expect("udtf");
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn udtf_rejects_non_create_input() {
    assert!(run_udtf("SELECT 1").await.is_err());
    assert!(
        run_udtf("DROP MATERIALIZED VIEW merge_check_probe")
            .await
            .is_err()
    );
}
