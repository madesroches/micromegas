//! Offline (no live DB) plan-shape tests for `OwnershipRewrite`: asserts on the *optimized*
//! `LogicalPlan` text only -- no query execution, no seeded row data (contrast
//! `ownership_rewrite_db_test.rs`). Planning through `into_optimized_plan()` (rather than stopping
//! at the analyzed-but-unoptimized plan) runs `DecorrelatePredicateSubquery`, the optimizer rule
//! that turns `OwnershipRewrite`'s injected `InSubquery`/`Exists` into a join -- and that join is
//! what can surface an ambiguous-column error for an unqualified outer `process_id` reference,
//! which analysis alone never catches. Covers the public-view-set allowlist plus two fail-closed
//! guards that would otherwise ship with no coverage: an unhandled view set (the fallback) and an
//! empty `ReadScope::Audiences(Arc::from([]))` (the empty-audience-set short-circuit).
//!
//! Unlike `lakehouse_admin_gate_test.rs`'s `ViewFactory::new(vec![])`, the `ViewFactory` here
//! registers real `processes`/`streams` global views (required for `OwnershipRewrite` to even be
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
use micromegas_analytics::lakehouse::async_events_view::AsyncEventsViewMaker;
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::dataframe_time_bounds::DataFrameTimeBounds;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::{NullPartitionProvider, PartitionCache};
use micromegas_analytics::lakehouse::processes_view::make_processes_view;
use micromegas_analytics::lakehouse::query::make_session_context;
use micromegas_analytics::lakehouse::read_scope::{CallerContext, IsolationConfig, ReadScope};
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::streams_view::make_streams_view;
use micromegas_analytics::lakehouse::thread_spans_view::ThreadSpansViewMaker;
use micromegas_analytics::lakehouse::view::{PartitionSpec, View};
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, default_view_factory};
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
    Arc::new(LakehouseContext::new(lake, runtime).expect("LakehouseContext::new"))
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

/// A minimal view set matching none of `OwnershipRewrite`'s branches: not named
/// `"processes"`/`"async_events"`/`"thread_spans"`, no `process_id` column, and (by construction
/// of the test) never listed in `public_view_sets`. Exercises the fallback: `analyze()` must
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

/// A minimal view carrying a `process_id` column but no `audience` column of its own -- stands
/// in for `net_spans`/`otel_spans`/`images` (those three still take the semi-join branch, since
/// they haven't gained the physical column). Otherwise identical in spirit to [`NoBranchView`].
#[derive(Debug)]
struct ProcessIdOnlyView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    schema: Arc<Schema>,
}

impl ProcessIdOnlyView {
    fn new() -> Self {
        Self {
            view_set_name: Arc::new("test_process_id_only".to_string()),
            view_instance_id: Arc::new("global".to_string()),
            schema: Arc::new(Schema::new(vec![Field::new(
                "process_id",
                DataType::Utf8,
                false,
            )])),
        }
    }
}

