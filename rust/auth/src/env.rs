//! Shared prefixed-env-var resolution, used by every `{prefix}_*`-with-fallback knob in this
//! crate (`{prefix}_API_KEYS`, `{prefix}_ADMINS`, `{prefix}_OIDC_CONFIG`,
//! `{prefix}_AUDIENCE_GRANTS`, `{prefix}_DEFAULT_AUDIENCE`, and the four
//! `{prefix}_API_KEY_CACHE_*` knobs).
//!
//! `micromegas_analytics::lakehouse::read_scope` keeps its own copy of this exact function
//! (`resolved_var`) rather than depending on `micromegas-auth` for it -- deliberately, since
//! `micromegas-analytics` does not pull in this crate's OIDC/JWT dependency tree (see
//! `read_scope.rs`'s module doc comment). Keep the two copies in step if this contract changes.

/// Resolves `{prefix}_{suffix}`, falling back to unprefixed `MICROMEGAS_{suffix}` when the
/// prefixed name is unset, or always when `prefix` is empty.
///
/// `suffix` is passed **without** the `MICROMEGAS_` prefix (e.g. `"API_KEYS"`, `"ADMINS"`,
/// `"OIDC_CONFIG"`, `"DEFAULT_AUDIENCE"`).
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

/// Refuses startup when any of the three env-var-based admin lists this plan removes --
/// `MICROMEGAS_ADMINS`, `MICROMEGAS_ANALYTICS_ADMINS`, `MICROMEGAS_INGESTION_ADMINS` -- is set to
/// any value (including an empty string), naming the replacement. Admin membership lives in the
/// `admins` group (schema v10) from here on; the v10 migration always seeds it with a single
/// wildcard row (`('admins', '*')`), regardless of these vars, and every var must be unset on
/// every later boot. Called from `ProviderBuilder::compose` and `analytics-web-srv`'s
/// `WebServerConfig`, the same posture `IsolationConfig::from_env` takes for `UNSTAMPED_AUDIENCE`.
pub fn reject_removed_admin_vars() -> anyhow::Result<()> {
    const REMOVED: [&str; 3] = [
        "MICROMEGAS_ADMINS",
        "MICROMEGAS_ANALYTICS_ADMINS",
        "MICROMEGAS_INGESTION_ADMINS",
    ];
    let set: Vec<&str> = REMOVED
        .iter()
        .copied()
        .filter(|var| std::env::var(var).is_ok())
        .collect();
    if set.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "{} {} no longer read -- admin membership lives in the `admins` group; manage it \
             with `micromegas-groups` or the Groups admin page",
            set.join(", "),
            if set.len() == 1 { "is" } else { "are" }
        ))
    }
}

/// Refuses startup when any of the renamed per-role cache-TTL vars this plan removes --
/// `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS`, `MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS`, or
/// their `{prefix}_` forms (`MICROMEGAS_ANALYTICS_API_KEY_CACHE_TTL_SECONDS`,
/// `MICROMEGAS_INGESTION_API_KEY_CACHE_TTL_SECONDS`,
/// `MICROMEGAS_ANALYTICS_AUDIENCE_GRANT_CACHE_TTL_SECONDS`,
/// `MICROMEGAS_INGESTION_AUDIENCE_GRANT_CACHE_TTL_SECONDS`) -- is set to any value (including an
/// empty string), naming the replacement. `db_api_key::DbApiKeyConfig` and
/// `db_audience_grants::DbAudienceGrantsConfig` now read the single flat, unprefixed
/// `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` knob instead; an operator who tuned one of the removed
/// vars would otherwise silently fall back to the 60s default with no log line and no startup
/// error. Called from the same sites as [`reject_removed_admin_vars`]:
/// `ProviderBuilder::compose` and `analytics-web-srv`'s `WebServerConfig::from_cli_and_env`.
pub fn reject_removed_cache_ttl_vars() -> anyhow::Result<()> {
    const REMOVED: [&str; 6] = [
        "MICROMEGAS_API_KEY_CACHE_TTL_SECONDS",
        "MICROMEGAS_ANALYTICS_API_KEY_CACHE_TTL_SECONDS",
        "MICROMEGAS_INGESTION_API_KEY_CACHE_TTL_SECONDS",
        "MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS",
        "MICROMEGAS_ANALYTICS_AUDIENCE_GRANT_CACHE_TTL_SECONDS",
        "MICROMEGAS_INGESTION_AUDIENCE_GRANT_CACHE_TTL_SECONDS",
    ];
    let set: Vec<&str> = REMOVED
        .iter()
        .copied()
        .filter(|var| std::env::var(var).is_ok())
        .collect();
    if set.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "{} {} no longer read -- use MICROMEGAS_AUTH_CACHE_TTL_SECONDS instead, the single \
             flat, unprefixed cache-TTL knob shared by the API-key, audience-grant, and group caches",
            set.join(", "),
            if set.len() == 1 { "is" } else { "are" }
        ))
    }
}
