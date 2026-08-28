//! Offline (no live DB) tests for Query Enforcement Prong B (#1371, AbAC Stage 3):
//! `is_readable`'s truth table and `AudienceGuard`'s no-I/O short-circuit under `ReadScope::All`.
//!
//! `IsolationConfig::from_env`'s knob is covered by `ownership_rewrite_config_tests.rs`;
//! the mutating-function registration gate's states (driven by
//! `CallerContext::admin_principal_possible`) are covered by `lakehouse_admin_gate_test.rs`. Both
//! predate this file and already exercise the pieces this stage added to them.
//!
//! `OwnerAudience::Unstamped` is gone: a row with a NULL `audience` column resolves to
//! `MICROMEGAS_DEFAULT_AUDIENCE` in `owner_query_sql`'s `COALESCE`, so it is an ordinary
//! `Audience(..)` rather than a state to special-case, and a resolved `None` -- now reachable only
//! for an id with no row at all -- maps to `OwnerAudience::Unknown` in `merge_owner_rows`, always
//! denied under `ReadScope::Audiences`. That mapping itself is private and DB-backed
//! (`fetch_owner_rows`/`merge_owner_rows`); see `prong_b_guard_db_test.rs` for end-to-end coverage
//! against a real row with a NULL `audience` column. What's covered here is the pure half:
//! `is_readable` already denies `Unknown` unconditionally under `ReadScope::Audiences`.
//!
//! `owner_query_sql` moved from property/`unnest`-based SQL to per-row `audience`-column point
//! queries, pinned below via the `#[doc(hidden)]` `owner_query_sql_for_test` accessor.
//!
//! Also covers, offline, that the #1486 `view_instance(...)` guard is actually wired into
//! `MaterializedView::scan` (not just correct in isolation, which the rest of this file already
//! covers): every test that proves the hook fires end-to-end
//! (`prong_b_guard_db_test.rs`/`ownership_rewrite_db_test.rs`) is `#[ignore]`d and DB-backed, so
//! CI's plain `cargo test` would not catch a regression that dropped the guard call out of
//! `scan` or stopped passing it into `MaterializedView::new`.
//! `materialized_view_scan_denies_before_jit_update_for_foreign_audience_instance` below builds
//! a real `MaterializedView` over a stub `View` whose `jit_update` must never run, and asserts
//! `scan` denies before reaching it.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::logical_expr::Expr;
use datafusion::prelude::{DataFrame, SessionContext};
use micromegas_analytics::lakehouse::audience_guard::{
    AudienceGuard, AudienceIndex, IdKind, OwnerAudience, is_readable, owner_query_sql_for_test,
};
use micromegas_analytics::lakehouse::dataframe_time_bounds::DataFrameTimeBounds;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::materialized_view::MaterializedView;
use micromegas_analytics::lakehouse::partition_cache::NullPartitionProvider;
use micromegas_analytics::lakehouse::read_scope::ReadScope;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::view::{PartitionSpec, View};
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use uuid::Uuid;

/// An `AudienceIndex` over a `connect_lazy` pool to an unroutable address -- `connect_lazy` never
/// touches the network at construction time (same trick `lakehouse_admin_gate_test.rs` uses for
/// `LakehouseContext`), so a test built from this index can assert "no I/O happened" simply by
/// not hanging/erroring on a query that would otherwise need a real connection. An explicit short
/// `acquire_timeout` keeps the resolution-error tests fast -- sqlx's default is 30s (see
/// `rust/auth/tests/db_api_key_tests.rs` and `rust/ingestion/tests/process_audience_cache_test.rs`
/// for the same pattern).
fn unroutable_index() -> Arc<AudienceIndex> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://user:pass@127.0.0.1:1/db")
        .expect("connect_lazy should not touch the network");
    Arc::new(AudienceIndex::new(
        pool,
        100_000,
        Duration::from_secs(300),
        Arc::from("public"),
    ))
}

