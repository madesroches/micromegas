//! Shared prefixed-env-var resolution, used by every `{prefix}_*`-with-fallback knob in this
//! crate (`{prefix}_API_KEYS`, `{prefix}_OIDC_CONFIG`, `{prefix}_AUDIENCE_GRANTS`,
//! `{prefix}_DEFAULT_AUDIENCE`).

/// Resolves `{prefix}_{suffix}`, falling back to unprefixed `MICROMEGAS_{suffix}` when the
/// prefixed name is unset, or always when `prefix` is empty.
///
/// `suffix` is passed **without** the `MICROMEGAS_` prefix (e.g. `"API_KEYS"`,
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
