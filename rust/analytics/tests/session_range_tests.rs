//! Offline (no live DB) tests for `SessionRange` handling in `MaterializedView::scan` and
//! `make_session_context`, over a fake view that counts its `jit_update` calls.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::catalog::TableProvider;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::SessionContext;
use micromegas_analytics::lakehouse::dataframe_time_bounds::{
    DataFrameTimeBounds, NamedColumnsTimeBounds,
};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::materialized_view::MaterializedView;
use micromegas_analytics::lakehouse::partition::Partition;
use micromegas_analytics::lakehouse::partition_cache::{
    NullPartitionProvider, PartitionCache, QueryPartitionProvider,
};
use micromegas_analytics::lakehouse::query::{SessionRange, make_session_context};
use micromegas_analytics::lakehouse::read_scope::CallerContext;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view::{PartitionSpec, View};
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

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

fn fake_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("a", DataType::Int64, false),
        Field::new("b", DataType::Int64, false),
    ]))
}

#[derive(Debug)]
struct FakeView {
    requires_range: bool,
    jit_calls: Mutex<Vec<Option<TimeRange>>>,
}

impl FakeView {
    fn new(requires_range: bool) -> Arc<Self> {
        Arc::new(Self {
            requires_range,
            jit_calls: Mutex::new(vec![]),
        })
    }

    fn calls(&self) -> Vec<Option<TimeRange>> {
        self.jit_calls.lock().expect("lock").clone()
    }
}

#[async_trait]
impl View for FakeView {
    fn get_view_set_name(&self) -> Arc<String> {
        Arc::new("fake_view".to_string())
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        Arc::new("global".to_string())
    }

    async fn make_batch_partition_spec(
        &self,
        _lakehouse: Arc<LakehouseContext>,
        _existing_partitions: Arc<PartitionCache>,
        _insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>> {
        unimplemented!("not used by the scan tests")
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![1]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        fake_schema()
    }

    async fn jit_update(
        &self,
        _lakehouse: Arc<LakehouseContext>,
        query_range: Option<TimeRange>,
    ) -> Result<()> {
        self.jit_calls.lock().expect("lock").push(query_range);
        Ok(())
    }

    fn make_time_filter(&self, _begin: DateTime<Utc>, _end: DateTime<Utc>) -> Result<Vec<Expr>> {
        Ok(vec![])
    }

    fn get_time_bounds(&self) -> Arc<dyn DataFrameTimeBounds> {
        Arc::new(NamedColumnsTimeBounds::new(
            Arc::new("a".to_string()),
            Arc::new("b".to_string()),
        ))
    }

    fn get_update_group(&self) -> Option<i32> {
        if self.requires_range { None } else { Some(1) }
    }

    fn requires_query_range(&self) -> bool {
        self.requires_range
    }
}

#[derive(Debug)]
struct PanickingProvider;

impl fmt::Display for PanickingProvider {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "PanickingProvider")
    }
}

#[async_trait]
impl QueryPartitionProvider for PanickingProvider {
    async fn fetch(
        &self,
        _view_set_name: &str,
        _view_instance_id: &str,
        _query_range: Option<TimeRange>,
        _file_schema_hash: Vec<u8>,
    ) -> Result<Vec<Partition>> {
        panic!("NoPartitions must not fetch partitions");
    }
}

/// Records the query range of every fetch.
#[derive(Debug, Default)]
struct RecordingProvider {
    fetches: Mutex<Vec<Option<TimeRange>>>,
}

impl fmt::Display for RecordingProvider {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "RecordingProvider")
    }
}

#[async_trait]
impl QueryPartitionProvider for RecordingProvider {
    async fn fetch(
        &self,
        _view_set_name: &str,
        _view_instance_id: &str,
        query_range: Option<TimeRange>,
        _file_schema_hash: Vec<u8>,
    ) -> Result<Vec<Partition>> {
        self.fetches.lock().expect("lock").push(query_range);
        Ok(vec![])
    }
}

