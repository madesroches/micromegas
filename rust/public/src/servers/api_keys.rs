//! Key-management API for the ingestion service (#1383, extended by #1411):
//! four OIDC-authenticated, admin-gated HTTP routes that let an operator
//! mint, list, revoke, and import `ingestion_api_keys` rows without a
//! redeploy.
//!
//! **Analytics keys are not minted through this API.** They are minted,
//! listed, revoked, and imported through `analytics-web-srv`'s own
//! `/api/analytics-api-keys*` routes instead (#1411) — issuing read
//! credentials from the fleet-facing ingestion service would be the wrong
//! direction for the write/read asymmetry. See `mkdocs/docs/admin/api-keys.md`
//! for the analytics-key HTTP-route reference.

use axum::extract::{Extension, Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use micromegas_auth::db_api_key::{DbApiKeyConfig, generate_key, hash_key};
use micromegas_auth::types::{AuthContext, AuthType};
use micromegas_tracing::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Bytes, not chars: deliberately stricter than the `VARCHAR(255)` column, which
/// bounds characters.
const MAX_NAME_BYTES: usize = 255;
const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;

/// Header `analytics-web-srv`'s ingestion-key proxy (`ingestion_keys_proxy.rs`)
/// sets to carry the identity of the human admin it is acting on behalf of.
///
/// Every proxied call reaches this module authenticated as the proxy's own
/// `MICROMEGAS_INGESTION_PROXY_OIDC_*` service credential, not the operator —
/// without this header, `actor()` would attribute every mint/revoke performed
/// through the web UI to that one constant service-account identity instead of
/// the admin who actually clicked the button, destroying the audit trail.
///
/// Only consulted by `actor()` *after* `require_key_admin` has already passed
/// for the request's own `AuthContext` (OIDC + `is_admin`) — an arbitrary
/// caller cannot use this header to spoof an identity, since it is ignored
/// for any non-admin or non-OIDC caller.
pub const ON_BEHALF_OF_HEADER: &str = "x-micromegas-on-behalf-of";

#[derive(Debug, Serialize)]
struct ErrorBody {
    message: String,
}

/// Errors this API returns. Modeled on
/// `rust/analytics-web-srv/src/data_sources.rs`'s `DataSourceError` precedent.
pub enum ApiKeyError {
    /// Caller is not an OIDC identity; API keys cannot manage keys.
    NotOidc,
    /// OIDC identity is not in the admin list.
    NotAdmin,
    /// Request body/query failed validation.
    BadRequest(String),
    /// Unknown `key_id`.
    NotFound,
    /// A DB error.
    Database(sqlx::Error),
}

impl IntoResponse for ApiKeyError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiKeyError::NotOidc => (
                StatusCode::FORBIDDEN,
                "caller is not an OIDC identity; API keys cannot manage keys".to_string(),
            ),
            ApiKeyError::NotAdmin => (
                StatusCode::FORBIDDEN,
                "OIDC identity is not in the admin list".to_string(),
            ),
            ApiKeyError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiKeyError::NotFound => (StatusCode::NOT_FOUND, "key not found".to_string()),
            ApiKeyError::Database(e) => {
                error!("api_keys: database error: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal database error".to_string(),
                )
            }
        };
        (status, Json(ErrorBody { message })).into_response()
    }
}

impl From<sqlx::Error> for ApiKeyError {
    fn from(err: sqlx::Error) -> Self {
        ApiKeyError::Database(err)
    }
}

/// Checked first in every handler: rejects any non-OIDC `auth_type` outright
/// (redundant with `is_admin: false` on key contexts, but states the rule
/// directly — no API key can manage keys), then requires `is_admin`. Distinct
/// variants/messages so an operator's 403 tells them which condition failed.
fn require_key_admin(ctx: &AuthContext) -> Result<(), ApiKeyError> {
    if ctx.auth_type != AuthType::Oidc {
        return Err(ApiKeyError::NotOidc);
    }
    if !ctx.is_admin {
        return Err(ApiKeyError::NotAdmin);
    }
    Ok(())
}

