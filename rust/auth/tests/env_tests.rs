//! Detection of the removed env-var keyrings and audience-grant maps
//! (`micromegas_auth::env::removed_vars_that_are_set`).
//!
//! Every test here mutates process-wide env vars, so all of them are `#[serial]`.

use micromegas_auth::env::removed_vars_that_are_set;
use serial_test::serial;

const ALL_REMOVED: [&str; 5] = [
    "MICROMEGAS_API_KEYS",
    "MICROMEGAS_INGESTION_API_KEYS",
    "MICROMEGAS_ANALYTICS_API_KEYS",
    "MICROMEGAS_AUDIENCE_GRANTS",
    "MICROMEGAS_ANALYTICS_AUDIENCE_GRANTS",
];

/// Clears every removed var on construction and on drop: on construction so a var exported in
/// the developer's shell can't fail an assertion, on drop so a failing test can't leak state
/// into the next.
struct EnvGuard;

impl EnvGuard {
    fn new() -> Self {
        Self::clear();
        Self
    }

    fn clear() {
        for var in ALL_REMOVED {
            // SAFETY: tests are serialized with `#[serial]`.
            unsafe {
                std::env::remove_var(var);
            }
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        Self::clear();
    }
}

#[test]
#[serial]
fn none_set_is_empty() {
    let _guard = EnvGuard::new();
    assert!(removed_vars_that_are_set(&ALL_REMOVED).is_empty());
}

#[test]
#[serial]
fn each_variable_set_individually_is_returned_alone() {
    for var in ALL_REMOVED {
        let _guard = EnvGuard::new();
        // SAFETY: serialized via `#[serial]`.
        unsafe {
            std::env::set_var(var, "some-value");
        }
        assert_eq!(removed_vars_that_are_set(&ALL_REMOVED), vec![var]);
    }
}

#[test]
#[serial]
fn empty_string_value_still_counts_as_set() {
    let _guard = EnvGuard::new();
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var("MICROMEGAS_API_KEYS", "");
    }
    assert_eq!(
        removed_vars_that_are_set(&ALL_REMOVED),
        vec!["MICROMEGAS_API_KEYS"]
    );
}

#[test]
#[serial]
fn two_set_at_once_are_both_returned_in_list_order() {
    let _guard = EnvGuard::new();
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var("MICROMEGAS_ANALYTICS_API_KEYS", "x");
        std::env::set_var("MICROMEGAS_API_KEYS", "y");
    }
    assert_eq!(
        removed_vars_that_are_set(&ALL_REMOVED),
        vec!["MICROMEGAS_API_KEYS", "MICROMEGAS_ANALYTICS_API_KEYS"]
    );
}
