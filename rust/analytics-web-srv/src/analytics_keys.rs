//! Analytics key management API for `analytics-web-srv`.
//!
//! Bound to `ValidatedUser`/cookie auth (via the `AdminUser` extractor), and
//! to a dedicated [`AnalyticsKeysState`] (holding a small pool into the
//! telemetry DB) instead of a bare `Extension<PgPool>` — `build_protected_routes`
//! already layers `Extension<PgPool>` for `app_db_pool`; axum extensions are
//! keyed by type, so a second bare `Extension<PgPool>` would silently resolve
//! to the app pool instead. Routes live under `{base_path}/api/analytics-api-keys`,
//! distinct from this service's own `/auth/*` routes (login/callback/refresh/logout/me)
//! — a completely different concern (browser session lifecycle).
//!
//! **Duplication, accepted.** This duplicates most of `ingestion_keys.rs`'s
//! validation/SQL/error shape. Sharing it would mean a generic abstraction
//! over a handful of near-identical handlers differing only in which table
//! they target — the same shape the codebase already declines to share
//! between `data_sources.rs`/`screens.rs`/`folders.rs` today.

use crate::auth::AdminUser;
use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use micromegas::auth::db_api_key::{generate_key, hash_key};
use micromegas::tracing::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Bytes, not chars: deliberately stricter than the `VARCHAR(255)` column,
/// which bounds characters — same rule as `ingestion_keys.rs`.
const MAX_NAME_BYTES: usize = 255;
const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;

/// Holds the (possibly absent) telemetry-DB pool for the analytics-key
/// routes. `None` only when `MICROMEGAS_SQL_CONNECTION_STRING` is unset — the
/// routes stay registered either way and return 503 per-request in that case,
/// the same always-register-503-when-unconfigured shape `maps::MapsState`'s
/// `Option<Arc<dyn ObjectStore>>` already uses. An unmigrated DB (missing the
/// v5 migration's tables) is a separate failure mode: the pool is still
/// `Some`, and a request fails with a 500 at query time instead.
#[derive(Clone)]
pub struct AnalyticsKeysState {
    pub pool: Option<PgPool>,
}

/// JSON error body returned by every handler in this module. Same
/// `{code, message}` shape as `data_sources.rs::ErrorResponse`, redefined
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
pub enum AnalyticsKeyError {
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

impl IntoResponse for AnalyticsKeyError {
    fn into_response(self) -> Response {
        match self {
            AnalyticsKeyError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new("BAD_REQUEST", msg)),
            )
                .into_response(),
            AnalyticsKeyError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse::new("NOT_FOUND", "key not found")),
            )
                .into_response(),
            AnalyticsKeyError::Database(err) => {
                error!("analytics_keys: database error: {err}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new(
                        "DATABASE_ERROR",
                        "internal database error",
                    )),
                )
                    .into_response()
            }
            AnalyticsKeyError::NotConfigured => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "NOT_CONFIGURED",
                    "analytics key store not configured: set MICROMEGAS_SQL_CONNECTION_STRING",
                )),
            )
                .into_response(),
        }
    }
}

impl From<sqlx::Error> for AnalyticsKeyError {
    fn from(err: sqlx::Error) -> Self {
        AnalyticsKeyError::Database(err)
    }
}

fn require_pool(state: &AnalyticsKeysState) -> Result<PgPool, AnalyticsKeyError> {
    state.pool.clone().ok_or(AnalyticsKeyError::NotConfigured)
}

fn validate_name(name: &str) -> Result<(), AnalyticsKeyError> {
    if name.is_empty() {
        return Err(AnalyticsKeyError::BadRequest(
            "name must not be empty".to_string(),
        ));
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(AnalyticsKeyError::BadRequest(format!(
            "name must be at most {MAX_NAME_BYTES} bytes"
        )));
    }
    Ok(())
}

#[derive(Deserialize)]
struct MintRequest {
    name: String,
}

#[derive(Serialize)]
struct MintResponse {
    key_id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    /// The cleartext key, returned exactly once. Never logged, never
    /// retrievable afterwards.
    key: String,
}

