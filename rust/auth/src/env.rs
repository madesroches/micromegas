//! Shared prefixed-env-var resolution, used by every `{prefix}_*`-with-fallback knob in this
//! crate (`{prefix}_OIDC_CONFIG`, `{prefix}_DEFAULT_AUDIENCE`), and the three surviving
//! `{prefix}_API_KEY_*CACHE*` knobs (`{prefix}_API_KEY_CACHE_SIZE`,
//! `{prefix}_API_KEY_UNKNOWN_CACHE_TTL_SECONDS`, `{prefix}_API_KEY_UNKNOWN_CACHE_SIZE`).

/// Resolves `{prefix}_{suffix}`, falling back to unprefixed `MICROMEGAS_{suffix}` when the
/// prefixed name is unset, or always when `prefix` is empty.
///
/// `suffix` is passed **without** the `MICROMEGAS_` prefix (e.g. `"OIDC_CONFIG"`,
/// `"DEFAULT_AUDIENCE"`).
pub fn resolve_prefixed_var(prefix: &str, suffix: &str) -> String {
    if prefix.is_empty() {
        format!("MICROMEGAS_{suffix}")
    } else {
        let prefixed = format!("{prefix}_{suffix}");
        if std::env::var(&prefixed).is_ok() {
            prefixed
        } else {
            format!("MICROMEGAS_{suffix}")
        }
    }
}

/// Returns the subset of `removed` that is set to any value, empty string included. Pure
/// detection, split out from the `warn_removed_*` wrappers so it is unit-testable without
/// capturing a log sink.
fn removed_vars_that_are_set(removed: &[&'static str]) -> Vec<&'static str> {
    removed
        .iter()
        .copied()
        .filter(|var| std::env::var(var).is_ok())
        .collect()
}

/// Warns when any of the three removed env-var API-keyrings --
/// `MICROMEGAS_API_KEYS`, `MICROMEGAS_INGESTION_API_KEYS`, `MICROMEGAS_ANALYTICS_API_KEYS` -- is
/// still set. `ingestion_api_keys` / `analytics_api_keys` are the only sources from here on;
/// `micromegas-import-keys` migrates a still-set keyring into them. Unlike the #1564 precedent
/// this removal warns rather than refuses startup -- see `## Decisions` in the design plan.
pub(crate) fn warn_removed_api_key_vars() {
    const REMOVED: [&str; 3] = [
        "MICROMEGAS_API_KEYS",
        "MICROMEGAS_INGESTION_API_KEYS",
        "MICROMEGAS_ANALYTICS_API_KEYS",
    ];
    let set = removed_vars_that_are_set(&REMOVED);
    if !set.is_empty() {
        micromegas_tracing::warn!(
            "{} {} set but no longer read -- import the keyring into ingestion_api_keys / \
             analytics_api_keys with `micromegas-import-keys`, then unset {}",
            set.join(", "),
            if set.len() == 1 { "is" } else { "are" },
            if set.len() == 1 { "it" } else { "them" }
        );
    }
}

/// Warns when either of the two removed env-var audience-grant maps --
/// `MICROMEGAS_AUDIENCE_GRANTS`, `MICROMEGAS_ANALYTICS_AUDIENCE_GRANTS` -- is still set. The
/// `audience_grants` table is the only source from here on; `micromegas-grants create <audience>
/// <axis> <selector>` creates a grant row. `MICROMEGAS_INGESTION_AUDIENCE_GRANTS` is excluded --
/// it was never read by any `AudienceReadPolicy::from_env` call, so warning about it would tell
/// an operator they lost a setting that never did anything.
pub(crate) fn warn_removed_audience_grant_vars() {
    const REMOVED: [&str; 2] = [
        "MICROMEGAS_AUDIENCE_GRANTS",
        "MICROMEGAS_ANALYTICS_AUDIENCE_GRANTS",
    ];
    let set = removed_vars_that_are_set(&REMOVED);
    if !set.is_empty() {
        micromegas_tracing::warn!(
            "{} {} set but no longer read -- create the grant with \
             `micromegas-grants create <audience> <axis> <selector>`, then unset {}",
            set.join(", "),
            if set.len() == 1 { "is" } else { "are" },
            if set.len() == 1 { "it" } else { "them" }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    const ALL_REMOVED: [&str; 5] = [
        "MICROMEGAS_API_KEYS",
        "MICROMEGAS_INGESTION_API_KEYS",
        "MICROMEGAS_ANALYTICS_API_KEYS",
        "MICROMEGAS_AUDIENCE_GRANTS",
        "MICROMEGAS_ANALYTICS_AUDIENCE_GRANTS",
    ];

    /// Clears every removed var on drop so a failing assertion in one test can't leak state into
    /// the next.
    struct EnvGuard;

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for var in ALL_REMOVED {
                // SAFETY: tests are serialized with `#[serial]`.
                unsafe {
                    std::env::remove_var(var);
                }
            }
        }
    }

    #[test]
    #[serial]
    fn none_set_is_empty() {
        let _guard = EnvGuard;
        assert!(removed_vars_that_are_set(&ALL_REMOVED).is_empty());
    }

    #[test]
    #[serial]
    fn each_variable_set_individually_is_returned_alone() {
        for var in ALL_REMOVED {
            let _guard = EnvGuard;
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
        let _guard = EnvGuard;
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
        let _guard = EnvGuard;
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
}
