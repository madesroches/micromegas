//! Unit tests for `IsolationConfig::from_env` (#1370, AbAC Stage 2; #1371, AbAC Stage 3), the
//! parser Stage 1's `AudienceReadPolicy::from_env` (`rust/auth/src/policy.rs`, covered by
//! `rust/auth/tests/policy_tests.rs`) is explicitly modeled on -- unlike that policy, this parser
//! shipped with no test coverage at all.
//!
//! Every test here mutates process-wide env vars, so all are `#[serial]` with an `EnvGuard` that
//! restores them on drop, the same pattern as `rust/auth/tests/default_provider_tests.rs`. A
//! test-only prefix (`MICROMEGAS_1370_CONFIG_TESTS`) keeps the *prefixed* var names from
//! colliding with any other test/process env; the *unprefixed* fallback vars
//! (`MICROMEGAS_UNSTAMPED_AUDIENCE`/`MICROMEGAS_PUBLIC_VIEW_SETS`/
//! `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS`) are not otherwise touched by any test in this repo
//! (checked via grep), but are still cleared by `EnvGuard` since this file's tests are the only
//! ones that set them.

#![cfg(test)]

use micromegas_analytics::lakehouse::read_scope::IsolationConfig;
use serial_test::serial;

const PREFIX: &str = "MICROMEGAS_1370_CONFIG_TESTS";
const PREFIXED_UNSTAMPED_VAR: &str = "MICROMEGAS_1370_CONFIG_TESTS_UNSTAMPED_AUDIENCE";
const PREFIXED_PUBLIC_VIEW_SETS_VAR: &str = "MICROMEGAS_1370_CONFIG_TESTS_PUBLIC_VIEW_SETS";
const PREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR: &str =
    "MICROMEGAS_1370_CONFIG_TESTS_USER_MAINTENANCE_FUNCTIONS";
const UNPREFIXED_UNSTAMPED_VAR: &str = "MICROMEGAS_UNSTAMPED_AUDIENCE";
const UNPREFIXED_PUBLIC_VIEW_SETS_VAR: &str = "MICROMEGAS_PUBLIC_VIEW_SETS";
const UNPREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR: &str = "MICROMEGAS_USER_MAINTENANCE_FUNCTIONS";

/// Clears all six vars on drop so a failing assertion in one test can't leak state into the
/// next.
struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
            std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
            std::env::remove_var(PREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR);
            std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
            std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
            std::env::remove_var(UNPREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR);
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
        std::env::remove_var(PREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR);
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR);
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
    assert_eq!(
        config.unstamped_audience, None,
        "an unset unstamped-audience var must resolve to None, not a default that would \
         silently open visibility"
    );
    assert!(
        config.public_view_sets.is_empty(),
        "an unset public-view-sets var must resolve to no public view sets"
    );
    assert!(
        !config.user_maintenance_functions,
        "an unset user-maintenance-functions var must resolve to false, not a default that \
         would silently open the mutating functions to every caller"
    );
}

#[test]
#[serial]
fn all_whitespace_unstamped_audience_resolves_to_none() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(PREFIXED_UNSTAMPED_VAR, "   ");
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
    assert_eq!(
        config.unstamped_audience, None,
        "an all-whitespace value must be treated as unset, not as a malformed audience"
    );
}

#[test]
#[serial]
fn malformed_unstamped_audience_is_rejected() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        // `"everyone"` (once the case this test covered, back under the `user:`/`group:`
        // prefix model) is a *valid* audience name under `[A-Za-z0-9_-]` -- `"a:b"` is a
        // still-invalid example under the relaxed charset (`:` is outside it).
        std::env::set_var(PREFIXED_UNSTAMPED_VAR, "a:b");
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let err = IsolationConfig::from_env(PREFIX)
        .expect_err("an audience outside [A-Za-z0-9_-] must be rejected, not silently ignored");
    let msg = err.to_string();
    assert!(
        msg.contains(PREFIXED_UNSTAMPED_VAR) && msg.contains("a:b"),
        "expected the error to name the offending var and value, got: {msg}"
    );
}

#[test]
#[serial]
fn well_formed_unstamped_audience_is_accepted() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(PREFIXED_UNSTAMPED_VAR, "everyone");
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
    assert_eq!(config.unstamped_audience, Some("everyone".to_string()));
}

#[test]
#[serial]
fn prefixed_unstamped_audience_wins_over_unprefixed_fallback() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(PREFIXED_UNSTAMPED_VAR, "prefixed");
        std::env::set_var(UNPREFIXED_UNSTAMPED_VAR, "unprefixed");
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
    assert_eq!(
        config.unstamped_audience,
        Some("prefixed".to_string()),
        "the prefixed var must win over the unprefixed fallback when both are set"
    );
}

#[test]
#[serial]
fn unprefixed_unstamped_audience_used_when_prefixed_is_unset() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::set_var(UNPREFIXED_UNSTAMPED_VAR, "unprefixed");
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
    assert_eq!(
        config.unstamped_audience,
        Some("unprefixed".to_string()),
        "the unprefixed fallback must be used only when the prefixed var is genuinely unset"
    );
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

#[test]
#[serial]
fn user_maintenance_functions_true_is_accepted_case_insensitively() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::set_var(PREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR, "TRUE");
        std::env::remove_var(UNPREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR);
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
    assert!(config.user_maintenance_functions);
}

#[test]
#[serial]
fn user_maintenance_functions_false_is_accepted() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::set_var(PREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR, "false");
        std::env::remove_var(UNPREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR);
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
    assert!(!config.user_maintenance_functions);
}

#[test]
#[serial]
fn user_maintenance_functions_garbage_value_is_rejected() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::set_var(PREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR, "yes");
        std::env::remove_var(UNPREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR);
    }
    let err = IsolationConfig::from_env(PREFIX).expect_err(
        "a value other than \"true\"/\"false\" must be rejected, not silently treated as false",
    );
    let msg = err.to_string();
    assert!(
        msg.contains(PREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR) && msg.contains("yes"),
        "expected the error to name the offending var and value, got: {msg}"
    );
}

#[test]
#[serial]
fn user_maintenance_functions_prefixed_wins_over_unprefixed_fallback() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::set_var(PREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR, "true");
        std::env::set_var(UNPREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR, "false");
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
    assert!(
        config.user_maintenance_functions,
        "the prefixed var must win over the unprefixed fallback when both are set"
    );
}

#[test]
#[serial]
fn user_maintenance_functions_unprefixed_used_when_prefixed_is_unset() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(UNPREFIXED_UNSTAMPED_VAR);
        std::env::remove_var(PREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(UNPREFIXED_PUBLIC_VIEW_SETS_VAR);
        std::env::remove_var(PREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR);
        std::env::set_var(UNPREFIXED_USER_MAINTENANCE_FUNCTIONS_VAR, "true");
    }
    let config = IsolationConfig::from_env(PREFIX).expect("from_env");
    assert!(
        config.user_maintenance_functions,
        "the unprefixed fallback must be used only when the prefixed var is genuinely unset"
    );
}
