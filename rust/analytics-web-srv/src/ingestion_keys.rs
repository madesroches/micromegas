//! Ingestion key management API for `analytics-web-srv` (#1458).
//!
//! Modeled directly on `analytics_keys.rs`, targeting `ingestion_api_keys`
//! instead of `analytics_api_keys`. Replaces the proxy that used to forward
//! mint/list/revoke calls to ingestion's own (now removed)
//! `/auth/api_keys*` routes: ingestion should only do ingestion, so
//! `analytics-web-srv` writes directly to `ingestion_api_keys` via the same
//! telemetry-DB pool it already opens for `analytics_api_keys` — both tables
//! live in the same database behind `MICROMEGAS_SQL_CONNECTION_STRING`.
//! Routes live under `{base_path}/api/ingestion-api-keys`, distinct from this
//! service's own `/auth/*` routes (login/callback/refresh/logout/me) — a
//! completely different concern (browser session lifecycle).
//!
//! This is also the attribution fix: every mint/revoke/import now records the
//! acting admin's own OIDC identity (via the [`AdminUser`] extractor), never a
//! shared service credential the way the removed proxy did.
//!
//! **Duplication, accepted.** This duplicates most of `analytics_keys.rs`'s
//! validation/SQL/error shape — deliberately, per that module's own doc
//! comment: sharing it would mean a generic abstraction over a handful of
//! near-identical handlers differing only in which table they target, the
//! same shape the codebase already declines to share between
//! `data_sources.rs`/`screens.rs`/`folders.rs` today.

use crate::auth::AdminUser;
use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use micromegas::auth::db_api_key::{generate_key, hash_key};
use micromegas::auth::policy::{PUBLIC_AUDIENCE, is_valid_audience};
use micromegas::tracing::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Bytes, not chars: deliberately stricter than the `VARCHAR(255)` column,
/// which bounds characters — same rule as `analytics_keys.rs`.
const MAX_NAME_BYTES: usize = 255;
const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;

/// Holds the (possibly absent) telemetry-DB pool for the ingestion-key
/// routes. `None` only when `MICROMEGAS_SQL_CONNECTION_STRING` is unset — the
/// routes stay registered either way and return 503 per-request in that case,
/// the same always-register-503-when-unconfigured shape `AnalyticsKeysState`
/// uses. An unmigrated DB (missing the v5 migration's tables) is a separate
/// failure mode: the pool is still `Some`, and a request fails with a 500 at
/// query time instead.
#[derive(Clone)]
pub struct IngestionKeysState {
    pub pool: Option<PgPool>,
    /// Resolved once at startup from `{prefix}_DEFAULT_KEY_AUDIENCE`
    /// (`micromegas::auth::policy::default_key_audience_from_env`, `web_server.rs`). `None`
    /// when the knob is unset -- `mint` then requires an explicit `audience` (400 otherwise);
    /// `import` falls back further, to `PUBLIC_AUDIENCE`. See [`resolve_audience`].
    pub default_audience: Option<String>,
}

/// JSON error body returned by every handler in this module. Same
/// `{code, message}` shape as `analytics_keys.rs::ErrorResponse`, redefined
/// here (rather than imported) since that struct's fields/constructor are
/// private to its own module.
#[derive(Debug, Serialize)]
struct ErrorResponse {
    code: String,
    message: String,
}

impl ErrorResponse {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// Errors this API returns.
///
/// No `Forbidden` variant here: the admin gate is the [`AdminUser`] extractor
/// (`auth/handlers.rs`), whose rejection renders as `AdminRequired`'s own 403
/// body — before any handler in this file even starts running — so a
/// `Forbidden` variant here would be dead code, never constructed.
#[derive(Debug)]
pub enum IngestionKeyError {
    /// Request body/query failed validation.
    BadRequest(String),
    /// Unknown `key_id`.
    NotFound,
    /// A DB error.
    Database(sqlx::Error),
    /// `state.pool == None` — the telemetry-DB pool was never configured
    /// (`MICROMEGAS_SQL_CONNECTION_STRING` unset).
    NotConfigured,
}

impl IntoResponse for IngestionKeyError {
    fn into_response(self) -> Response {
        match self {
            IngestionKeyError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new("BAD_REQUEST", msg)),
            )
                .into_response(),
            IngestionKeyError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse::new("NOT_FOUND", "key not found")),
            )
                .into_response(),
            IngestionKeyError::Database(err) => {
                error!("ingestion_keys: database error: {err}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new(
                        "DATABASE_ERROR",
                        "internal database error",
                    )),
                )
                    .into_response()
            }
            IngestionKeyError::NotConfigured => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "NOT_CONFIGURED",
                    "ingestion key store not configured: set MICROMEGAS_SQL_CONNECTION_STRING",
                )),
            )
                .into_response(),
        }
    }
}

