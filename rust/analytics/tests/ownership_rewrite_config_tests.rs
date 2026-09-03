//! Unit tests for `IsolationConfig::from_env`, modeled on `AudienceReadPolicy::from_env`
//! (`rust/auth/src/policy.rs`, covered by `rust/auth/tests/policy_tests.rs`).
//!
//! Every test here mutates process-wide env vars, so all are `#[serial]` with an `EnvGuard` that
//! restores them on drop, the same pattern as `rust/auth/tests/default_provider_tests.rs`. Within
//! this crate, this file's tests are the only ones that touch `MICROMEGAS_PUBLIC_VIEW_SETS` and
//! `MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` (checked via grep), so `EnvGuard` clears both. The
//! `rust/public` crate has its own tests that touch these vars too
//! (`flight_sql_server.rs::mod tests`, `tests/isolation_config_fail_fast_tests.rs`), but those run
//! in their own processes, so they can't interleave with this crate's `#[serial]` tests.

#![cfg(test)]

use micromegas_analytics::lakehouse::read_scope::IsolationConfig;
use serial_test::serial;

const PUBLIC_VIEW_SETS_VAR: &str = "MICROMEGAS_PUBLIC_VIEW_SETS";
const ANALYTICS_PUBLIC_VIEW_SETS_VAR: &str = "MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS";

/// Clears both vars on drop so a failing assertion in one test can't leak state into the next.
struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(PUBLIC_VIEW_SETS_VAR);
            std::env::remove_var(ANALYTICS_PUBLIC_VIEW_SETS_VAR);
        }
    }
}

#[test]
#[serial]
fn unset_vars_resolve_to_default() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(ANALYTICS_PUBLIC_VIEW_SETS_VAR);
    }
    let config = IsolationConfig::from_env().expect("from_env");
    assert!(
        config.public_view_sets.is_empty(),
        "an unset public-view-sets var must resolve to no public view sets"
    );
}

/// Pins that the dropped per-service prefix is no longer read at all -- otherwise nothing
/// distinguishes the intended drop of `MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` from an accidental
/// one.
#[test]
#[serial]
fn analytics_prefixed_var_is_not_read() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PUBLIC_VIEW_SETS_VAR);
        std::env::set_var(ANALYTICS_PUBLIC_VIEW_SETS_VAR, "log_stats");
    }
    let config = IsolationConfig::from_env().expect("from_env");
    assert!(
        config.public_view_sets.is_empty(),
        "MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS must not be read; only the unprefixed \
         MICROMEGAS_PUBLIC_VIEW_SETS is"
    );
}

#[test]
#[serial]
fn public_view_sets_parses_comma_separated_list() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(PUBLIC_VIEW_SETS_VAR, "log_stats, images");
        std::env::remove_var(ANALYTICS_PUBLIC_VIEW_SETS_VAR);
    }
    let config = IsolationConfig::from_env().expect("from_env");
    assert_eq!(
        config.public_view_sets,
        vec!["log_stats".to_string(), "images".to_string()]
    );
}

#[test]
#[serial]
fn public_view_sets_rejects_json_array_shaped_entries() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(PUBLIC_VIEW_SETS_VAR, r#"["log_stats"]"#);
        std::env::remove_var(ANALYTICS_PUBLIC_VIEW_SETS_VAR);
    }
    let err = IsolationConfig::from_env().expect_err(
        "a JSON-array-shaped value must be rejected -- this var is comma-separated, not a \
         MICROMEGAS_API_KEYS-style JSON array",
    );
    let msg = err.to_string();
    assert!(
        msg.contains(PUBLIC_VIEW_SETS_VAR),
        "expected the error to name the offending var, got: {msg}"
    );
}

#[test]
#[serial]
fn public_view_sets_rejects_empty_entries() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(PUBLIC_VIEW_SETS_VAR, "log_stats,,images");
        std::env::remove_var(ANALYTICS_PUBLIC_VIEW_SETS_VAR);
    }
    let err = IsolationConfig::from_env()
        .expect_err("an empty comma-separated entry must be rejected, not silently dropped");
    let msg = err.to_string();
    assert!(
        msg.contains(PUBLIC_VIEW_SETS_VAR),
        "expected the error to name the offending var, got: {msg}"
    );
}

#[test]
#[serial]
fn public_view_sets_all_whitespace_resolves_to_empty() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(PUBLIC_VIEW_SETS_VAR, "   ");
        std::env::remove_var(ANALYTICS_PUBLIC_VIEW_SETS_VAR);
    }
    let config = IsolationConfig::from_env().expect("from_env");
    assert!(
        config.public_view_sets.is_empty(),
        "an all-whitespace value must resolve to no public view sets, not an error"
    );
}