fn audiences(names: &[&str]) -> ReadScope {
    ReadScope::Audiences(
        names
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into(),
    )
}

fn audience(name: &str) -> OwnerAudience {
    OwnerAudience::Audience(Arc::from(name))
}

// --- `is_readable` truth table -----------------------------------------------------------

#[test]
fn all_passes_everything_including_unknown() {
    assert!(is_readable(&ReadScope::All, &OwnerAudience::Unknown));
    assert!(is_readable(&ReadScope::All, &audience("team-alpha")));
}

#[test]
fn audiences_denies_unknown_always() {
    // `Unknown` now covers both "no such row" and (post-backfill) "a row that violates the
    // stamp-at-write-time invariant" -- either way, always denied under a restricted scope.
    let scope = audiences(&["team-alpha"]);
    assert!(!is_readable(&scope, &OwnerAudience::Unknown));
}

#[test]
fn audiences_audience_matches_byte_exactly() {
    let scope = audiences(&["Team-Alpha"]);
    assert!(
        !is_readable(&scope, &audience("team-alpha")),
        "case must matter: 'Team-Alpha' != 'team-alpha'"
    );
    assert!(is_readable(&scope, &audience("Team-Alpha")));
}

#[test]
fn audiences_ambiguous_denies_unless_every_owner_is_readable() {
    let scope = audiences(&["team-alpha"]);
    assert!(
        !is_readable(
            &scope,
            &OwnerAudience::Ambiguous(vec![audience("team-alpha"), audience("team-beta")])
        ),
        "one unreadable owner among the collision's arms -> deny"
    );
    assert!(
        is_readable(
            &scope,
            &OwnerAudience::Ambiguous(vec![audience("team-alpha"), audience("team-alpha")])
        ),
        "every arm independently readable -> allow"
    );
    assert!(
        !is_readable(
            &scope,
            &OwnerAudience::Ambiguous(vec![audience("team-beta"), audience("team-beta")])
        ),
        "no arm readable -> deny"
    );
    assert!(
        !is_readable(&scope, &OwnerAudience::Ambiguous(vec![])),
        "an empty owner set must not be vacuously readable"
    );
}

#[test]
fn empty_audience_set_denies_everything() {
    let scope = audiences(&[]);
    assert!(!is_readable(&scope, &OwnerAudience::Unknown));
    assert!(!is_readable(&scope, &audience("public")));
}

// --- `AudienceGuard` no-I/O short-circuit under `ReadScope::All` -------------------------

#[tokio::test]
async fn authorize_under_read_scope_all_performs_no_io() {
    let guard = AudienceGuard::new(ReadScope::All, false, vec![], unroutable_index());
    let id = Uuid::new_v4();
    let authorized = guard
        .authorize(id, IdKind::Process, "test_fn")
        .await
        .expect("ReadScope::All must authorize with no I/O, even over an unroutable pool");
    assert_eq!(authorized.id(), id);
}

#[tokio::test]
async fn readable_ids_under_read_scope_all_performs_no_io() {
    let guard = AudienceGuard::new(ReadScope::All, false, vec![], unroutable_index());
    let ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    let readable = guard
        .readable_ids(&ids, IdKind::ProcessOrStream)
        .await
        .expect("ReadScope::All must resolve every id as readable with no I/O");
    for id in &ids {
        assert!(readable.contains(id));
    }
}

#[tokio::test]
async fn authorize_under_restricted_scope_denies_on_resolution_error_not_pass() {
    // No `ReadScope::All` short-circuit here, so this one *does* try to reach the unroutable
    // pool -- the resolution error it gets back must deny, never authorize.
    let guard = AudienceGuard::new(
        audiences(&["team-alpha"]),
        false,
        vec![],
        unroutable_index(),
    );
    let err = guard
        .authorize(Uuid::new_v4(), IdKind::Process, "test_fn")
        .await
        .expect_err("a resolution error must be a denial, never a readable verdict");
    // Whatever the underlying I/O failure looks like, it must not read as the caller-visible
    // existence-oracle-proof denial text either -- assert only that it *is* an error, per the
    // module's fail-closed contract.
    let _ = err;
}

