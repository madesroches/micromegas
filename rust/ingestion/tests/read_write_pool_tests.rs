//! No-DB unit tests for `micromegas_ingestion::data_lake_connection`'s read-only-connection
//! rejection: the pure `is_read_only`/`is_read_only_violation` helpers, the pool-option builders'
//! shape, and `ReadOnlyFallback`'s accept/evict decisions (driven by explicit `Instant`s, no
//! sleeps).

use micromegas_ingestion::data_lake_connection::{
    ReadOnlyFallback, WritablePolicy, is_read_only, is_read_only_violation, pool_options,
    read_write_pool_options,
};
use std::time::{Duration, Instant};

const FALLBACK_AFTER: Duration = Duration::from_secs(10);
const PROBE_INTERVAL: Duration = Duration::from_secs(30);

#[test]
fn is_read_only_matches_libpq_target_session_attrs_semantics() {
    assert!(is_read_only("on"));
    assert!(!is_read_only("off"));
    // Anything unexpected counts as writable -- an unrecognized value shouldn't take the
    // service down.
    assert!(!is_read_only("unexpected"));
    assert!(!is_read_only(""));
}

#[test]
fn read_write_pool_options_replaces_the_ping_with_before_acquire() {
    assert!(!read_write_pool_options().get_test_before_acquire());
}

#[test]
fn prefer_pool_options_also_replaces_the_ping() {
    assert!(!pool_options(WritablePolicy::Prefer).get_test_before_acquire());
}

// ---------------------------------------------------------------------------
// ReadOnlyFallback
// ---------------------------------------------------------------------------

#[test]
fn on_connect_rejects_read_only_before_the_window_and_accepts_at_it() {
    let fallback = ReadOnlyFallback::new();
    let start = Instant::now();

    assert!(
        !fallback.on_connect(true, start),
        "a read-only connect must be rejected the instant the streak starts"
    );
    assert!(
        !fallback.on_connect(true, start + FALLBACK_AFTER - Duration::from_millis(1)),
        "still rejected just under the fallback window"
    );
    assert!(
        fallback.on_connect(true, start + FALLBACK_AFTER),
        "accepted at or after the fallback window"
    );
}

#[test]
fn on_connect_writable_clears_the_streak_so_the_next_read_only_connect_is_rejected_again() {
    let fallback = ReadOnlyFallback::new();
    let start = Instant::now();

    assert!(!fallback.on_connect(true, start));
    assert!(fallback.on_connect(true, start + FALLBACK_AFTER));

    // A writable connect ends the streak...
    assert!(fallback.on_connect(false, start + FALLBACK_AFTER + Duration::from_secs(1)));

    // ...so the next read-only connect starts a fresh streak and is rejected again, even though
    // the old streak was already past its window.
    let restart = start + FALLBACK_AFTER + Duration::from_secs(2);
    assert!(!fallback.on_connect(true, restart));
    assert!(!fallback.on_connect(true, restart + FALLBACK_AFTER - Duration::from_millis(1)));
    assert!(fallback.on_connect(true, restart + FALLBACK_AFTER));
}

#[test]
fn on_acquire_keeps_writable_and_clears_the_streak() {
    let fallback = ReadOnlyFallback::new();
    let start = Instant::now();

    assert!(!fallback.on_connect(true, start));
    assert!(fallback.on_connect(true, start + FALLBACK_AFTER));

    assert!(
        fallback.on_acquire(false, start + FALLBACK_AFTER + Duration::from_secs(1)),
        "a writable acquire is always kept"
    );

    // The streak is cleared, so the next read-only connect must wait out a fresh window again.
    let restart = start + FALLBACK_AFTER + Duration::from_secs(2);
    assert!(!fallback.on_connect(true, restart));
}

#[test]
fn on_acquire_evicts_read_only_immediately_once_the_streak_is_cleared() {
    let fallback = ReadOnlyFallback::new();
    let now = Instant::now();

    // No streak has ever run: a read-only connection surfacing here is unexpected and evicted
    // at once, exactly like the `Require` policy.
    assert!(!fallback.on_acquire(true, now));
}

#[test]
fn on_acquire_reprobes_read_only_at_most_once_per_probe_interval() {
    let fallback = ReadOnlyFallback::new();
    let start = Instant::now();
    assert!(!fallback.on_connect(true, start));
    assert!(fallback.on_connect(true, start + FALLBACK_AFTER));

    let fallback_accepted_at = start + FALLBACK_AFTER;

    // First acquire after falling back re-probes (evicts).
    assert!(!fallback.on_acquire(true, fallback_accepted_at));
    // A second call shortly after keeps -- still inside the probe interval.
    assert!(fallback.on_acquire(true, fallback_accepted_at + Duration::from_secs(1)));
    // Once the probe interval elapses, it evicts again.
    assert!(!fallback.on_acquire(true, fallback_accepted_at + PROBE_INTERVAL));
    // ...and keeps again right after that re-probe.
    assert!(fallback.on_acquire(
        true,
        fallback_accepted_at + PROBE_INTERVAL + Duration::from_secs(1)
    ));
}

// ---------------------------------------------------------------------------
// is_read_only_violation
// ---------------------------------------------------------------------------

/// A minimal `sqlx::error::DatabaseError` carrying a chosen SQLSTATE -- `PgDatabaseError` has no
/// public constructor, so this stands in for one.
#[derive(Debug)]
struct FakeDatabaseError {
    code: &'static str,
}

impl std::fmt::Display for FakeDatabaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fake database error (code={})", self.code)
    }
}

impl std::error::Error for FakeDatabaseError {}

impl sqlx::error::DatabaseError for FakeDatabaseError {
    fn message(&self) -> &str {
        "fake database error"
    }

    fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
        Some(std::borrow::Cow::Borrowed(self.code))
    }

    fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }

    fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
        self
    }

    fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
        self
    }

    fn kind(&self) -> sqlx::error::ErrorKind {
        sqlx::error::ErrorKind::Other
    }
}

fn fake_db_error(code: &'static str) -> sqlx::Error {
    sqlx::Error::Database(Box::new(FakeDatabaseError { code }))
}

#[test]
fn is_read_only_violation_matches_sqlstate_25006_through_anyhow_context_layers() {
    let err = anyhow::Error::from(fake_db_error("25006"))
        .context("inserting into blocks")
        .context("writing partition");
    assert!(is_read_only_violation(&err));
}

#[test]
fn is_read_only_violation_is_false_for_another_code_or_a_non_database_error() {
    let wrong_code = anyhow::Error::from(fake_db_error("42P01")).context("some other failure");
    assert!(!is_read_only_violation(&wrong_code));

    let not_a_db_error = anyhow::anyhow!("plain error, no sqlx::Error in the chain");
    assert!(!is_read_only_violation(&not_a_db_error));
}