async fn scan(
    lakehouse: &Arc<LakehouseContext>,
    view: Arc<FakeView>,
    provider: Arc<dyn QueryPartitionProvider>,
    range: SessionRange,
) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
    let table = MaterializedView::new(
        lakehouse.clone(),
        lakehouse.reader_factory().clone(),
        view,
        provider,
        range,
        None,
    );
    let ctx = SessionContext::new();
    let state = ctx.state();
    table.scan(&state, None, &[], None).await
}

fn a_range() -> TimeRange {
    let now = Utc::now();
    TimeRange::new(now - TimeDelta::hours(1), now)
}

#[tokio::test]
async fn unbounded_scan_of_a_range_requiring_view_errors_without_jit() {
    let lakehouse = make_offline_lakehouse_context().await;
    let view = FakeView::new(true);
    let err = scan(
        &lakehouse,
        view.clone(),
        Arc::new(NullPartitionProvider {}),
        SessionRange::Unbounded,
    )
    .await
    .expect_err("an unbounded scan of an on-demand view must fail");
    let msg = err.to_string();
    assert!(msg.contains("fake_view"), "unexpected error: {msg}");
    assert!(
        msg.contains("needs a query time range"),
        "unexpected error: {msg}"
    );
    assert!(view.calls().is_empty());
}

#[tokio::test]
async fn unbounded_scan_of_an_ordinary_view_is_unchanged() {
    let lakehouse = make_offline_lakehouse_context().await;
    let view = FakeView::new(false);
    scan(
        &lakehouse,
        view.clone(),
        Arc::new(NullPartitionProvider {}),
        SessionRange::Unbounded,
    )
    .await
    .expect("scan");
    assert_eq!(view.calls(), vec![None]);
}

#[tokio::test]
async fn no_partitions_scan_is_an_empty_plan_with_the_schema() {
    let lakehouse = make_offline_lakehouse_context().await;
    let view = FakeView::new(true);
    let plan = scan(
        &lakehouse,
        view.clone(),
        Arc::new(PanickingProvider),
        SessionRange::NoPartitions,
    )
    .await
    .expect("scan");
    assert_eq!(plan.schema(), fake_schema());
    assert!(view.calls().is_empty());
}

#[tokio::test]
async fn snapshot_scan_runs_jit_without_a_range_then_fetches() {
    let lakehouse = make_offline_lakehouse_context().await;
    let view = FakeView::new(true);
    let provider = Arc::new(RecordingProvider::default());
    scan(
        &lakehouse,
        view.clone(),
        provider.clone(),
        SessionRange::Snapshot,
    )
    .await
    .expect("scan");
    assert_eq!(view.calls(), vec![None]);
    assert_eq!(*provider.fetches.lock().expect("lock"), vec![None]);
}

#[tokio::test]
async fn query_scan_runs_jit_over_the_range() {
    let lakehouse = make_offline_lakehouse_context().await;
    let view = FakeView::new(true);
    let provider = Arc::new(RecordingProvider::default());
    let range = a_range();
    scan(
        &lakehouse,
        view.clone(),
        provider.clone(),
        SessionRange::Query(range),
    )
    .await
    .expect("scan");
    assert_eq!(view.calls(), vec![Some(range)]);
    assert_eq!(*provider.fetches.lock().expect("lock"), vec![Some(range)]);
}

#[tokio::test]
async fn only_a_query_range_installs_the_table_scan_rewrite() {
    let lakehouse = make_offline_lakehouse_context().await;
    let cases = [
        (SessionRange::Query(a_range()), true),
        (SessionRange::Unbounded, false),
        (SessionRange::Snapshot, false),
        (SessionRange::NoPartitions, false),
    ];
    for (range, expect_rewrite) in cases {
        let ctx = make_session_context(
            lakehouse.clone(),
            Arc::new(NullPartitionProvider {}),
            range,
            Arc::new(ViewFactory::new(vec![])),
            Arc::new(NoOpSessionConfigurator),
            CallerContext::maintenance(),
        )
        .await
        .expect("make_session_context");
        let has_rewrite = ctx
            .state()
            .analyzer()
            .rules
            .iter()
            .any(|rule| rule.name() == "table_scan_rewrite");
        assert_eq!(has_rewrite, expect_rewrite, "{range:?}");
    }
}