// --- `global_rows_visible` (list_partitions' 'global'-row rule, #1482 §4) ----------------

// `unroutable_index()` builds a `sqlx::PgPool` via `connect_lazy`, which -- despite never
// touching the network -- requires a Tokio runtime context to construct (it registers pool
// maintenance internals against the ambient runtime). Hence `#[tokio::test]`, even though
// `global_rows_visible` itself is pure and does no I/O.

#[tokio::test]
async fn global_rows_visible_under_all() {
    let guard = AudienceGuard::new(ReadScope::All, false, vec![], unroutable_index());
    assert!(guard.global_rows_visible("log_entries"));
}

#[tokio::test]
async fn global_rows_visible_via_public_view_sets() {
    let guard = AudienceGuard::new(
        audiences(&["team-alpha"]),
        false,
        vec!["log_stats".to_string()],
        unroutable_index(),
    );
    assert!(guard.global_rows_visible("log_stats"));
    assert!(!guard.global_rows_visible("log_entries"));
}

#[tokio::test]
async fn global_rows_visible_via_lakehouse_admin() {
    // #1482 §4: the removed `unstamped_audience`-in-scope disjunct is replaced by the same
    // lakehouse-admin gate that already governs the mutating UDTFs/UDFs -- no new authority, no
    // new knob.
    let admin_guard =
        AudienceGuard::new(audiences(&["team-alpha"]), true, vec![], unroutable_index());
    assert!(admin_guard.global_rows_visible("log_entries"));

    let non_admin_guard = AudienceGuard::new(
        audiences(&["team-alpha"]),
        false,
        vec![],
        unroutable_index(),
    );
    assert!(
        !non_admin_guard.global_rows_visible("log_entries"),
        "a non-admin caller whose view set isn't public must still be hidden from 'global' rows"
    );
}

#[tokio::test]
async fn global_rows_hidden_by_default_under_restricted_scope() {
    let guard = AudienceGuard::new(
        audiences(&["team-alpha"]),
        false,
        vec![],
        unroutable_index(),
    );
    assert!(!guard.global_rows_visible("log_entries"));
}

// --- `authorize_view_instance` (the `view_instance(...)` scan-time guard, #1486) ---------

#[tokio::test]
async fn authorize_view_instance_under_read_scope_all_performs_no_io() {
    let guard = AudienceGuard::new(ReadScope::All, false, vec![], unroutable_index());
    guard
        .authorize_view_instance("thread_spans", &Uuid::new_v4().to_string())
        .await
        .expect("ReadScope::All must authorize with no I/O, even over an unroutable pool");
}

#[tokio::test]
async fn authorize_view_instance_allows_public_view_set_with_no_io() {
    let guard = AudienceGuard::new(
        audiences(&["team-alpha"]),
        false,
        vec!["log_entries".to_string()],
        unroutable_index(),
    );
    guard
        .authorize_view_instance("log_entries", &Uuid::new_v4().to_string())
        .await
        .expect(
            "a view set on public_view_sets must be authorized with no I/O, regardless of the \
             instance id",
        );
}

#[tokio::test]
async fn authorize_view_instance_allows_global_with_no_io_admin_or_not_public_or_not() {
    // Rule (3) deliberately differs from `global_rows_visible`: `'global'` is passed through
    // uncalled and left to Prong A's row filter, for both an admin and a non-admin guard, and
    // for a view set that is *not* on the public allowlist -- pin it, since it's the one rule
    // in this method that isn't just `global_rows_visible` reused.
    let non_admin_non_public = AudienceGuard::new(
        audiences(&["team-alpha"]),
        false,
        vec![],
        unroutable_index(),
    );
    non_admin_non_public
        .authorize_view_instance("log_entries", "global")
        .await
        .expect(
            "'global' must be authorized with no I/O for a non-admin caller over a non-public \
             view set -- there is nothing to protect, jit_update no-ops for 'global'",
        );

    let admin = AudienceGuard::new(audiences(&["team-alpha"]), true, vec![], unroutable_index());
    admin
        .authorize_view_instance("log_entries", "global")
        .await
        .expect("'global' must also be authorized with no I/O for an admin caller");
}

