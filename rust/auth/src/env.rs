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
pub fn removed_vars_that_are_set(removed: &[&'static str]) -> Vec<&'static str> {
    removed
        .iter()
        .copied()
        .filter(|var| std::env::var(var).is_ok())
        .collect()
}

/// Warns when any of the three removed env-var API-keyrings --
/// `MICROMEGAS_API_KEYS`, `MICROMEGAS_INGESTION_API_KEYS`, `MICROMEGAS_ANALYTICS_API_KEYS` -- is
/// still set. `ingestion_api_keys` / `analytics_api_keys` are the only sources from here on, and
/// the keys they hold are minted there -- a still-set keyring's own key strings cannot be carried
/// over, so every client presenting one needs a freshly minted key. Unlike the earlier
/// removed-var families, this removal warns rather than refusing startup.
pub(crate) fn warn_removed_api_key_vars() {
    const REMOVED: [&str; 3] = [
        "MICROMEGAS_API_KEYS",
        "MICROMEGAS_INGESTION_API_KEYS",
        "MICROMEGAS_ANALYTICS_API_KEYS",
    ];
    let set = removed_vars_that_are_set(&REMOVED);
    if !set.is_empty() {
        let api_keys_caveat = if set.contains(&"MICROMEGAS_API_KEYS") {
            " on ingestion/flight-sql (`object-cache-srv` still requires it)"
        } else {
            ""
        };
        micromegas_tracing::warn!(
            "{} {} set but no longer read -- mint replacement keys into ingestion_api_keys / \
             analytics_api_keys from the web app's admin pages, redistribute them to the \
             clients still presenting the old ones, then unset {}{}",
            set.join(", "),
            if set.len() == 1 { "is" } else { "are" },
            if set.len() == 1 { "it" } else { "them" },
            api_keys_caveat
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
