//! Offline (no live DB) plan-shape tests for `OwnershipRewrite` (#1370, AbAC Stage 2): asserts on
//! the *analyzed* `LogicalPlan` text only -- no query execution, no seeded row data (contrast
//! `ownership_rewrite_db_test.rs`). Covers the public-view-set allowlist (§7) plus two fail-closed
//! guards that would otherwise ship with no coverage: an unhandled view set (§7's fallback) and an
//! empty `ReadScope::Audiences(Arc::from([]))` (§3's empty-audience-set short-circuit).
//!
//! Unlike `lakehouse_admin_gate_test.rs`'s `ViewFactory::new(vec![])`, the `ViewFactory` here
//! registers real `processes`/`streams` global views (Design §2 of
//! `tasks/1370_ownership_rewrite_plan.md` requires them for `OwnershipRewrite` to even be
//! constructed under a restricted `ReadScope`), built the same offline way
//! `read_policy_threading_tests.rs`'s fixture is: `SqlBatchView::new` and `register_table` only
//! *plan* SQL (`ctx.sql(...)`, never `.collect()`ed), so a `connect_lazy` Postgres pool and an
//! in-memory object store are sufficient -- no live DB, matching this file's own offline
//! convention (contrast the DB-backed sibling `ownership_rewrite_db_test.rs`).

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::execution::context::SessionContext;
use datafusion::logical_expr::{Expr, LogicalPlan};
use datafusion::prelude::DataFrame;
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::dataframe_time_bounds::DataFrameTimeBounds;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::{NullPartitionProvider, PartitionCache};
use micromegas_analytics::lakehouse::processes_view::make_processes_view;
use micromegas_analytics::lakehouse::query::make_session_context;
use micromegas_analytics::lakehouse::read_scope::{
    CallerContext, OwnershipRewriteConfig, ReadScope,
};
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::streams_view::make_streams_view;
use micromegas_analytics::lakehouse::view::{PartitionSpec, View};
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

/// Offline `LakehouseContext` -- matches `lakehouse_admin_gate_test.rs`'s /
/// `read_policy_threading_tests.rs`'s harness.
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

/// Never actually invoked by a plan-shape-only test (no `.collect()`, so `get_time_bounds` is
/// never called), but `View::get_time_bounds` still needs a value to return.
#[derive(Debug)]
struct UnusedTimeBounds;

#[async_trait]
impl DataFrameTimeBounds for UnusedTimeBounds {
    async fn get_time_bounds(&self, _df: DataFrame) -> Result<TimeRange> {
        unreachable!("not exercised by plan-shape-only tests")
    }
}

/// A minimal view set matching none of `OwnershipRewrite`'s §3-§7 branches: not named
/// `"processes"`/`"async_events"`/`"thread_spans"`, no `process_id` column, and (by construction
/// of the test) never listed in `public_view_sets`. Exercises §7's fallback: `analyze()` must
/// `Err` rather than silently plan an unfiltered scan.
#[derive(Debug)]
struct NoBranchView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    schema: Arc<Schema>,
}

impl NoBranchView {
    fn new() -> Self {
        Self {
            view_set_name: Arc::new("test_no_branch".to_string()),
            view_instance_id: Arc::new("global".to_string()),
            schema: Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)])),
        }
    }
}

#[async_trait]
impl View for NoBranchView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        _lakehouse: Arc<LakehouseContext>,
        _existing_partitions: Arc<PartitionCache>,
        _insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>> {
        anyhow::bail!("not exercised by plan-shape-only tests")
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![1]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        self.schema.clone()
    }

    async fn jit_update(
        &self,
        _lakehouse: Arc<LakehouseContext>,
        _query_range: Option<TimeRange>,
    ) -> Result<()> {
        Ok(())
    }

    fn make_time_filter(&self, _begin: DateTime<Utc>, _end: DateTime<Utc>) -> Result<Vec<Expr>> {
        Ok(vec![])
    }

    fn get_time_bounds(&self) -> Arc<dyn DataFrameTimeBounds> {
        Arc::new(UnusedTimeBounds)
    }

    fn get_update_group(&self) -> Option<i32> {
        None
    }
}