#[tokio::test]
async fn authorize_view_instance_denies_on_resolution_error_not_pass() {
    // No `ReadScope::All`/public-view-set/`'global'` short-circuit here, so this does try to
    // reach the unroutable pool -- mirrors
    // `authorize_under_restricted_scope_denies_on_resolution_error_not_pass`, pinning that rule
    // (4)'s fall-through to `authorize` really attempts resolution and fails closed rather than
    // short-circuiting to a pass.
    let guard = AudienceGuard::new(
        audiences(&["team-alpha"]),
        false,
        vec![],
        unroutable_index(),
    );
    guard
        .authorize_view_instance("thread_spans", &Uuid::new_v4().to_string())
        .await
        .expect_err("a resolution error must be a denial, never a readable verdict");
}

#[tokio::test]
async fn authorize_view_instance_denies_non_uuid_non_global_id() {
    let guard = AudienceGuard::new(
        audiences(&["team-alpha"]),
        false,
        vec![],
        unroutable_index(),
    );
    let err = guard
        .authorize_view_instance("thread_spans", "not-a-uuid-or-global")
        .await
        .expect_err("an id that is neither 'global' nor a valid Uuid must be denied");
    assert!(
        err.to_string().contains("not found or not accessible"),
        "expected the uniform not-found-shaped denial text, got: {err}"
    );
}

// --- `owner_query_sql`: column-based, no join/unnest --------------------------------------

#[test]
fn owner_query_sql_never_mentions_unnest_or_properties() {
    for kind in [IdKind::Process, IdKind::Block, IdKind::ProcessOrStream] {
        let sql = owner_query_sql_for_test(kind);
        assert!(
            !sql.contains("unnest"),
            "{kind:?}: owner_query_sql must read the audience column directly, not unnest a \
             properties array -- got: {sql}"
        );
        assert!(
            !sql.contains("properties"),
            "{kind:?}: owner_query_sql must not reference properties at all -- got: {sql}"
        );
        assert!(
            sql.contains("audience"),
            "{kind:?}: owner_query_sql must reference the audience column -- got: {sql}"
        );
    }
}

#[test]
fn owner_query_sql_block_has_no_join_to_processes() {
    // The one behaviour change worth pinning: IdKind::Block used to join through `processes` to
    // resolve a block's audience; it now reads `blocks.audience` alone, a single-table point
    // query, so a block whose `processes` row is gone (retention swept it, or it hasn't arrived
    // yet) resolves to its own stamp instead of falling through to `Unknown`.
    let sql = owner_query_sql_for_test(IdKind::Block);
    assert!(
        !sql.to_lowercase().contains("join"),
        "IdKind::Block must be a single-table query with no join -- got: {sql}"
    );
    assert!(sql.contains("FROM blocks"));
}

// --- `MaterializedView::scan` enforcement wiring (offline) --------------------------------

/// Offline `LakehouseContext` -- matches `lakehouse_admin_gate_test.rs`'s /
/// `ownership_rewrite_public_view_set_tests.rs`'s harness: a `connect_lazy` Postgres pool (never
/// touches the network at construction time) plus an in-memory object store, so `jit_update`
/// would be the only thing in this test able to reach real I/O -- and it must never run.
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

/// Never actually invoked -- `get_time_bounds()` is only called on a `.collect()`-then-limit
/// path this test never reaches (the guard denies before `jit_update`, and the scan never runs
/// to completion).
#[derive(Debug)]
struct UnusedTimeBounds;