impl From<sqlx::Error> for IngestionKeyError {
    fn from(err: sqlx::Error) -> Self {
        IngestionKeyError::Database(err)
    }
}

fn require_pool(state: &IngestionKeysState) -> Result<PgPool, IngestionKeyError> {
    state.pool.clone().ok_or(IngestionKeyError::NotConfigured)
}

fn validate_name(name: &str) -> Result<(), IngestionKeyError> {
    if name.is_empty() {
        return Err(IngestionKeyError::BadRequest(
            "name must not be empty".to_string(),
        ));
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(IngestionKeyError::BadRequest(format!(
            "name must be at most {MAX_NAME_BYTES} bytes"
        )));
    }
    Ok(())
}

/// Resolves the audience to stamp on a mint/import `INSERT`'s `NOT NULL` column
/// (`tasks/1372_audience_on_keys_plan.md` §5-§6). `pub`, not module-private, and sync with no
/// pool access, so the whole resolution matrix is unit-testable without a database.
///
/// `requested`: a missing field or an empty string counts as absent (the empty string is not a
/// name -- it fails [`is_valid_audience`] either way); anything else is taken **verbatim**, no
/// case folding. `fallback`: `None` for `mint` (an unresolved mint is a `BadRequest`, never a
/// silent `public`), `Some(PUBLIC_AUDIENCE)` for `import` (continuity with the v6 backfill).
///
/// Resolution order: `requested` → `state.default_audience` → `fallback`; the first
/// non-absent value is validated with [`is_valid_audience`] and returned. `BadRequest` when
/// nothing resolves at all.
pub fn resolve_audience(
    state: &IngestionKeysState,
    requested: Option<&str>,
    fallback: Option<&str>,
) -> Result<String, IngestionKeyError> {
    let requested = requested.filter(|s| !s.is_empty());
    let default_audience = state.default_audience.as_deref();
    let candidate = requested.or(default_audience).or(fallback);
    match candidate {
        Some(aud) if is_valid_audience(aud) => Ok(aud.to_string()),
        Some(aud) => Err(IngestionKeyError::BadRequest(format!(
            "invalid audience {aud:?}: must match [A-Za-z0-9_-]{{1,255}}"
        ))),
        None => Err(IngestionKeyError::BadRequest(
            "no audience given and MICROMEGAS_DEFAULT_KEY_AUDIENCE is not set".to_string(),
        )),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MintRequest {
    name: String,
    audience: Option<String>,
}

#[derive(Serialize)]
struct MintResponse {
    key_id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    audience: String,
    /// The cleartext key, returned exactly once. Never logged, never
    /// retrievable afterwards.
    key: String,
}

/// `POST {base_path}/api/ingestion-api-keys` — mints a new
/// `ingestion_api_keys` row.
async fn mint_key(
    Extension(state): Extension<IngestionKeysState>,
    AdminUser(user): AdminUser,
    Json(body): Json<MintRequest>,
) -> Result<(StatusCode, Json<MintResponse>), IngestionKeyError> {
    let pool = require_pool(&state)?;
    validate_name(&body.name)?;
    // `fallback: None` -- a new credential must never silently default to `public`; with
    // neither an explicit `audience` nor `MICROMEGAS_DEFAULT_KEY_AUDIENCE` configured, this
    // is a `BadRequest`, not a fail-open publish grant.
    let audience = resolve_audience(&state, body.audience.as_deref(), None)?;

    let key = generate_key();
    let hash = hash_key(&key);
    let key_id = Uuid::new_v4();
    let created_at = Utc::now();
    let created_by = user.email.clone().unwrap_or_else(|| user.subject.clone());

    // Table name is a literal, never derived from caller input: no route in
    // this module ever writes to `analytics_api_keys`.
    sqlx::query(
        "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by, audience)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(key_id)
    .bind(&hash[..])
    .bind(&body.name)
    .bind(created_at)
    .bind(&created_by)
    .bind(&audience)
    .execute(&pool)
    .await?;

    info!(
        "minted ingestion api key key_id={key_id} name={} created_by={created_by} audience={audience}",
        body.name
    );

    Ok((
        StatusCode::CREATED,
        Json(MintResponse {
            key_id,
            name: body.name,
            created_at,
            audience,
            key,
        }),
    ))
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    include_revoked: Option<bool>,
}

#[derive(Serialize, sqlx::FromRow)]
struct KeyListEntry {
    key_id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    created_by: String,
    last_used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    revoked_by: Option<String>,
    audience: String,
}

/// `GET {base_path}/api/ingestion-api-keys?limit=&offset=&include_revoked=` —
/// lists `ingestion_api_keys` rows, newest first. Never `key_hash`, never the
/// key.
async fn list_keys(
    Extension(state): Extension<IngestionKeysState>,
    AdminUser(_user): AdminUser,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<KeyListEntry>>, IngestionKeyError> {
    let pool = require_pool(&state)?;

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if limit <= 0 {
        return Err(IngestionKeyError::BadRequest(
            "limit must be > 0".to_string(),
        ));
    }
    // A read endpoint, so capping is safer than erroring: silently clamp
    // rather than reject values above MAX_LIMIT.
    let limit = limit.min(MAX_LIMIT);
    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(IngestionKeyError::BadRequest(
            "offset must be >= 0".to_string(),
        ));
    }
    let include_revoked = query.include_revoked.unwrap_or(true);

    let rows = if include_revoked {
        sqlx::query_as::<_, KeyListEntry>(
            "SELECT key_id, name, created_at, created_by, last_used_at, revoked_at, revoked_by, audience
             FROM ingestion_api_keys
             ORDER BY created_at DESC
             LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&pool)
        .await?
    } else {
        sqlx::query_as::<_, KeyListEntry>(
            "SELECT key_id, name, created_at, created_by, last_used_at, revoked_at, revoked_by, audience
             FROM ingestion_api_keys
             WHERE revoked_at IS NULL
             ORDER BY created_at DESC
             LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&pool)
        .await?
    };

    Ok(Json(rows))
}

#[derive(Serialize)]
struct RevokeResponse {
    revoked_at: DateTime<Utc>,
}

/// `DELETE {base_path}/api/ingestion-api-keys/{key_id}` — idempotent in one
/// statement, preserving the original revocation time on a repeat call.
///
/// No `effective_within_seconds` field, unlike the removed ingestion-hosted
/// `revoke_key`: that field threaded the *validating* provider's
/// `cache_ttl_secs`, but nothing in `analytics-web-srv` runs a
/// `DbApiKeyAuthProvider` — there is no running cache TTL here to report. The
/// revocation latency is still bounded by whichever ingestion/flight-sql
/// process's cache TTL is validating the key, documented in the runbook
/// rather than echoed by this response.
async fn revoke_key(
    Extension(state): Extension<IngestionKeysState>,
    AdminUser(user): AdminUser,
    Path(key_id): Path<Uuid>,
) -> Result<Json<RevokeResponse>, IngestionKeyError> {
    let pool = require_pool(&state)?;
    let revoked_by = user.email.clone().unwrap_or_else(|| user.subject.clone());

    let row = sqlx::query(
        "UPDATE ingestion_api_keys
         SET revoked_at = COALESCE(revoked_at, now()),
             revoked_by = COALESCE(revoked_by, $2)
         WHERE key_id = $1
         RETURNING revoked_at",
    )
    .bind(key_id)
    .bind(&revoked_by)
    .fetch_optional(&pool)
    .await?;

    match row {
        Some(row) => {
            let revoked_at: DateTime<Utc> = row.try_get("revoked_at")?;
            info!("revoked ingestion api key key_id={key_id} revoked_by={revoked_by}");
            Ok(Json(RevokeResponse { revoked_at }))
        }
        None => Err(IngestionKeyError::NotFound),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportRequest {
    name: String,
    key: String,
    audience: Option<String>,
}

#[derive(Serialize)]
struct ImportResponse {
    key_id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    created_by: String,
    /// `null` unless the already-present row (on the `imported: false` path)
    /// was itself revoked.
    revoked_at: Option<DateTime<Utc>>,
    /// `true` on a fresh insert; `false` when `key_hash` already existed.
    imported: bool,
    /// The audience the row actually carries. On the already-present
    /// (`imported: false`) path this is the **existing** row's audience, never the
    /// request's -- the binding is immutable, so an import never rewrites it.
    audience: String,
}

/// Row shape shared by both branches of `import_key`'s `INSERT ... ON
/// CONFLICT` / fallback `SELECT`.
#[derive(sqlx::FromRow)]
struct ImportedRow {
    key_id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    created_by: String,
    revoked_at: Option<DateTime<Utc>>,
    audience: String,
}

/// `POST {base_path}/api/ingestion-api-keys/import` — a route the removed
/// proxy never had (the CLI called ingestion's own import route directly
/// instead); now required since the CLI's `--table ingestion` path always
/// targets `analytics-web-srv` and ingestion no longer has an import route of
/// its own to fall back on.
///
/// Hashes and stores a caller-supplied key string verbatim, rather than
/// generating a fresh one. `created_by` is the importing caller's own OIDC
/// identity, never the literal string `"import"`.
///
/// No format validation on `key` beyond non-empty: `hash_key` covers the whole
/// string regardless of shape, which is what lets an operator-chosen legacy
/// key of any format import cleanly.
async fn import_key(
    Extension(state): Extension<IngestionKeysState>,
    AdminUser(user): AdminUser,
    Json(body): Json<ImportRequest>,
) -> Result<(StatusCode, Json<ImportResponse>), IngestionKeyError> {
    let pool = require_pool(&state)?;
    validate_name(&body.name)?;
    if body.key.is_empty() {
        return Err(IngestionKeyError::BadRequest(
            "key must not be empty".to_string(),
        ));
    }
    // `fallback: Some(PUBLIC_AUDIENCE)` -- continuity with the v6 backfill: a legacy key's
    // already-ingested history was just stamped `public`, so an import with no explicit
    // audience and no knob keeps the new rows under the same audience rather than a 400.
    let audience = resolve_audience(&state, body.audience.as_deref(), Some(PUBLIC_AUDIENCE))?;

    let hash = hash_key(&body.key);
    let key_id = Uuid::new_v4();
    let created_at = Utc::now();
    let created_by = user.email.clone().unwrap_or_else(|| user.subject.clone());

    let inserted = sqlx::query_as::<_, ImportedRow>(
        "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by, audience)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (key_hash) DO NOTHING
         RETURNING key_id, name, created_at, created_by, revoked_at, audience",
    )
    .bind(key_id)
    .bind(&hash[..])
    .bind(&body.name)
    .bind(created_at)
    .bind(&created_by)
    .bind(&audience)
    .fetch_optional(&pool)
    .await?;

    let (row, imported, status) = match inserted {
        Some(row) => (row, true, StatusCode::CREATED),
        None => {
            // The hash already exists: report the existing row (including
            // whether it's revoked, and its actual, immutable audience) instead
            // of the freshly-generated values above, which never made it into
            // the table.
            let row = sqlx::query_as::<_, ImportedRow>(
                "SELECT key_id, name, created_at, created_by, revoked_at, audience
                 FROM ingestion_api_keys
                 WHERE key_hash = $1",
            )
            .bind(&hash[..])
            .fetch_one(&pool)
            .await?;
            (row, false, StatusCode::OK)
        }
    };

    info!(
        "imported ingestion api key key_id={} name={} created_by={} imported={imported} audience={}",
        row.key_id, row.name, row.created_by, row.audience
    );

    Ok((
        status,
        Json(ImportResponse {
            key_id: row.key_id,
            name: row.name,
            created_at: row.created_at,
            created_by: row.created_by,
            revoked_at: row.revoked_at,
            imported,
            audience: row.audience,
        }),
    ))
}

/// Routes only — [`IngestionKeysState`] is layered separately in
/// `web_server.rs::build_protected_routes`, the same way `app_db_pool`/
/// `maps_state`/`analytics_keys_state` are.
pub fn ingestion_keys_router(base_path: &str) -> Router {
    Router::new()
        .route(
            &format!("{base_path}/api/ingestion-api-keys"),
            post(mint_key).get(list_keys),
        )
        .route(
            &format!("{base_path}/api/ingestion-api-keys/{{key_id}}"),
            delete(revoke_key),
        )
        .route(
            &format!("{base_path}/api/ingestion-api-keys/import"),
            post(import_key),
        )
}