/// Resolves the identity to attribute a mutation to.
///
/// Ordinarily the authenticated caller's own email/subject — but once the
/// caller has already passed `require_key_admin` (an OIDC-authenticated
/// admin), an `on_behalf_of` value (from [`ON_BEHALF_OF_HEADER`]) is
/// preferred instead when present. This is what lets `analytics-web-srv`'s
/// ingestion-key proxy — which calls in under its own service-credential
/// identity — attribute `created_by`/`revoked_by` to the human admin who
/// actually clicked mint/revoke in the browser.
///
/// Gating on `require_key_admin` having already passed is what keeps a
/// non-admin (or non-OIDC) caller from spoofing an arbitrary identity by
/// simply setting this header itself: the header is only ever *read* here,
/// never treated as its own authentication signal.
fn actor(ctx: &AuthContext, on_behalf_of: Option<&str>) -> String {
    if ctx.auth_type == AuthType::Oidc
        && ctx.is_admin
        && let Some(v) = on_behalf_of.map(str::trim).filter(|v| !v.is_empty())
    {
        return v.to_string();
    }
    ctx.email.clone().unwrap_or_else(|| ctx.subject.clone())
}

/// Reads [`ON_BEHALF_OF_HEADER`] out of the request, if present.
fn on_behalf_of_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(ON_BEHALF_OF_HEADER)
        .and_then(|v| v.to_str().ok())
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
    /// The cleartext key, returned exactly once. Never logged, never retrievable
    /// afterwards.
    key: String,
}

