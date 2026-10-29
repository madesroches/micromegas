//! Shared handling of `key:<key_id>` selectors for the group-member and audience-grant routes.

use micromegas::auth::policy::{key_selector, parse_key_selector};
use uuid::Uuid;

/// The 404 body when a `key:` selector names no live analytics key.
pub fn key_not_found_message(selector: &str) -> String {
    format!("analytics API key {selector:?} not found or revoked")
}

/// Rewrites any `Uuid::parse_str` spelling after `key:` to the canonical selector, since
/// matching is an exact string compare. Anything else is returned unchanged, left for the
/// shape check to reject.
pub fn canonicalize_key_selector(selector: &str) -> String {
    match selector.strip_prefix("key:").map(Uuid::parse_str) {
        Some(Ok(id)) => key_selector(id),
        _ => selector.to_string(),
    }
}

/// For a canonical `key:` selector, the name of the live (not revoked) analytics key it names, or
/// `None` when there is no such key. Catches typos and ingestion key ids, which live in the other
/// table. Callers run it only after authorization so the answer is not an existence oracle for
/// callers who could not create the row anyway.
pub async fn ensure_live_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    selector: &str,
) -> Result<Option<String>, sqlx::Error> {
    let Some(key_id) = parse_key_selector(selector) else {
        return Ok(None);
    };
    sqlx::query_scalar(
        "SELECT name FROM analytics_api_keys WHERE key_id = $1 AND revoked_at IS NULL",
    )
    .bind(key_id)
    .fetch_optional(&mut **tx)
    .await
}