/// `POST {base_path}/api/analytics-api-keys` — mints a new
/// `analytics_api_keys` row.
async fn mint_key(
    Extension(state): Extension<AnalyticsKeysState>,
    AdminUser(user): AdminUser,
    Json(body): Json<MintRequest>,
) -> Result<(StatusCode, Json<MintResponse>), AnalyticsKeyError> {
    let pool = require_pool(&state)?;
    validate_name(&body.name)?;

    let key = generate_key();
    let hash = hash_key(&key);
    let key_id = Uuid::new_v4();
    let created_at = Utc::now();
    let created_by = user.email.clone().unwrap_or_else(|| user.subject.clone());

    // Table name is a literal, never derived from caller input: no route in
    // this module ever writes to `ingestion_api_keys`.
    sqlx::query(
        "INSERT INTO analytics_api_keys (key_id, key_hash, name, created_at, created_by)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(key_id)
    .bind(&hash[..])
    .bind(&body.name)
    .bind(created_at)
    .bind(&created_by)
    .execute(&pool)
    .await?;

    info!(
        "minted analytics api key key_id={key_id} name={} created_by={created_by}",
        body.name
    );

    Ok((
        StatusCode::CREATED,
        Json(MintResponse {
            key_id,
            name: body.name,
            created_at,
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
}

/// `GET {base_path}/api/analytics-api-keys?limit=&offset=&include_revoked=` —
/// lists `analytics_api_keys` rows, newest first. Never `key_hash`, never the
/// key.
async fn list_keys(
    Extension(state): Extension<AnalyticsKeysState>,
    AdminUser(_user): AdminUser,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<KeyListEntry>>, AnalyticsKeyError> {
    let pool = require_pool(&state)?;

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if limit <= 0 {
        return Err(AnalyticsKeyError::BadRequest(
            "limit must be > 0".to_string(),
        ));
    }
    // A read endpoint, so capping is safer than erroring: silently clamp
    // rather than reject values above MAX_LIMIT.
    let limit = limit.min(MAX_LIMIT);
    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(AnalyticsKeyError::BadRequest(
            "offset must be >= 0".to_string(),
        ));
    }
    let include_revoked = query.include_revoked.unwrap_or(true);

    let rows = if include_revoked {
        sqlx::query_as::<_, KeyListEntry>(
            "SELECT key_id, name, created_at, created_by, last_used_at, revoked_at, revoked_by
             FROM analytics_api_keys
             ORDER BY created_at DESC
             LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&pool)
        .await?
    } else {
        sqlx::query_as::<_, KeyListEntry>(
            "SELECT key_id, name, created_at, created_by, last_used_at, revoked_at, revoked_by
             FROM analytics_api_keys
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

/// `DELETE {base_path}/api/analytics-api-keys/{key_id}` — idempotent in one
/// statement, preserving the original revocation time on a repeat call.
///
/// No `effective_within_seconds` field, unlike ingestion's `revoke_key`: that
/// field threads the *validating* provider's `cache_ttl_secs`, but nothing in
/// `analytics-web-srv` runs a `DbApiKeyAuthProvider` — there is no running
/// cache TTL here to report. The revocation latency is still bounded by
/// whichever `flight-sql` process's cache TTL is validating the key,
/// documented in the runbook rather than echoed by this response.
async fn revoke_key(
    Extension(state): Extension<AnalyticsKeysState>,
    AdminUser(user): AdminUser,
    Path(key_id): Path<Uuid>,
) -> Result<Json<RevokeResponse>, AnalyticsKeyError> {
    let pool = require_pool(&state)?;
    let revoked_by = user.email.clone().unwrap_or_else(|| user.subject.clone());

    let row = sqlx::query(
        "UPDATE analytics_api_keys
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
            info!("revoked analytics api key key_id={key_id} revoked_by={revoked_by}");
            Ok(Json(RevokeResponse { revoked_at }))
        }
        None => Err(AnalyticsKeyError::NotFound),
    }
}

#[derive(Deserialize)]
struct ImportRequest {
    name: String,
    key: String,
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
}

/// `POST {base_path}/api/analytics-api-keys/import` — same shape as
/// `ingestion_keys.rs`'s import route: hashes and stores a caller-supplied
/// key string verbatim, rather than generating a fresh one. `created_by` is
/// the importing caller's own OIDC identity, never the literal string
/// `"import"`.
///
/// No format validation on `key` beyond non-empty: `hash_key` covers the whole
/// string regardless of shape, which is what lets an operator-chosen legacy
/// key of any format import cleanly.
async fn import_key(
    Extension(state): Extension<AnalyticsKeysState>,
    AdminUser(user): AdminUser,
    Json(body): Json<ImportRequest>,
) -> Result<(StatusCode, Json<ImportResponse>), AnalyticsKeyError> {
    let pool = require_pool(&state)?;
    validate_name(&body.name)?;
    if body.key.is_empty() {
        return Err(AnalyticsKeyError::BadRequest(
            "key must not be empty".to_string(),
        ));
    }

    let hash = hash_key(&body.key);
    let key_id = Uuid::new_v4();
    let created_at = Utc::now();
    let created_by = user.email.clone().unwrap_or_else(|| user.subject.clone());

    let inserted = sqlx::query_as::<_, ImportedRow>(
        "INSERT INTO analytics_api_keys (key_id, key_hash, name, created_at, created_by)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (key_hash) DO NOTHING
         RETURNING key_id, name, created_at, created_by, revoked_at",
    )
    .bind(key_id)
    .bind(&hash[..])
    .bind(&body.name)
    .bind(created_at)
    .bind(&created_by)
    .fetch_optional(&pool)
    .await?;

    let (row, imported, status) = match inserted {
        Some(row) => (row, true, StatusCode::CREATED),
        None => {
            // The hash already exists: report the existing row (including
            // whether it's revoked) instead of the freshly-generated values
            // above, which never made it into the table.
            let row = sqlx::query_as::<_, ImportedRow>(
                "SELECT key_id, name, created_at, created_by, revoked_at
                 FROM analytics_api_keys
                 WHERE key_hash = $1",
            )
            .bind(&hash[..])
            .fetch_one(&pool)
            .await?;
            (row, false, StatusCode::OK)
        }
    };

    info!(
        "imported analytics api key key_id={} name={} created_by={} imported={imported}",
        row.key_id, row.name, row.created_by
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
        }),
    ))
}

/// Routes only — [`AnalyticsKeysState`] is layered separately in
/// `web_server.rs::build_protected_routes`, the same way `app_db_pool`/
/// `maps_state` are.
pub fn analytics_keys_router(base_path: &str) -> Router {
    Router::new()
        .route(
            &format!("{base_path}/api/analytics-api-keys"),
            post(mint_key).get(list_keys),
        )
        .route(
            &format!("{base_path}/api/analytics-api-keys/{{key_id}}"),
            delete(revoke_key),
        )
        .route(
            &format!("{base_path}/api/analytics-api-keys/import"),
            post(import_key),
        )
}