/// `POST /auth/api_keys` — mints a new `ingestion_api_keys` row.
async fn mint_key(
    Extension(ctx): Extension<AuthContext>,
    Extension(pool): Extension<PgPool>,
    headers: HeaderMap,
    Json(body): Json<MintRequest>,
) -> Result<(StatusCode, Json<MintResponse>), ApiKeyError> {
    require_key_admin(&ctx)?;

    if body.name.is_empty() {
        return Err(ApiKeyError::BadRequest(
            "name must not be empty".to_string(),
        ));
    }
    if body.name.len() > MAX_NAME_BYTES {
        return Err(ApiKeyError::BadRequest(format!(
            "name must be at most {MAX_NAME_BYTES} bytes"
        )));
    }

    let key = generate_key();
    let hash = hash_key(&key);
    let key_id = Uuid::new_v4();
    let created_at = Utc::now();
    let created_by = actor(&ctx, on_behalf_of_header(&headers));

    // Table name is a literal, never derived from caller input: no route in this
    // module ever writes to `analytics_api_keys` (see the module doc comment).
    sqlx::query(
        "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by)
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
        "minted ingestion api key key_id={key_id} name={} created_by={created_by}",
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

/// `GET /auth/api_keys?limit=&offset=&include_revoked=` — lists `ingestion_api_keys`
/// rows, newest first. Never `key_hash`, never the key.
async fn list_keys(
    Extension(ctx): Extension<AuthContext>,
    Extension(pool): Extension<PgPool>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<KeyListEntry>>, ApiKeyError> {
    require_key_admin(&ctx)?;

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if limit <= 0 {
        return Err(ApiKeyError::BadRequest("limit must be > 0".to_string()));
    }
    // A read endpoint, so capping is safer than erroring: silently clamp rather
    // than reject values above MAX_LIMIT.
    let limit = limit.min(MAX_LIMIT);
    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(ApiKeyError::BadRequest("offset must be >= 0".to_string()));
    }
    let include_revoked = query.include_revoked.unwrap_or(true);

    let rows = if include_revoked {
        sqlx::query_as::<_, KeyListEntry>(
            "SELECT key_id, name, created_at, created_by, last_used_at, revoked_at, revoked_by
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
            "SELECT key_id, name, created_at, created_by, last_used_at, revoked_at, revoked_by
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
    /// Revocation is not instantaneous: a remote process's positive cache may
    /// keep authenticating this key for up to this many seconds after the
    /// response below. Mirrors the `cache_ttl_secs` the *running* provider was
    /// actually built with (threaded in via `api_keys_router`'s `config`
    /// parameter), so this can never silently disagree with reality.
    effective_within_seconds: u64,
}

/// `DELETE /auth/api_keys/{key_id}` — idempotent in one statement, preserving the
/// original revocation time on a repeat call.
async fn revoke_key(
    Extension(ctx): Extension<AuthContext>,
    Extension(pool): Extension<PgPool>,
    Extension(config): Extension<DbApiKeyConfig>,
    headers: HeaderMap,
    Path(key_id): Path<Uuid>,
) -> Result<Json<RevokeResponse>, ApiKeyError> {
    require_key_admin(&ctx)?;

    let revoked_by = actor(&ctx, on_behalf_of_header(&headers));
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
            Ok(Json(RevokeResponse {
                revoked_at,
                effective_within_seconds: config.cache_ttl_secs,
            }))
        }
        None => Err(ApiKeyError::NotFound),
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
    /// was itself revoked — lets a caller distinguish "already present and
    /// usable" from "already present but revoked".
    revoked_at: Option<DateTime<Utc>>,
    /// `true` on a fresh insert; `false` when `key_hash` already existed
    /// (idempotent re-run of the same legacy keyring).
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

/// `POST /auth/api_keys/import` — hashes and stores a caller-supplied key
/// string verbatim, rather than generating a fresh one (`mint_key`'s only
/// mode). This is what lets a legacy env-keyring key's own string carry
/// forward into the DB-backed store, since existing clients must keep
/// presenting the *same* key. `created_by` is the importing caller's own OIDC
/// identity (§3 of the plan) — never the literal string `"import"`.
///
/// No format validation on `key` beyond non-empty: `hash_key` covers the whole
/// string regardless of shape, which is what lets an operator-chosen legacy
/// key of any format import cleanly.
async fn import_key(
    Extension(ctx): Extension<AuthContext>,
    Extension(pool): Extension<PgPool>,
    Json(body): Json<ImportRequest>,
) -> Result<(StatusCode, Json<ImportResponse>), ApiKeyError> {
    require_key_admin(&ctx)?;

    if body.name.is_empty() {
        return Err(ApiKeyError::BadRequest(
            "name must not be empty".to_string(),
        ));
    }
    if body.name.len() > MAX_NAME_BYTES {
        return Err(ApiKeyError::BadRequest(format!(
            "name must be at most {MAX_NAME_BYTES} bytes"
        )));
    }
    if body.key.is_empty() {
        return Err(ApiKeyError::BadRequest("key must not be empty".to_string()));
    }

    let hash = hash_key(&body.key);
    let key_id = Uuid::new_v4();
    let created_at = Utc::now();
    // Import is never proxied (see the module doc comment), so there is no
    // on-behalf-of identity to prefer here — always the importing caller's
    // own OIDC identity, per this function's doc comment above.
    let created_by = actor(&ctx, None);

    // Table name is a literal, never derived from caller input — same rule as
    // mint_key.
    let inserted = sqlx::query_as::<_, ImportedRow>(
        "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by)
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
        "imported ingestion api key key_id={} name={} created_by={} imported={imported}",
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

/// Router for the ingestion key-management routes. Hardcodes
/// `ingestion_api_keys`: there is no parameter an operator or a defaulting bug
/// could point at `analytics_api_keys`. `config.cache_ttl_secs` is what the
/// `DELETE` response's `effective_within_seconds` reports; the caller
/// (`serve_ingestion_with_api_key_config`) must build it with the identical
/// prefix it gave `ProviderBuilder::with_db_key_store` so the two cannot
/// silently disagree.
///
/// Merge this into `protected_app` **before** the `auth_middleware` layer so it
/// reuses the existing middleware rather than re-implementing auth: handlers
/// read `Extension<AuthContext>`, inserted by `auth_middleware`.
pub fn api_keys_router(pool: PgPool, config: DbApiKeyConfig) -> Router {
    Router::new()
        .route("/auth/api_keys", post(mint_key).get(list_keys))
        .route("/auth/api_keys/{key_id}", delete(revoke_key))
        .route("/auth/api_keys/import", post(import_key))
        .layer(Extension(config))
        .layer(Extension(pool))
}
