//! Audience grant admin API for `analytics-web-srv` (#1489, AbAC Stage 6a).
//!
//! Directly mirrors `ingestion_keys.rs`'s shape (`AudienceGrantsState { pool: Option<PgPool> }`,
//! an `IntoResponse` error enum, `AdminUser`-gated handlers) over the new `audience_grants` table
//! (migration v7, `rust/ingestion/src/sql_migration.rs`). This is the admin write surface for the
//! grant store `micromegas-auth::db_audience_grants::DbAudienceGrantsSource` reads from -- the
//! store's own snapshot cache picks up rows written here within its cache TTL.
//!
//! **Duplication, accepted.** This duplicates most of `ingestion_keys.rs`/`analytics_keys.rs`'s
//! validation/SQL/error shape -- deliberately, per those modules' own doc comments: a generic
//! abstraction over a handful of near-identical handlers differing only in which table they
//! target is a shape this codebase already declines elsewhere
//! (`data_sources.rs`/`screens.rs`/`folders.rs`).

use crate::auth::AdminUser;
use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use micromegas::auth::policy::{is_valid_audience, valid_selector};
use micromegas::tracing::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;
/// `valid_selector` places no charset/length bound on a `group:<id>` selector (a hierarchical
/// IdP group name can be arbitrarily long), but the `selector` column is `VARCHAR(255)` -- this
/// check is what turns an over-long selector into a `400 BadRequest` instead of a `500` at the
/// `INSERT`.
const MAX_SELECTOR_BYTES: usize = 255;

/// Holds the (possibly absent) telemetry-DB pool for the audience-grant routes. `None` only when
/// `MICROMEGAS_SQL_CONNECTION_STRING` is unset -- the routes stay registered either way and
/// return 503 per-request in that case, the same always-register-503-when-unconfigured shape
/// `IngestionKeysState`/`AnalyticsKeysState` use.
#[derive(Clone)]
pub struct AudienceGrantsState {
    pub pool: Option<PgPool>,
}

/// JSON error body returned by every handler in this module. Same `{code, message}` shape as
/// `ingestion_keys.rs::ErrorResponse`, redefined here since that struct is private to its module.
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
/// (`auth/handlers.rs`), whose rejection renders as `AdminRequired`'s own 403 body -- before any
/// handler in this file even starts running -- so a `Forbidden` variant here would be dead code.
#[derive(Debug)]
pub enum AudienceGrantError {
    /// Request body/query failed validation.
    BadRequest(String),
    /// Unknown `(audience, axis, selector)` on `DELETE`.
    NotFound,
    /// A DB error.
    Database(sqlx::Error),
    /// `state.pool == None` -- the telemetry-DB pool was never configured
    /// (`MICROMEGAS_SQL_CONNECTION_STRING` unset).
    NotConfigured,
    /// The create statement (see [`insert_or_get`]) returned zero rows twice in a row -- an
    /// internal error, not a caller mistake (see that function's doc comment).
    Internal(String),
}

impl IntoResponse for AudienceGrantError {
    fn into_response(self) -> Response {
        match self {
            AudienceGrantError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new("BAD_REQUEST", msg)),
            )
                .into_response(),
            AudienceGrantError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse::new("NOT_FOUND", "grant not found")),
            )
                .into_response(),
            AudienceGrantError::Database(err) => {
                error!("audience_grants: database error: {err}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new(
                        "DATABASE_ERROR",
                        "internal database error",
                    )),
                )
                    .into_response()
            }
            AudienceGrantError::NotConfigured => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "NOT_CONFIGURED",
                    "audience grant store not configured: set MICROMEGAS_SQL_CONNECTION_STRING",
                )),
            )
                .into_response(),
            AudienceGrantError::Internal(msg) => {
                error!("audience_grants: internal error: {msg}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new("INTERNAL_ERROR", "internal error")),
                )
                    .into_response()
            }
        }
    }
}

impl From<sqlx::Error> for AudienceGrantError {
    fn from(err: sqlx::Error) -> Self {
        AudienceGrantError::Database(err)
    }
}

fn require_pool(state: &AudienceGrantsState) -> Result<PgPool, AudienceGrantError> {
    state.pool.clone().ok_or(AudienceGrantError::NotConfigured)
}

fn validate_audience(audience: &str) -> Result<(), AudienceGrantError> {
    if !is_valid_audience(audience) {
        return Err(AudienceGrantError::BadRequest(format!(
            "invalid audience {audience:?}: must match [A-Za-z0-9_-]{{1,255}}"
        )));
    }
    Ok(())
}

