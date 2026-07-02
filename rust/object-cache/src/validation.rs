use anyhow::{Result, bail};

/// Validate a request key: reject structurally unsafe keys (empty, absolute, or
/// containing `..` traversal segments) and enforce that it falls under one of
/// the `allowed_prefixes`.
///
/// An empty `allowed_prefixes` list means allow-all (still enforcing structure).
/// Callers that must fail closed are responsible for refusing to configure an
/// empty list.
pub fn validate_key(key: &str, allowed_prefixes: &[String]) -> Result<()> {
    if key.is_empty() {
        bail!("empty key");
    }
    if key.starts_with('/') {
        bail!("key must not start with /");
    }
    if key.split('/').any(|seg| seg == "..") {
        bail!("key must not contain ..");
    }
    if allowed_prefixes.is_empty() {
        return Ok(());
    }
    // Admit the key if it matches any prefix on the equal-or-`{p}/`-boundary
    // rule, so `blobs` admits `blobs` and `blobs/x` but not `blobs-secret/y`.
    let allowed = allowed_prefixes
        .iter()
        .any(|p| key == p || key.starts_with(&format!("{p}/")));
    if !allowed {
        bail!("key {key} is outside allowed prefixes {allowed_prefixes:?}");
    }
    Ok(())
}
