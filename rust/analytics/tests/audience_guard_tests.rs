//! Offline (no live DB) tests for Query Enforcement Prong B (#1371, AbAC Stage 3):
//! `is_readable`'s truth table and `AudienceGuard`'s no-I/O short-circuit under `ReadScope::All`.
//!
//! `IsolationConfig::from_env`'s three knobs (including the new
//! `user_maintenance_functions`) are covered by `ownership_rewrite_config_tests.rs`; the
//! mutating-function registration gate's two states are covered by
//! `lakehouse_admin_gate_test.rs`. Both predate this file and already exercise the pieces this
//! stage added to them.

use micromegas_analytics::lakehouse::audience_guard::{
    AudienceGuard, AudienceIndex, IdKind, OwnerAudience, is_readable,
};
use micromegas_analytics::lakehouse::read_scope::ReadScope;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// An `AudienceIndex` over a `connect_lazy` pool to an unroutable address -- `connect_lazy` never
/// touches the network at construction time (same trick `lakehouse_admin_gate_test.rs` uses for
/// `LakehouseContext`), so a test built from this index can assert "no I/O happened" simply by
/// not hanging/erroring on a query that would otherwise need a real connection.
fn unroutable_index() -> Arc<AudienceIndex> {
    let pool = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
        .expect("connect_lazy should not touch the network");
    Arc::new(AudienceIndex::new(pool, 100_000, Duration::from_secs(300)))
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
    assert!(is_readable(&ReadScope::All, None, &OwnerAudience::Unknown));
    assert!(is_readable(
        &ReadScope::All,
        None,
        &OwnerAudience::Unstamped
    ));
    assert!(is_readable(&ReadScope::All, None, &audience("team-alpha")));
    assert!(is_readable(
        &ReadScope::All,
        Some("public"),
        &OwnerAudience::Unknown
    ));
}

#[test]
fn audiences_denies_unknown_always() {
    let scope = audiences(&["team-alpha"]);
    assert!(!is_readable(&scope, None, &OwnerAudience::Unknown));
    assert!(!is_readable(
        &scope,
        Some("team-alpha"),
        &OwnerAudience::Unknown
    ));
}

#[test]
fn audiences_unstamped_passes_only_when_configured_and_in_scope() {
    let scope = audiences(&["team-alpha"]);
    assert!(
        !is_readable(&scope, None, &OwnerAudience::Unstamped),
        "no unstamped_audience configured -> deny"
    );
    assert!(
        !is_readable(&scope, Some("public"), &OwnerAudience::Unstamped),
        "unstamped_audience configured but not in the caller's scope -> deny"
    );
    assert!(
        is_readable(&scope, Some("team-alpha"), &OwnerAudience::Unstamped),
        "unstamped_audience configured and in scope -> allow"
    );
}

#[test]
fn audiences_audience_matches_byte_exactly() {
    let scope = audiences(&["Team-Alpha"]);
    assert!(
        !is_readable(&scope, None, &audience("team-alpha")),
        "case must matter: 'Team-Alpha' != 'team-alpha'"
    );
    assert!(is_readable(&scope, None, &audience("Team-Alpha")));
}

#[test]
fn audiences_ambiguous_denies_unless_every_owner_is_readable() {
    let scope = audiences(&["team-alpha"]);
    assert!(
        !is_readable(
            &scope,
            None,
            &OwnerAudience::Ambiguous(vec![audience("team-alpha"), audience("team-beta")])
        ),
        "one unreadable owner among the collision's arms -> deny"
    );
    assert!(
        is_readable(
            &scope,
            None,
            &OwnerAudience::Ambiguous(vec![audience("team-alpha"), audience("team-alpha")])
        ),
        "every arm independently readable -> allow"
    );
    assert!(
        !is_readable(
            &scope,
            None,
            &OwnerAudience::Ambiguous(vec![audience("team-beta"), audience("team-beta")])
        ),
        "no arm readable -> deny"
    );
    assert!(
        !is_readable(&scope, None, &OwnerAudience::Ambiguous(vec![])),
        "an empty owner set must not be vacuously readable"
    );
}

#[test]
fn empty_audience_set_denies_everything() {
    let scope = audiences(&[]);
    assert!(!is_readable(&scope, None, &OwnerAudience::Unknown));
    assert!(!is_readable(
        &scope,
        Some("public"),
        &OwnerAudience::Unstamped
    ));
    assert!(!is_readable(&scope, None, &audience("public")));
}

// --- `AudienceGuard` no-I/O short-circuit under `ReadScope::All` -------------------------

#[tokio::test]
async fn authorize_under_read_scope_all_performs_no_io() {
    let guard = AudienceGuard::new(ReadScope::All, None, vec![], unroutable_index());
    let id = Uuid::new_v4();
    let authorized = guard
        .authorize(id, IdKind::Process, "test_fn")
        .await
        .expect("ReadScope::All must authorize with no I/O, even over an unroutable pool");
    assert_eq!(authorized.id(), id);
}

#[tokio::test]
async fn readable_ids_under_read_scope_all_performs_no_io() {
    let guard = AudienceGuard::new(ReadScope::All, None, vec![], unroutable_index());
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
    let guard = AudienceGuard::new(audiences(&["team-alpha"]), None, vec![], unroutable_index());
    let err = guard
        .authorize(Uuid::new_v4(), IdKind::Process, "test_fn")
        .await
        .expect_err("a resolution error must be a denial, never a readable verdict");
    // Whatever the underlying I/O failure looks like, it must not read as the caller-visible
    // existence-oracle-proof denial text either -- assert only that it *is* an error, per the
    // module's fail-closed contract.
    let _ = err;
}

// --- `global_rows_visible` (list_partitions' §8 'global'-row rule) -----------------------

// `unroutable_index()` builds a `sqlx::PgPool` via `connect_lazy`, which -- despite never
// touching the network -- requires a Tokio runtime context to construct (it registers pool
// maintenance internals against the ambient runtime). Hence `#[tokio::test]`, even though
// `global_rows_visible` itself is pure and does no I/O.

#[tokio::test]
async fn global_rows_visible_under_all() {
    let guard = AudienceGuard::new(ReadScope::All, None, vec![], unroutable_index());
    assert!(guard.global_rows_visible("log_entries"));
}

#[tokio::test]
async fn global_rows_visible_via_public_view_sets() {
    let guard = AudienceGuard::new(
        audiences(&["team-alpha"]),
        None,
        vec!["log_stats".to_string()],
        unroutable_index(),
    );
    assert!(guard.global_rows_visible("log_stats"));
    assert!(!guard.global_rows_visible("log_entries"));
}

#[tokio::test]
async fn global_rows_visible_via_unstamped_audience_in_scope() {
    let guard = AudienceGuard::new(
        audiences(&["public"]),
        Some("public".to_string()),
        vec![],
        unroutable_index(),
    );
    assert!(guard.global_rows_visible("log_entries"));

    let guard_out_of_scope = AudienceGuard::new(
        audiences(&["team-alpha"]),
        Some("public".to_string()),
        vec![],
        unroutable_index(),
    );
    assert!(
        !guard_out_of_scope.global_rows_visible("log_entries"),
        "unstamped_audience configured but not in the caller's own scope -> still hidden"
    );
}

#[tokio::test]
async fn global_rows_hidden_by_default_under_restricted_scope() {
    let guard = AudienceGuard::new(audiences(&["team-alpha"]), None, vec![], unroutable_index());
    assert!(!guard.global_rows_visible("log_entries"));
}
