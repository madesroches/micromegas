//! Unit tests for `IsolationConfig::from_env` (#1370, AbAC Stage 2), the parser Stage 1's
//! `AudienceReadPolicy::from_env` (`rust/auth/src/policy.rs`, covered by
//! `rust/auth/tests/policy_tests.rs`) is explicitly modeled on -- unlike that policy, this parser
//! shipped with no test coverage at all.
//!
//! Every test here mutates process-wide env vars, so all are `#[serial]` with an `EnvGuard` that
//! restores them on drop, the same pattern as `rust/auth/tests/default_provider_tests.rs`. A
//! test-only prefix (`MICROMEGAS_1370_CONFIG_TESTS`) keeps the *prefixed* var names from
//! colliding with any other test/process env; the *unprefixed* fallback vars
//! (`MICROMEGAS_UNSTAMPED_AUDIENCE`/`MICROMEGAS_PUBLIC_VIEW_SETS`) are not otherwise touched by
//! any test in this repo (checked via grep), but are still cleared by `EnvGuard` since this
//! file's tests are the only ones that set them.
//!
//! `unstamped_audience`/`MICROMEGAS_UNSTAMPED_AUDIENCE` are removed outright (#1482 §0/§4): the
//! audience column is now physical and non-nullable on every global view, so there is no more
//! query-time "unstamped" fallback to configure. What used to be several parsing cases for that
//! knob collapses to one: setting it at all (prefixed or unprefixed, including to an empty
//! string) is now a startup error naming its replacement,
//! `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` (the write-side knob, `rust/ingestion`).

#![cfg(test)]

use micromegas_analytics::lakehouse::read_scope::IsolationConfig;
use serial_test::serial;

const PREFIX: &str = "MICROMEGAS_1370_CONFIG_TESTS";
const PREFIXED_UNSTAMPED_VAR: &str = "MICROMEGAS_1370_CONFIG_TESTS_UNSTAMPED_AUDIENCE";
const PREFIXED_PUBLIC_VIEW_SETS_VAR: &str = "MICROMEGAS_1370_CONFIG_TESTS_PUBLIC_VIEW_SETS";
const UNPREFIXED_UNSTAMPED_VAR: &str = "MICROMEGAS_UNSTAMPED_AUDIENCE";
const UNPREFIXED_PUBLIC_VIEW_SETS_VAR: &str = "MICROMEGAS_PUBLIC_VIEW_SETS";

/// Clears all four vars on drop so a failing assertion in one test can't leak state into the
/// next.
struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
            std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
            std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
            std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
        }
    }
}

#[test]
#[serial]
fn unset_vars_resolve_to_default() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
    assert!(
        config.public_view_sets.is_empty(),
        "an unset public-view-sets var must resolve to no public view sets"
    );
}

#[test]
#[serial]
fn a_set_unstamped_audience_var_is_a_startup_error() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(PREFIXED_UNSTAMPED_VAR, "everyone");
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let err = IsolationConfig::from_env(PREFIX)
        .expect_err("a set *_UNSTAMPED_AUDIENCE must be rejected, not silently ignored");
    let msg = err.to_string();
    assert!(
        msg.contains(PREFIXED_UNSTAMPED_VAR)
            && msg.contains("MICROMEGAS_DEFAULT_INGESTION_AUDIENCE"),
        "expected the error to name the offending var and its replacement, got: {msg}"
    );
}

#[test]
#[serial]
fn an_empty_string_unstamped_audience_var_is_still_a_startup_error() {
    // Even the "opt into fail-closed" spelling from the removed knob's era must fail loudly, not
    // be silently treated as unset.
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(PREFIXED_UNSTAMPED_VAR, "");
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let err = IsolationConfig::from_env(PREFIX)
        .expect_err("an explicitly-set empty *_UNSTAMPED_AUDIENCE must still be rejected");
    assert!(err.to_string().contains(PREFIXED_UNSTAMPED_VAR));
}

#[test]
#[serial]
fn unprefixed_unstamped_audience_var_is_also_a_startup_error() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::set_var(UNPREFIXED_UNSTAMPED_VAR, "everyone");
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let err = IsolationConfig::from_env(PREFIX)
        .expect_err("the unprefixed fallback spelling must be rejected too");
    assert!(err.to_string().contains(UNPREFIXED_UNSTAMPED_VAR));
}

#[test]
#[serial]
fn public_view_sets_parses_comma_separated_list() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::set_var(PREFIXED_PUBLIC_VIEW_SETS_VAR, "log_stats, images");
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
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
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::set_var(PREFIXED_PUBLIC_VIEW_SETS_VAR, r#"["log_stats"]"#);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let err = IsolationConfig::from_env(PREFIX).expect_err(
        "a JSON-array-shaped value must be rejected -- this var is comma-separated, not a \
         MICROMEGAS_ADMINS-style JSON array",
    );
    let msg = err.to_string();
    assert!(
        msg.contains(PREFIXED_PUBLIC_VIEW_SETS_VAR),
        "expected the error to name the offending var, got: {msg}"
    );
}

#[test]
#[serial]
fn public_view_sets_rejects_empty_entries() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::set_var(PREFIXED_PUBLIC_VIEW_SETS_VAR, "log_stats,,images");
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let err = IsolationConfig::from_env(PREFIX)
        .expect_err("an empty comma-separated entry must be rejected, not silently dropped");
    let msg = err.to_string();
    assert!(
        msg.contains(PREFIXED_PUBLIC_VIEW_SETS_VAR),
        "expected the error to name the offending var, got: {msg}"
    );
}

#[test]
#[serial]
fn public_view_sets_all_whitespace_resolves_to_empty() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::set_var(PREFIXED_PUBLIC_VIEW_SETS_VAR, "   ");
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
    assert!(
        config.public_view_sets.is_empty(),
        "an all-whitespace value must resolve to no public view sets, not an error"
    );
}