#[async_trait]
impl DataFrameTimeBounds for UnusedTimeBounds {
    async fn get_time_bounds(&self, _df: DataFrame) -> Result<TimeRange> {
        unreachable!("not exercised: the #1486 guard must deny before this view is ever scanned")
    }
}

/// A minimal `View` standing in for a real caller-named `view_instance(...)` target.
/// `jit_update` flips `jit_update_called` and then fails -- proof, if `MaterializedView::scan`
/// ever reaches it, that the #1486 guard check was skipped or misordered.
#[derive(Debug)]
struct JitUpdateMustNotRunView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    schema: Arc<Schema>,
    jit_update_called: Arc<AtomicBool>,
}

impl JitUpdateMustNotRunView {
    /// `view_instance_id` is caller-supplied so the same stub serves both a `Uuid` instance id
    /// (`AudienceGuard::authorize_view_instance` takes its resolution branch rather than one of
    /// its no-I/O short-circuits) and the `"global"` instance id (the guard's unconditional-pass
    /// short-circuit).
    fn new(jit_update_called: Arc<AtomicBool>, view_instance_id: &str) -> Self {
        Self {
            view_set_name: Arc::new("test_guarded_view_set".to_string()),
            view_instance_id: Arc::new(view_instance_id.to_string()),
            schema: Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)])),
            jit_update_called,
        }
    }
}