/// Builds a `ViewFactory` registering real `processes`/`streams` (required by Design §2 for
/// `OwnershipRewrite` to be constructed at all under a restricted `ReadScope`), `blocks` (a
/// process_id-**column** view, used below as the "public view set"), and [`NoBranchView`] (the
/// "matches no branch" view set).
async fn make_test_view_factory(lakehouse: &LakehouseContext) -> Arc<ViewFactory> {
    let blocks_view = Arc::new(BlocksView::new().expect("BlocksView::new"));
    let processes_view = Arc::new(
        make_processes_view(
            lakehouse.runtime().clone(),
            lakehouse.lake().clone(),
            Arc::new(ViewFactory::new(vec![blocks_view.clone()])),
        )
        .await
        .expect("make_processes_view"),
    );
    let streams_view = Arc::new(
        make_streams_view(
            lakehouse.runtime().clone(),
            lakehouse.lake().clone(),
            Arc::new(ViewFactory::new(vec![blocks_view.clone()])),
        )
        .await
        .expect("make_streams_view"),
    );
    let no_branch_view: Arc<dyn View> = Arc::new(NoBranchView::new());
    Arc::new(ViewFactory::new(vec![
        processes_view,
        streams_view,
        blocks_view,
        no_branch_view,
    ]))
}

/// Builds a session under `read_scope`/`ownership_config`, plans `sql`, and returns the
/// **analyzed** (not optimizer-passed) `LogicalPlan` -- i.e. exactly what `OwnershipRewrite`
/// produced, before `DecorrelatePredicateSubquery` (an optimizer, not analyzer, rule) rewrites the
/// injected `InSubquery`/`Exists` into a join.
async fn analyzed_plan(
    read_scope: ReadScope,
    ownership_config: OwnershipRewriteConfig,
    sql: &str,
) -> datafusion::error::Result<LogicalPlan> {
    let lakehouse = make_offline_lakehouse_context().await;
    let view_factory = make_test_view_factory(&lakehouse).await;
    let caller = CallerContext {
        read_scope,
        is_admin: false,
        ownership_config: Arc::new(ownership_config),
    };
    let ctx: SessionContext = make_session_context(
        lakehouse,
        Arc::new(NullPartitionProvider {}),
        None,
        view_factory,
        Arc::new(NoOpSessionConfigurator),
        caller,
    )
    .await
    .expect("make_session_context");
    let unoptimized = ctx.sql(sql).await?.into_unoptimized_plan();
    let state = ctx.state();
    state
        .analyzer()
        .execute_and_check(unoptimized, state.config_options(), |_, _| {})
}

fn scope(audiences: &[&str]) -> ReadScope {
    ReadScope::Audiences(
        audiences
            .iter()
            .map(|a| a.to_string())
            .collect::<Vec<_>>()
            .into(),
    )
}

#[tokio::test]
async fn public_view_set_plans_with_no_injected_predicate() {
    let config = OwnershipRewriteConfig {
        unstamped_audience: None,
        public_view_sets: vec!["blocks".to_string()],
    };
    let plan = analyzed_plan(scope(&["user:a"]), config, "SELECT * FROM blocks")
        .await
        .expect("a public view set must plan without error");
    let plan_text = format!("{plan}");
    assert!(
        !plan_text.contains("Filter"),
        "a public view set must plan with no injected Filter node, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn non_public_process_id_column_view_plans_with_an_injected_semi_join() {
    let config = OwnershipRewriteConfig::default();
    let plan = analyzed_plan(scope(&["user:a"]), config, "SELECT * FROM streams")
        .await
        .expect("a non-public process_id-column view must plan");
    let plan_text = format!("{plan}");
    assert!(
        plan_text.contains("Filter") && plan_text.contains("IN"),
        "a non-public process_id-column view must plan with an injected `IN (subquery)` Filter, \
         got:\n{plan_text}"
    );
}

#[tokio::test]
async fn unhandled_view_set_fails_analysis_loudly() {
    let config = OwnershipRewriteConfig::default();
    let err = analyzed_plan(scope(&["user:a"]), config, "SELECT * FROM test_no_branch")
        .await
        .expect_err("a view set matching no branch must fail analysis, not plan unfiltered");
    let msg = err.to_string();
    assert!(
        msg.contains("OwnershipRewrite: no audience rule defined for view set 'test_no_branch'"),
        "expected the named §7 fallback error, got: {msg}"
    );
}

#[tokio::test]
async fn empty_audience_set_plans_a_literal_false_predicate() {
    let config = OwnershipRewriteConfig::default();
    let plan = analyzed_plan(
        ReadScope::Audiences(Arc::from([])),
        config,
        "SELECT * FROM streams",
    )
    .await
    .expect("an empty audience set must still plan (fail closed, not fail to plan)");
    let plan_text = format!("{plan}");
    assert!(
        plan_text.contains("Filter: Boolean(false)"),
        "an empty ReadScope::Audiences must plan a lit(false) predicate (rendered as a `Filter: \
         Boolean(false)` node inside the per-process-audience subquery), not an unfiltered scan \
         or a bare IN (), got:\n{plan_text}"
    );
}
