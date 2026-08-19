//! Offline (no live DB) plan-shape tests for `OwnershipRewrite` (#1370, AbAC Stage 2): asserts on
//! the *optimized* `LogicalPlan` text only -- no query execution, no seeded row data (contrast
//! `ownership_rewrite_db_test.rs`). Planning through `into_optimized_plan()` (rather than stopping
//! at the analyzed-but-unoptimized plan) runs `DecorrelatePredicateSubquery`, the optimizer rule
//! that turns `OwnershipRewrite`'s injected `InSubquery`/`Exists` into a join -- and that join is
//! what previously surfaced an ambiguous-column error for an unqualified outer `process_id`
//! reference (#1370 issue 1), which analysis alone never caught. Covers the public-view-set
//! allowlist (§7) plus two fail-closed guards that would otherwise ship with no coverage: an
//! unhandled view set (§7's fallback) and an empty `ReadScope::Audiences(Arc::from([]))` (§3's
//! empty-audience-set short-circuit).
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
/// process_id-**column** view, used below as the "public view set"), [`NoBranchView`] (the
/// "matches no branch" view set), and the `async_events`/`thread_spans` view sets (§5/§6 -- reached
/// only via `view_instance(...)`, never as a global table, so they are registered with
/// `add_view_set` rather than as a global view, mirroring `view_factory.rs::default_view_factory`).
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
    let mut factory = ViewFactory::new(vec![
        processes_view,
        streams_view,
        blocks_view.clone(),
        no_branch_view,
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
/// `DecorrelatePredicateSubquery`'s join surfaces (#1370 issue 1: an unqualified outer
/// `process_id` reference).
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
        unstamped_audience: None,
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
async fn non_public_process_id_column_view_plans_with_an_injected_semi_join() {
    let config = IsolationConfig::default();
    let plan = optimized_plan(scope(&["user:a"]), config, "SELECT * FROM streams")
        .await
        .expect("a non-public process_id-column view must plan");
    let plan_text = format!("{plan}");
    assert!(
        plan_text.contains("LeftSemi Join")
            && plan_text.contains("CAST(__streams__partitions.process_id AS Utf8)"),
        "a non-public process_id-column view must plan with `DecorrelatePredicateSubquery` \
         turning the injected `IN (subquery)` into a `LeftSemi Join` on the outer scan's own \
         qualified, cast `process_id` column, got:\n{plan_text}"
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
        "expected the named §7 fallback error, got: {msg}"
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
        "an empty ReadScope::Audiences must plan a lit(false) predicate over the \
         per-process-audience subquery; the optimizer folds that constant-false filter, plus the \
         resulting empty `LeftSemi Join` right side, all the way down to an `EmptyRelation` -- not \
         an unfiltered scan, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn processes_own_scan_plans_with_an_injected_semi_join() {
    // §3: `processes`'s own scan uses the same `process_id IN (subquery)` construction as §4,
    // filtered against the shared `per_process_audience` aggregate built from
    // `__processes__partitions` -- not an unfiltered scan of the audience source itself.
    let config = IsolationConfig::default();
    let plan = optimized_plan(scope(&["user:a"]), config, "SELECT * FROM processes")
        .await
        .expect("processes' own scan must plan");
    let plan_text = format!("{plan}");
    assert!(
        plan_text.contains("LeftSemi Join")
            && plan_text
                .contains("__processes__partitions.process_id = __correlated_sq_1.process_id"),
        "processes' own scan (§3) must plan with `DecorrelatePredicateSubquery` turning the \
         injected `IN (subquery)` into a self-referential `LeftSemi Join` on the outer scan's own \
         qualified `process_id` column, not an unfiltered scan, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn async_events_view_instance_plans_with_an_injected_exists() {
    // §5: `async_events` is process-scoped but has no `process_id` column to join on -- the
    // predicate is a literal-valued `EXISTS`, keyed on `get_view_instance_id()` (parsed as the
    // process_id UUID). A syntactically valid UUID literal is enough for a plan-shape-only test --
    // no data is scanned.
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
        plan_text.contains("LeftSemi Join")
            && plan_text.contains(&format!(
                "Filter: __processes__partitions.process_id = Utf8(\"{process_id}\")"
            )),
        "async_events' view_instance scan (§5) must plan with `DecorrelatePredicateSubquery` \
         turning the injected literal-valued `EXISTS` into a `LeftSemi Join` against the process \
         named by its view_instance_id, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn thread_spans_view_instance_plans_with_an_injected_two_hop_exists() {
    // §6: `thread_spans` is stream-scoped with no `process_id`/`stream_id` column -- the predicate
    // is a literal `EXISTS` built from a two-hop `streams` -> `per_process_audience` join, keyed on
    // `get_view_instance_id()` (parsed as the stream_id UUID).
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
        plan_text.contains("LeftSemi Join")
            && plan_text.contains(
                "Inner Join: __streams__partitions.process_id = __processes__partitions.process_id"
            )
            && plan_text.contains(&format!(
                "Filter: __streams__partitions.stream_id = Utf8(\"{stream_id}\")"
            )),
        "thread_spans' view_instance scan (§6) must plan with `DecorrelatePredicateSubquery` \
         turning the injected two-hop `EXISTS` into a `LeftSemi Join` whose right side inner-joins \
         `streams` (resolved by its view_instance_id) to `per_process_audience`, got:\n{plan_text}"
    );
}

#[tokio::test]
async fn real_view_factory_covers_every_registered_view_set() {
    // Regression coverage for #1370 review issue 3: every test above plans against this file's
    // own synthetic `make_test_view_factory`, so nothing actually exercises `predicate_for`'s
    // branch table against `default_view_factory()` -- the real, production view-set inventory.
    // A future view set registered there with no matching branch in `ownership_rewrite.rs` would
    // compile and pass CI cleanly today, then fail every restricted-caller query in production
    // with the §7 fallback `DataFusionError::Plan`. This test *enumerates* `default_view_factory`'s
    // actual registrations via the public `get_global_views()`/`get_view_sets()` accessors --
    // rather than hardcoding a parallel list that could silently drift out of sync -- through both
    // the global and `view_instance(...)` access paths, where a given view set offers both -- and
    // asserts each one plans successfully with an injected audience filter, not an error and not
    // an unfiltered scan. (Every branch's injected `InSubquery`/`Exists` gets turned into a
    // `LeftSemi Join` by `DecorrelatePredicateSubquery`, per the per-branch tests above, so a
    // single shared assertion suffices here.) A view set added tomorrow with no branch in
    // `OwnershipRewrite::predicate_for` will now automatically show up here and fail via the §7
    // fallback, instead of staying green because a hand-maintained list never mentioned it.
    //
    // A syntactically valid UUID literal is enough for a plan-shape-only test -- no data is
    // scanned (same rationale as the §5/§6 tests above).
    let process_id = "00000000-0000-0000-0000-000000000003";
    let stream_id = "00000000-0000-0000-0000-000000000004";

    let lakehouse = make_offline_lakehouse_context().await;
    let inventory_view_factory = Arc::new(
        default_view_factory(lakehouse.runtime().clone(), lakehouse.lake().clone())
            .await
            .expect("default_view_factory"),
    );

    // Global instances (§3/§4), implicitly available with no view_instance(...) call.
    let mut queries: Vec<String> = inventory_view_factory
        .get_global_views()
        .iter()
        .map(|view| format!("SELECT * FROM {}", view.get_view_set_name()))
        .collect();
    // view_instance(...)-reachable view sets, keyed on either process_id or stream_id. Only
    // `thread_spans` is stream-scoped (§6); every other view set here is process-scoped (§3-§5),
    // matching the distinction the per-branch tests above already draw.
    for view_set_name in inventory_view_factory.get_view_sets().keys() {
        let instance_id = if view_set_name.as_str() == "thread_spans" {
            stream_id
        } else {
            process_id
        };
        queries.push(format!(
            "SELECT * FROM view_instance('{view_set_name}', '{instance_id}')"
        ));
    }

    for sql in &queries {
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
        assert!(
            plan_text.contains("LeftSemi Join"),
            "real default_view_factory() query `{sql}` must plan with an injected audience \
             filter (LeftSemi Join after DecorrelatePredicateSubquery), not an unfiltered scan, \
             got:\n{plan_text}"
        );
    }
}