fn validate_axis(axis: &str) -> Result<(), AudienceGrantError> {
    if axis != "read" && axis != "mint" {
        return Err(AudienceGrantError::BadRequest(format!(
            "invalid axis {axis:?}: must be 'read' or 'mint'"
        )));
    }
    Ok(())
}

fn validate_selector(selector: &str) -> Result<(), AudienceGrantError> {
    if !valid_selector(selector) {
        return Err(AudienceGrantError::BadRequest(format!(
            "invalid selector {selector:?}: must be '*', 'user:<id>', or 'group:<id>'"
        )));
    }
    if selector.len() > MAX_SELECTOR_BYTES {
        return Err(AudienceGrantError::BadRequest(format!(
            "selector must be at most {MAX_SELECTOR_BYTES} bytes"
        )));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateGrantRequest {
    audience: String,
    axis: String,
    selector: String,
}

#[derive(Serialize)]
struct GrantResponse {
    audience: String,
    axis: String,
    selector: String,
    created_at: DateTime<Utc>,
    created_by: String,
}

/// Row shape shared by both branches of `insert_or_get`'s CTE (see its doc comment).
#[derive(sqlx::FromRow)]
struct UpsertedRow {
    audience: String,
    axis: String,
    selector: String,
    created_at: DateTime<Utc>,
    created_by: String,
    created: bool,
}

/// One round trip: a CTE that unions the just-inserted row with the pre-existing one, so there is
/// no window between a failed insert and a re-`SELECT` for a concurrent `DELETE` to invalidate --
/// unlike `ingestion_keys.rs::import_key`'s insert-then-re-`SELECT`, safe there only because that
/// table never physically deletes rows.
///
/// This single statement can still return **zero rows**: Postgres data-modifying CTEs share one
/// statement-level snapshot with the query around them, so when two callers race to create the
/// same new `(audience, axis, selector)`, the loser's `ins` branch resolves to "do nothing" (its
/// `INSERT ... ON CONFLICT` finds the winner's row already committed) while its plain-`SELECT`
/// branch still runs against the snapshot taken before the winner committed -- neither branch
/// sees the row. The caller retries the exact same statement once more (now that the winner's
/// insert has definitely committed, the loser's re-`SELECT` branch will see it); a second
/// zero-row result is treated as an internal error rather than looping further.
const UPSERT_GRANT_SQL: &str = "
    WITH ins AS (
        INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
        VALUES ($1, $2, $3, now(), $4)
        ON CONFLICT (audience, axis, selector) DO NOTHING
        RETURNING audience, axis, selector, created_at, created_by
    )
    SELECT audience, axis, selector, created_at, created_by, true AS created FROM ins
    UNION ALL
    SELECT audience, axis, selector, created_at, created_by, false AS created
    FROM audience_grants
    WHERE audience = $1 AND axis = $2 AND selector = $3
      AND NOT EXISTS (SELECT 1 FROM ins)";

async fn insert_or_get(
    pool: &PgPool,
    audience: &str,
    axis: &str,
    selector: &str,
    created_by: &str,
) -> Result<UpsertedRow, AudienceGrantError> {
    for _ in 0..2 {
        let row = sqlx::query_as::<_, UpsertedRow>(UPSERT_GRANT_SQL)
            .bind(audience)
            .bind(axis)
            .bind(selector)
            .bind(created_by)
            .fetch_optional(pool)
            .await?;
        if let Some(row) = row {
            return Ok(row);
        }
    }
    Err(AudienceGrantError::Internal(format!(
        "audience_grants upsert for ({audience:?}, {axis:?}, {selector:?}) returned no row \
         after a retry"
    )))
}

/// `POST {base_path}/api/audience-grants` -- creates (or reports the pre-existing) grant row.
/// `201` when this call created it, `200` when it already existed.
async fn create_grant(
    Extension(state): Extension<AudienceGrantsState>,
    AdminUser(user): AdminUser,
    Json(body): Json<CreateGrantRequest>,
) -> Result<(StatusCode, Json<GrantResponse>), AudienceGrantError> {
    let pool = require_pool(&state)?;
    validate_audience(&body.audience)?;
    validate_axis(&body.axis)?;
    validate_selector(&body.selector)?;

    let created_by = user.email.clone().unwrap_or_else(|| user.subject.clone());
    let row = insert_or_get(
        &pool,
        &body.audience,
        &body.axis,
        &body.selector,
        &created_by,
    )
    .await?;

    let status = if row.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    info!(
        "audience grant audience={} axis={} selector={} created={} created_by={}",
        row.audience, row.axis, row.selector, row.created, row.created_by
    );
    Ok((
        status,
        Json(GrantResponse {
            audience: row.audience,
            axis: row.axis,
            selector: row.selector,
            created_at: row.created_at,
            created_by: row.created_by,
        }),
    ))
}

#[derive(Deserialize)]
struct ListQuery {
    audience: Option<String>,
    axis: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
struct GrantListEntry {
    audience: String,
    axis: String,
    selector: String,
    created_at: DateTime<Utc>,
    created_by: String,
}

/// `GET {base_path}/api/audience-grants?audience=&axis=&limit=&offset=` -- lists rows, optionally
/// filtered, newest first. Admin-gated like the write side: revealing who can read which audience
/// is itself confidentiality-sensitive.
async fn list_grants(
    Extension(state): Extension<AudienceGrantsState>,
    AdminUser(_user): AdminUser,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<GrantListEntry>>, AudienceGrantError> {
    let pool = require_pool(&state)?;

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if limit <= 0 {
        return Err(AudienceGrantError::BadRequest(
            "limit must be > 0".to_string(),
        ));
    }
    // A read endpoint, so capping is safer than erroring: silently clamp rather than reject
    // values above MAX_LIMIT.
    let limit = limit.min(MAX_LIMIT);
    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(AudienceGrantError::BadRequest(
            "offset must be >= 0".to_string(),
        ));
    }
    if let Some(axis) = &query.axis {
        validate_axis(axis)?;
    }

    let rows = match (&query.audience, &query.axis) {
        (None, None) => {
            sqlx::query_as::<_, GrantListEntry>(
                "SELECT audience, axis, selector, created_at, created_by
                 FROM audience_grants
                 ORDER BY created_at DESC
                 LIMIT $1 OFFSET $2",
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(&pool)
            .await?
        }
        (Some(audience), None) => {
            sqlx::query_as::<_, GrantListEntry>(
                "SELECT audience, axis, selector, created_at, created_by
                 FROM audience_grants
                 WHERE audience = $1
                 ORDER BY created_at DESC
                 LIMIT $2 OFFSET $3",
            )
            .bind(audience)
            .bind(limit)
            .bind(offset)
            .fetch_all(&pool)
            .await?
        }
        (None, Some(axis)) => {
            sqlx::query_as::<_, GrantListEntry>(
                "SELECT audience, axis, selector, created_at, created_by
                 FROM audience_grants
                 WHERE axis = $1
                 ORDER BY created_at DESC
                 LIMIT $2 OFFSET $3",
            )
            .bind(axis)
            .bind(limit)
            .bind(offset)
            .fetch_all(&pool)
            .await?
        }
        (Some(audience), Some(axis)) => {
            sqlx::query_as::<_, GrantListEntry>(
                "SELECT audience, axis, selector, created_at, created_by
                 FROM audience_grants
                 WHERE audience = $1 AND axis = $2
                 ORDER BY created_at DESC
                 LIMIT $3 OFFSET $4",
            )
            .bind(audience)
            .bind(axis)
            .bind(limit)
            .bind(offset)
            .fetch_all(&pool)
            .await?
        }
    };

    Ok(Json(rows))
}

#[derive(Deserialize)]
struct DeleteGrantQuery {
    audience: String,
    axis: String,
    selector: String,
}

/// `DELETE {base_path}/api/audience-grants?audience=&axis=&selector=` -- natural key passed as
/// query parameters, not path segments: `valid_selector` places no charset restriction on a
/// `group:<id>` selector (a hierarchical IdP group name can contain `/`, `?`, or other
/// URL-significant characters), so encoding it as a raw path segment the way every other route's
/// `Uuid` id does would be unsafe here. `404` if no such row.
async fn delete_grant(
    Extension(state): Extension<AudienceGrantsState>,
    AdminUser(user): AdminUser,
    Query(query): Query<DeleteGrantQuery>,
) -> Result<StatusCode, AudienceGrantError> {
    let pool = require_pool(&state)?;
    validate_axis(&query.axis)?;

    let result = sqlx::query(
        "DELETE FROM audience_grants WHERE audience = $1 AND axis = $2 AND selector = $3",
    )
    .bind(&query.audience)
    .bind(&query.axis)
    .bind(&query.selector)
    .execute(&pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AudienceGrantError::NotFound);
    }

    let deleted_by = user.email.clone().unwrap_or_else(|| user.subject.clone());
    info!(
        "deleted audience grant audience={} axis={} selector={} deleted_by={deleted_by}",
        query.audience, query.axis, query.selector
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Routes only -- [`AudienceGrantsState`] is layered separately in
/// `web_server.rs::build_protected_routes`, the same way `analytics_keys_state`/
/// `ingestion_keys_state` are.
pub fn audience_grants_router(base_path: &str) -> Router {
    Router::new().route(
        &format!("{base_path}/api/audience-grants"),
        post(create_grant).get(list_grants).delete(delete_grant),
    )
}