#[async_trait]
impl View for ProcessIdOnlyView {
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

/// Builds a `ViewFactory` registering real `processes`/`streams` (required for `OwnershipRewrite`
/// to be constructed at all under a restricted `ReadScope`), `blocks` (a process_id-**column**
/// view, used below as the "public view set"), [`NoBranchView`] (the "matches no branch" view
/// set), and the `async_events`/`thread_spans` view sets (reached only via `view_instance(...)`,
/// never as a global table, so they are registered with
/// `add_view_set` rather than as a global view, mirroring `view_factory.rs::default_view_factory`).
async fn make_test_view_factory(lakehouse: &LakehouseContext) -> Arc<ViewFactory> {
    let blocks_view =
        Arc::new(BlocksView::new(lakehouse.default_audience()).expect("BlocksView::new"));
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
    let process_id_only_view: Arc<dyn View> = Arc::new(ProcessIdOnlyView::new());
    let mut factory = ViewFactory::new(vec![
        processes_view,
        streams_view,
        blocks_view.clone(),
        no_branch_view,
        process_id_only_view,
    ]);
    // `AsyncEventsViewMaker`/`ThreadSpansViewMaker` only consult the `ViewFactory` they're given
    // from `jit_update` (materialization -- never reached by these plan-shape-only tests, which
    // never `.collect()`), so a minimal `blocks`-only factory is enough here.
    let jit_factory = Arc::new(ViewFactory::new(vec![blocks_view]));
    factory.add_view_set(
        String::from("async_events"),
        Arc::new(AsyncEventsViewMaker::new(jit_factory.clone())),
    );
    factory.add_view_set(
        String::from("thread_spans"),
        Arc::new(ThreadSpansViewMaker::new(jit_factory)),
    );
    Arc::new(factory)
}

/// Builds a session under `read_scope`/`isolation_config` against the given `view_factory`, plans
/// `sql`, and returns the **optimized** `LogicalPlan` -- i.e. what actually executes, after
/// `DecorrelatePredicateSubquery` (an optimizer, not analyzer, rule) has rewritten
/// `OwnershipRewrite`'s injected `InSubquery`/`Exists` into a join. Stopping at the
/// analyzed-but-unoptimized plan would miss ambiguous-column errors that only
/// `DecorrelatePredicateSubquery`'s join surfaces (an unqualified outer `process_id` reference).
async fn optimized_plan_with_factory(
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    read_scope: ReadScope,
    isolation_config: IsolationConfig,
    sql: &str,
) -> datafusion::error::Result<LogicalPlan> {
    let caller = CallerContext {
        read_scope,
        is_admin: false,
        isolation_config: Arc::new(isolation_config),
        admin_principal_possible: true,
        identity: None,
        grant_selectors: Arc::from([]),
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
    ctx.sql(sql).await?.into_optimized_plan()
}

/// Thin wrapper over `optimized_plan_with_factory` against this file's synthetic
/// `make_test_view_factory`.
async fn optimized_plan(
    read_scope: ReadScope,
    isolation_config: IsolationConfig,
    sql: &str,
) -> datafusion::error::Result<LogicalPlan> {
    let lakehouse = make_offline_lakehouse_context().await;
    let view_factory = make_test_view_factory(&lakehouse).await;
    optimized_plan_with_factory(lakehouse, view_factory, read_scope, isolation_config, sql).await
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
    let config = IsolationConfig {
        public_view_sets: vec!["blocks".to_string()],
    };
    let plan = optimized_plan(scope(&["user:a"]), config, "SELECT * FROM blocks")
        .await
        .expect("a public view set must plan without error");
    let plan_text = format!("{plan}");
    assert!(
        !plan_text.contains("Filter"),
        "a public view set must plan with no injected Filter node, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn non_public_process_id_only_view_plans_with_an_injected_semi_join() {
    // A view that carries a `process_id` column but no `audience` column of its own (e.g.
    // net_spans/otel_spans/images in the real system, `test_process_id_only` here) still goes
    // through the semi-join. `streams`/`processes`/`blocks` all carry `audience` directly and
    // take the direct-filter branch instead -- see the tests below.
    let config = IsolationConfig::default();
    let plan = optimized_plan(
        scope(&["user:a"]),
        config,
        "SELECT * FROM test_process_id_only",
    )
    .await
    .expect("a process_id-only view must plan");
    let plan_text = format!("{plan}");
    assert!(
        plan_text.contains("LeftSemi Join")
            && plan_text.contains("CAST(test_process_id_only.process_id AS Utf8)"),
        "a process_id-only view (no `audience` column) must plan with \
         `DecorrelatePredicateSubquery` turning the injected `IN (subquery)` into a `LeftSemi \
         Join` on the outer scan's own qualified, cast `process_id` column, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn streams_plans_with_a_direct_audience_filter_no_join() {
    // `streams` carries a physical `audience` column, so it is filtered directly -- no
    // semi-join, no `property_get`, no per-process aggregate.
    let config = IsolationConfig::default();
    let plan = optimized_plan(scope(&["user:a"]), config, "SELECT * FROM streams")
        .await
        .expect("streams must plan");
    let plan_text = format!("{plan}");
    assert!(
        plan_text.contains("Filter")
            && plan_text.contains("audience")
            && !plan_text.contains("LeftSemi Join")
            && !plan_text.contains("property_get"),
        "streams must plan with a bare Filter on its own `audience` column, no join and no \
         property_get, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn unhandled_view_set_fails_analysis_loudly() {
    let config = IsolationConfig::default();
    let err = optimized_plan(scope(&["user:a"]), config, "SELECT * FROM test_no_branch")
        .await
        .expect_err("a view set matching no branch must fail analysis, not plan unfiltered");
    let msg = err.to_string();
    assert!(
        msg.contains("OwnershipRewrite: no audience rule defined for view set 'test_no_branch'"),
        "expected the named fallback error, got: {msg}"
    );
}

#[tokio::test]
async fn empty_audience_set_plans_a_literal_false_predicate() {
    let config = IsolationConfig::default();
    let plan = optimized_plan(
        ReadScope::Audiences(Arc::from([])),
        config,
        "SELECT * FROM streams",
    )
    .await
    .expect("an empty audience set must still plan (fail closed, not fail to plan)");
    let plan_text = format!("{plan}");
    assert!(
        plan_text.contains("EmptyRelation"),
        "an empty ReadScope::Audiences must plan a lit(false) predicate directly on `streams`' \
         own `audience` column; the optimizer folds that constant-false filter all the way down \
         to an `EmptyRelation` -- not an unfiltered scan, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn processes_own_scan_plans_with_a_direct_audience_filter_no_join() {
    // `processes` -- the audience source itself -- carries the physical `audience` column too,
    // and is a member of the column-carrying branch rather than a special case (there is no
    // self-referential semi-join).
    let config = IsolationConfig::default();
    let plan = optimized_plan(scope(&["user:a"]), config, "SELECT * FROM processes")
        .await
        .expect("processes' own scan must plan");
    let plan_text = format!("{plan}");
    assert!(
        plan_text.contains("Filter")
            && plan_text.contains("audience")
            && !plan_text.contains("LeftSemi Join")
            && !plan_text.contains("property_get"),
        "processes' own scan must plan with a bare Filter on its own `audience` column, no \
         self-referential join and no property_get, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn async_events_view_instance_plans_with_no_injected_predicate() {
    // `async_events` is reachable only through a guarded, non-`global` `view_instance(...)` call
    // (never as a global table -- see `make_test_view_factory`'s doc comment), so
    // `AudienceGuard::authorize_view_instance` already denies an unauthorized caller before any
    // row is read and `OwnershipRewrite::predicate_for` skips its literal-valued `EXISTS`
    // entirely (`MaterializedView::instance_is_audience_guarded`). A syntactically valid UUID
    // literal is enough for a plan-shape-only test -- no data is scanned.
    let config = IsolationConfig::default();
    let process_id = "00000000-0000-0000-0000-000000000001";
    let plan = optimized_plan(
        scope(&["user:a"]),
        config,
        &format!("SELECT * FROM view_instance('async_events', '{process_id}')"),
    )
    .await
    .expect("async_events' view_instance scan must plan");
    let plan_text = format!("{plan}");
    assert!(
        !plan_text.contains("Filter") && !plan_text.contains("LeftSemi Join"),
        "async_events' guarded view_instance scan must plan with no injected predicate at all -- \
         the call-level guard's instance check is the only enforcement here, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn thread_spans_view_instance_plans_with_no_injected_predicate() {
    // `thread_spans` is reachable only through a guarded, non-`global` `view_instance(...)` call
    // (never as a global table), so the same skip as `async_events` above applies: the
    // call-level guard's instance check already denies an unauthorized caller before any row is read, so
    // `OwnershipRewrite::predicate_for` skips its two-hop `EXISTS` entirely.
    let config = IsolationConfig::default();
    let stream_id = "00000000-0000-0000-0000-000000000002";
    let plan = optimized_plan(
        scope(&["user:a"]),
        config,
        &format!("SELECT * FROM view_instance('thread_spans', '{stream_id}')"),
    )
    .await
    .expect("thread_spans' view_instance scan must plan");
    let plan_text = format!("{plan}");
    assert!(
        !plan_text.contains("Filter") && !plan_text.contains("LeftSemi Join"),
        "thread_spans' guarded view_instance scan must plan with no injected predicate at all -- \
         the call-level guard's instance check is the only enforcement here, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn guarded_net_spans_view_instance_plans_with_no_injected_predicate() {
    // Regression coverage against a **real** view set, not a synthetic mirror:
    // `default_view_factory` already registers `net_spans` via `add_view_set`, so
    // `view_instance('net_spans', '<uuid>')` is plannable with no new fixture. This, plus
    // `non_public_process_id_only_view_plans_with_an_injected_semi_join`'s **global**
    // `ProcessIdOnlyView` above, is what proves the skip is keyed on the query path (guarded
    // instance vs. not), not on the view's schema: same `process_id`-only shape, opposite plan.
    let config = IsolationConfig::default();
    let lakehouse = make_offline_lakehouse_context().await;
    let view_factory = Arc::new(
        default_view_factory(
            lakehouse.runtime().clone(),
            lakehouse.lake().clone(),
            lakehouse.default_audience(),
        )
        .await
        .expect("default_view_factory"),
    );
    let process_id = "00000000-0000-0000-0000-000000000005";
    let plan = optimized_plan_with_factory(
        lakehouse,
        view_factory,
        scope(&["user:a"]),
        config,
        &format!("SELECT * FROM view_instance('net_spans', '{process_id}')"),
    )
    .await
    .expect("net_spans' view_instance scan must plan");
    let plan_text = format!("{plan}");
    assert!(
        !plan_text.contains("Filter") && !plan_text.contains("LeftSemi Join"),
        "net_spans' guarded view_instance scan must plan with no injected predicate at all -- \
         the call-level guard's instance check is the only enforcement here, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn real_view_factory_covers_every_registered_view_set() {
    // Every test above plans against this file's own synthetic `make_test_view_factory`, so
    // nothing else exercises `predicate_for`'s branch table against `default_view_factory()` --
    // the real, production view-set inventory. A future view set registered there with no
    // matching branch in `ownership_rewrite.rs` would compile and pass CI cleanly today, then
    // fail every restricted-caller query in production with the fallback `DataFusionError::Plan`.
    // This test *enumerates* `default_view_factory`'s actual registrations via the public
    // `get_global_views()`/`get_view_sets()` accessors -- rather than hardcoding a parallel list
    // that could silently drift out of sync -- through both the global and `view_instance(...)`
    // access paths, where a given view set offers both -- and asserts each one plans
    // successfully, not with an error. What it must plan *with* is a two-way split, keyed on
    // whether the view carries a physical `audience` column, not on which access path was used: a
    // view with the column plans a bare `Filter` on it (whether reached globally or through a
    // guarded `view_instance(...)` -- `log_entries`/`measures` are registered both ways and must
    // plan the same `Filter` shape either way); a view with no `audience` column, reached only
    // through a guarded `view_instance(...)` in `default_view_factory`, plans with no injected
    // predicate at all -- the call-level guard's instance check is already the sole enforcement
    // for it. A view set added tomorrow with no branch in `OwnershipRewrite::predicate_for` will
    // automatically show up here and fail via the fallback, instead of staying green because a
    // hand-maintained list never mentioned it.
    //
    // A syntactically valid UUID literal is enough for a plan-shape-only test -- no data is
    // scanned (same rationale as the per-branch tests above).
    let process_id = "00000000-0000-0000-0000-000000000003";
    let stream_id = "00000000-0000-0000-0000-000000000004";

    let lakehouse = make_offline_lakehouse_context().await;
    let inventory_view_factory = Arc::new(
        default_view_factory(
            lakehouse.runtime().clone(),
            lakehouse.lake().clone(),
            lakehouse.default_audience(),
        )
        .await
        .expect("default_view_factory"),
    );

    // Global instances, implicitly available with no view_instance(...) call. Keyed on whether
    // the view's own file schema carries `audience` -- the same schema introspection
    // `OwnershipRewrite::predicate_for` itself uses -- to know which shape each query must plan
    // with.
    let mut queries: Vec<(String, bool)> = inventory_view_factory
        .get_global_views()
        .iter()
        .map(|view| {
            let has_audience = view.get_file_schema().field_with_name("audience").is_ok();
            (
                format!("SELECT * FROM {}", view.get_view_set_name()),
                has_audience,
            )
        })
        .collect();
    // view_instance(...)-reachable view sets, keyed on either process_id or stream_id. Only
    // `thread_spans` is stream-scoped (§thread_spans); every other view set here is
    // process-scoped, matching the distinction the per-branch tests above already draw.
    for (view_set_name, maker) in inventory_view_factory.get_view_sets() {
        let instance_id = if view_set_name.as_str() == "thread_spans" {
            stream_id
        } else {
            process_id
        };
        let has_audience = maker.get_schema().field_with_name("audience").is_ok();
        queries.push((
            format!("SELECT * FROM view_instance('{view_set_name}', '{instance_id}')"),
            has_audience,
        ));
    }

    for (sql, has_audience) in &queries {
        let config = IsolationConfig::default();
        let plan = optimized_plan_with_factory(
            lakehouse.clone(),
            inventory_view_factory.clone(),
            scope(&["user:a"]),
            config,
            sql,
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "real default_view_factory() query `{sql}` must plan under a restricted \
                     ReadScope -- if this fails, a view set is missing a branch in \
                     OwnershipRewrite::predicate_for; got error: {e}"
            )
        });
        let plan_text = format!("{plan}");
        if *has_audience {
            // A view carrying the physical `audience` column is filtered directly -- no join, no
            // property_get. This is the regression test for the optimization itself.
            assert!(
                plan_text.contains("Filter")
                    && plan_text.contains("audience")
                    && !plan_text.contains("LeftSemi Join")
                    && !plan_text.contains("property_get"),
                "real default_view_factory() query `{sql}` carries an `audience` column and must \
                 plan with a bare Filter on it, no join, got:\n{plan_text}"
            );
        } else {
            // Every view set reaching this arm (`net_spans`, `otel_spans`, `images`,
            // `async_events`, `thread_spans`) is registered only via `add_view_set`, so the only
            // query above that reaches it is a guarded, non-`global` `view_instance(...)` scan --
            // `MaterializedView::instance_is_audience_guarded()` is true, and
            // `OwnershipRewrite::predicate_for` skips its predicate entirely.
            assert!(
                !plan_text.contains("Filter") && !plan_text.contains("LeftSemi Join"),
                "real default_view_factory() query `{sql}` has no `audience` column and is a \
                 guarded view_instance(...) scan; it must plan with no injected predicate at \
                 all, got:\n{plan_text}"
            );
        }
    }
}