#[async_trait]
impl View for JitUpdateMustNotRunView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        _lakehouse: Arc<LakehouseContext>,
        _existing_partitions: Arc<micromegas_analytics::lakehouse::partition_cache::PartitionCache>,
        _insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>> {
        unreachable!("not exercised: the #1486 guard must deny before this view is ever scanned")
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
        self.jit_update_called.store(true, Ordering::SeqCst);
        anyhow::bail!(
            "jit_update must never run for a foreign-audience view_instance() call -- the \
             #1486 guard in MaterializedView::scan should have denied the scan first"
        )
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

/// Pins the actual enforcement wiring `materialized_view.rs:79-87` relies on: that
/// `MaterializedView::scan` calls the `instance_guard`'s `authorize_view_instance` and denies
/// before ever calling `View::jit_update`. Every test that already covers this end-to-end
/// (`prong_b_guard_db_test.rs`, `ownership_rewrite_db_test.rs`) is `#[ignore]`d and DB-backed, so
/// CI's plain `cargo test` needs this offline seam to catch a regression -- e.g. dropping
/// `Some(...)` back to `None` at `view_instance_table_function.rs`'s `MaterializedView::new`
/// call, or deleting the `if let Some(guard) = ...` block in `scan`.
#[tokio::test]
async fn materialized_view_scan_denies_before_jit_update_for_foreign_audience_instance() {
    let lakehouse = make_offline_lakehouse_context().await;
    let jit_update_called = Arc::new(AtomicBool::new(false));
    let view: Arc<dyn View> = Arc::new(JitUpdateMustNotRunView::new(
        jit_update_called.clone(),
        &Uuid::new_v4().to_string(),
    ));
    // Scoped away from the instance's (unresolvable, since the index's pool is unroutable)
    // owning audience -- not `ReadScope::All`, and the view set is not on `public_view_sets`, so
    // `authorize_view_instance` takes its resolution branch and fails closed on the connection
    // error, exactly like `authorize_view_instance_denies_on_resolution_error_not_pass` above.
    let guard = Arc::new(AudienceGuard::new(
        audiences(&["team-alpha"]),
        false,
        vec![],
        unroutable_index(),
    ));
    let materialized_view = MaterializedView::new(
        lakehouse.clone(),
        lakehouse.reader_factory().clone(),
        view,
        Arc::new(NullPartitionProvider {}),
        None,
        Some(guard),
    );

    let ctx = SessionContext::new();
    ctx.register_table("guarded_instance", Arc::new(materialized_view))
        .expect("register_table");
    ctx.sql("SELECT * FROM guarded_instance")
        .await
        .expect("planning must succeed -- the guard only runs at scan time, not at plan time")
        .collect()
        .await
        .expect_err(
            "a caller scoped away from the instance's owning audience must be denied at scan \
             time, before jit_update ever runs",
        );

    assert!(
        !jit_update_called.load(Ordering::SeqCst),
        "jit_update must never have run: MaterializedView::scan's #1486 guard check should have \
         denied the scan before reaching it"
    );
}

/// Pins `MaterializedView::instance_is_audience_guarded()` against
/// `AudienceGuard::authorize_view_instance`'s own arms (the coupling its doc comment calls out):
/// true only when a guard is present *and* the instance id is not `"global"` -- the exact
/// condition under which `authorize_view_instance` takes its Uuid-resolution arm (or its
/// fail-closed fallthrough) rather than one of its unconditional-pass short-circuits. A future
/// edit to either side that breaks this correspondence should fail this test, not silently change
/// which of `OwnershipRewrite`'s subquery predicates get skipped.
#[tokio::test]
async fn instance_is_audience_guarded_matches_authorize_view_instance_arms() {
    let lakehouse = make_offline_lakehouse_context().await;
    let jit_update_called = Arc::new(AtomicBool::new(false));
    let guard = Arc::new(AudienceGuard::new(
        audiences(&["team-alpha"]),
        false,
        vec![],
        unroutable_index(),
    ));

    // guard present + "global" -> false: `authorize_view_instance` passes this unconditionally
    // (global instances are row-filtered, not call-guarded), so there is nothing for
    // `OwnershipRewrite` to skip.
    let global_view: Arc<dyn View> = Arc::new(JitUpdateMustNotRunView::new(
        jit_update_called.clone(),
        "global",
    ));
    let global_mat_view = MaterializedView::new(
        lakehouse.clone(),
        lakehouse.reader_factory().clone(),
        global_view,
        Arc::new(NullPartitionProvider {}),
        None,
        Some(guard.clone()),
    );
    assert!(
        !global_mat_view.instance_is_audience_guarded(),
        "a guard present over the 'global' instance id must report false: \
         authorize_view_instance passes it unconditionally"
    );

    // guard present + UUID -> true: `authorize_view_instance` takes its Uuid-resolution arm (or
    // its fail-closed fallthrough) for this instance id.
    let uuid_view: Arc<dyn View> = Arc::new(JitUpdateMustNotRunView::new(
        jit_update_called.clone(),
        &Uuid::new_v4().to_string(),
    ));
    let uuid_mat_view = MaterializedView::new(
        lakehouse.clone(),
        lakehouse.reader_factory().clone(),
        uuid_view,
        Arc::new(NullPartitionProvider {}),
        None,
        Some(guard),
    );
    assert!(
        uuid_mat_view.instance_is_audience_guarded(),
        "a guard present over a UUID instance id must report true: authorize_view_instance \
         resolves and can deny it"
    );

    // guard absent -> false: this is a server-constructed MaterializedView (e.g. a global table
    // or OwnershipRewrite's own processes/streams source), never reachable through the guard's
    // arms at all.
    let no_guard_view: Arc<dyn View> = Arc::new(JitUpdateMustNotRunView::new(
        jit_update_called,
        &Uuid::new_v4().to_string(),
    ));
    let no_guard_mat_view = MaterializedView::new(
        lakehouse.clone(),
        lakehouse.reader_factory().clone(),
        no_guard_view,
        Arc::new(NullPartitionProvider {}),
        None,
        None,
    );
    assert!(
        !no_guard_mat_view.instance_is_audience_guarded(),
        "no guard at all must report false: there is no authorize_view_instance call to skip \
         ahead of"
    );
}
