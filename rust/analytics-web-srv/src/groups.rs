//! Group admin API for `analytics-web-srv`.
//!
//! Mirrors `analytics_keys.rs`/`audience_grants.rs`'s shape (`GroupsState { pool: Option<PgPool> }`,
//! an `IntoResponse` error enum, `AdminUser`-gated) over the `groups`/`group_members` tables
//! (migration v10, `rust/ingestion/src/sql_migration.rs`). Every write goes through the
//! [`AdminUser`] extractor's gate -- documented as [`can_manage_group`] -- which is
//! `caller.is_admin` today; delegating group ownership later means widening that one function,
//! not this module's routing or handler shapes.

use crate::auth::{AdminUser, ValidatedUser};
use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use micromegas::auth::db_snapshot::SnapshotLoader;
use micromegas::auth::groups::{ADMINS_GROUP, GroupsLoader, is_valid_group_name};
use micromegas::auth::policy::valid_selector;
use micromegas::tracing::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// `valid_selector` places no charset/length bound on a `group:<id>` member (mirrors
/// `audience_grants.rs`'s identical constant/reasoning) -- the `member` column is `VARCHAR(255)`.
const MAX_MEMBER_BYTES: usize = 255;

/// Holds the (possibly absent) telemetry-DB pool for the group-admin routes. `None` only when
/// `MICROMEGAS_SQL_CONNECTION_STRING` is unset -- the routes stay registered either way and
/// return 503 per-request in that case, the same shape `AudienceGrantsState`/`AnalyticsKeysState`
/// use.
#[derive(Clone)]
pub struct GroupsState {
    pub pool: Option<PgPool>,
}

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
/// No `Forbidden` variant: the admin gate is the [`AdminUser`] extractor, whose rejection
/// renders as `AdminRequired`'s own 403 body before any handler here starts running.
pub enum GroupsError {
    BadRequest(String),
    NotFound,
    /// `409` -- the group already exists, is `admins` (undeletable), is still referenced, would
    /// create a cycle, or would remove the last row of `admins`. The message names which.
    Conflict(String),
    Database(sqlx::Error),
    NotConfigured,
    /// A non-DB internal error -- e.g. `GroupsLoader::fetch`'s snapshot query failing.
    Internal(String),
}

impl IntoResponse for GroupsError {
    fn into_response(self) -> Response {
        match self {
            GroupsError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new("BAD_REQUEST", msg)),
            )
                .into_response(),
            GroupsError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse::new("NOT_FOUND", "group not found")),
            )
                .into_response(),
            GroupsError::Conflict(msg) => (
                StatusCode::CONFLICT,
                Json(ErrorResponse::new("CONFLICT", msg)),
            )
                .into_response(),
            GroupsError::Database(err) => {
                error!("groups: database error: {err}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new(
                        "DATABASE_ERROR",
                        "internal database error",
                    )),
                )
                    .into_response()
            }
            GroupsError::NotConfigured => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "NOT_CONFIGURED",
                    "group store not configured: set MICROMEGAS_SQL_CONNECTION_STRING",
                )),
            )
                .into_response(),
            GroupsError::Internal(msg) => {
                error!("groups: internal error: {msg}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new("INTERNAL_ERROR", "internal error")),
                )
                    .into_response()
            }
        }
    }
}

impl From<sqlx::Error> for GroupsError {
    fn from(err: sqlx::Error) -> Self {
        GroupsError::Database(err)
    }
}

fn require_pool(state: &GroupsState) -> Result<PgPool, GroupsError> {
    state.pool.clone().ok_or(GroupsError::NotConfigured)
}

/// The one write predicate every handler here is gated on: `caller.is_admin` today, enforced
/// structurally by the [`AdminUser`] extractor before any handler body runs (so this function is
/// currently a no-op restatement of that gate, kept as the documented seam). Two-sided
/// authorization from the start -- editing membership requires authority over the *group* (this
/// predicate); granting an audience to `group:X` still requires authority over the *audience*
/// (`audience_grants.rs`'s `caller_holds_pair`). Delegating group ownership later means widening
/// this one function.
#[allow(dead_code)]
fn can_manage_group(caller: &ValidatedUser, _group: &str) -> bool {
    caller.is_admin
}

fn validate_group_name(name: &str) -> Result<(), GroupsError> {
    if !is_valid_group_name(name) {
        return Err(GroupsError::BadRequest(format!(
            "invalid group name {name:?}: must match [A-Za-z0-9_-]{{1,255}}"
        )));
    }
    Ok(())
}

/// `caller`'s own identity for `created_by` -- the same resolution every admin route in this
/// crate uses.
fn caller_identity(caller: &ValidatedUser) -> String {
    caller
        .email
        .clone()
        .unwrap_or_else(|| caller.subject.clone())
}

#[derive(Serialize, sqlx::FromRow)]
struct GroupSummary {
    name: String,
    description: Option<String>,
    member_count: i64,
    created_at: DateTime<Utc>,
    created_by: String,
}

/// `GET {base_path}/api/groups` -- every group with its member count.
async fn list_groups(
    Extension(state): Extension<GroupsState>,
    AdminUser(_user): AdminUser,
) -> Result<Json<Vec<GroupSummary>>, GroupsError> {
    let pool = require_pool(&state)?;
    let rows = sqlx::query_as::<_, GroupSummary>(
        "SELECT g.name, g.description, g.created_at, g.created_by,
                COUNT(m.member) AS member_count
         FROM groups g
         LEFT JOIN group_members m ON m.group_name = g.name
         GROUP BY g.name, g.description, g.created_at, g.created_by
         ORDER BY g.name",
    )
    .fetch_all(&pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct CreateGroupRequest {
    name: String,
    description: Option<String>,
}

/// `POST {base_path}/api/groups` -- creates a new, empty group. `400` on a malformed name, `409`
/// if it already exists.
async fn create_group(
    Extension(state): Extension<GroupsState>,
    AdminUser(user): AdminUser,
    Json(body): Json<CreateGroupRequest>,
) -> Result<(StatusCode, Json<GroupSummary>), GroupsError> {
    let pool = require_pool(&state)?;
    validate_group_name(&body.name)?;
    let created_by = caller_identity(&user);

    let existing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM groups WHERE name = $1)")
        .bind(&body.name)
        .fetch_one(&pool)
        .await?;
    if existing {
        return Err(GroupsError::Conflict(format!(
            "group {:?} already exists",
            body.name
        )));
    }

    sqlx::query(
        "INSERT INTO groups (name, description, created_at, created_by) VALUES ($1, $2, now(), $3)",
    )
    .bind(&body.name)
    .bind(&body.description)
    .bind(&created_by)
    .execute(&pool)
    .await?;

    let row = sqlx::query_as::<_, GroupSummary>(
        "SELECT name, description, 0::bigint AS member_count, created_at, created_by
         FROM groups WHERE name = $1",
    )
    .bind(&body.name)
    .fetch_one(&pool)
    .await?;

    info!("created group name={} created_by={created_by}", row.name);
    Ok((StatusCode::CREATED, Json(row)))
}

/// `DELETE {base_path}/api/groups/{name}` -- `204`; `409` for `admins`; `409` while referenced by
/// any `group_members.member = 'group:<name>'` or `audience_grants.selector = 'group:<name>'`
/// row (the response lists the referrers).
async fn delete_group(
    Extension(state): Extension<GroupsState>,
    AdminUser(_user): AdminUser,
    Path(name): Path<String>,
) -> Result<StatusCode, GroupsError> {
    let pool = require_pool(&state)?;
    if name == ADMINS_GROUP {
        return Err(GroupsError::Conflict(
            "the admins group cannot be deleted".to_string(),
        ));
    }

    let nesting_selector = format!("group:{name}");
    let nested_into: Vec<String> = sqlx::query_scalar(
        "SELECT group_name FROM group_members WHERE member = $1 ORDER BY group_name",
    )
    .bind(&nesting_selector)
    .fetch_all(&pool)
    .await?;
    let granted_to: Vec<String> = sqlx::query_scalar(
        "SELECT audience FROM audience_grants WHERE selector = $1 ORDER BY audience",
    )
    .bind(&nesting_selector)
    .fetch_all(&pool)
    .await?;
    if !nested_into.is_empty() || !granted_to.is_empty() {
        let mut referrers = Vec::new();
        if !nested_into.is_empty() {
            referrers.push(format!("nested into: {}", nested_into.join(", ")));
        }
        if !granted_to.is_empty() {
            referrers.push(format!("granted audiences: {}", granted_to.join(", ")));
        }
        return Err(GroupsError::Conflict(format!(
            "group {name:?} is still referenced ({})",
            referrers.join("; ")
        )));
    }

    let rows_affected = sqlx::query("DELETE FROM groups WHERE name = $1")
        .bind(&name)
        .execute(&pool)
        .await?
        .rows_affected();
    if rows_affected == 0 {
        return Err(GroupsError::NotFound);
    }
    info!("deleted group name={name}");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow)]
struct MemberRow {
    group_name: String,
    member: String,
    created_at: DateTime<Utc>,
    created_by: String,
}

/// `GET {base_path}/api/groups/{name}/members`.
async fn list_members(
    Extension(state): Extension<GroupsState>,
    AdminUser(_user): AdminUser,
    Path(name): Path<String>,
) -> Result<Json<Vec<MemberRow>>, GroupsError> {
    let pool = require_pool(&state)?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM groups WHERE name = $1)")
        .bind(&name)
        .fetch_one(&pool)
        .await?;
    if !exists {
        return Err(GroupsError::NotFound);
    }
    let rows = sqlx::query_as::<_, MemberRow>(
        "SELECT group_name, member, created_at, created_by
         FROM group_members WHERE group_name = $1 ORDER BY member",
    )
    .bind(&name)
    .fetch_all(&pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct AddMemberRequest {
    member: String,
}

/// `POST {base_path}/api/groups/{name}/members` -- validates `valid_selector` + 255 bytes; for
/// `group:X`, `404` if `X` does not exist and `409` if `nesting_would_cycle`; `201` created /
/// `200` already existed, via the same CTE `insert_or_get` UPSERT pattern `audience_grants.rs`
/// uses (see that module's `insert_or_get` doc comment).
async fn add_member(
    Extension(state): Extension<GroupsState>,
    AdminUser(user): AdminUser,
    Path(name): Path<String>,
    Json(body): Json<AddMemberRequest>,
) -> Result<(StatusCode, Json<MemberRow>), GroupsError> {
    let pool = require_pool(&state)?;
    if !valid_selector(&body.member) {
        return Err(GroupsError::BadRequest(format!(
            "invalid member {:?}: must be '*', 'user:<id>', or 'group:<id>'",
            body.member
        )));
    }
    if body.member.len() > MAX_MEMBER_BYTES {
        return Err(GroupsError::BadRequest(format!(
            "member must be at most {MAX_MEMBER_BYTES} bytes"
        )));
    }
    let group_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM groups WHERE name = $1)")
            .bind(&name)
            .fetch_one(&pool)
            .await?;
    if !group_exists {
        return Err(GroupsError::NotFound);
    }

    if let Some(nested) = body.member.strip_prefix("group:") {
        let nested_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM groups WHERE name = $1)")
                .bind(nested)
                .fetch_one(&pool)
                .await?;
        if !nested_exists {
            return Err(GroupsError::NotFound);
        }
        // Queried fresh (never the TTL snapshot): a stale snapshot could accept a cycle another
        // replica just refused; the read-time visited set in `GroupGraph::closure` covers the
        // race that remains.
        let graph = GroupsLoader::fetch(&pool)
            .await
            .map_err(|e| GroupsError::Internal(format!("querying the group graph: {e:#}")))?;
        if graph.nesting_would_cycle(&name, nested) {
            return Err(GroupsError::Conflict(format!(
                "adding {:?} as a member of {name:?} would create a cycle",
                body.member
            )));
        }
    }

    let created_by = caller_identity(&user);
    for _ in 0..2 {
        let row = sqlx::query_as::<_, UpsertedMemberRow>(
            "WITH ins AS (
                INSERT INTO group_members (group_name, member, created_at, created_by)
                VALUES ($1, $2, now(), $3)
                ON CONFLICT (group_name, member) DO NOTHING
                RETURNING group_name, member, created_at, created_by
             )
             SELECT group_name, member, created_at, created_by, true AS created FROM ins
             UNION ALL
             SELECT group_name, member, created_at, created_by, false AS created
             FROM group_members
             WHERE group_name = $1 AND member = $2
               AND NOT EXISTS (SELECT 1 FROM ins)",
        )
        .bind(&name)
        .bind(&body.member)
        .bind(&created_by)
        .fetch_optional(&pool)
        .await?;
        if let Some(row) = row {
            info!(
                "group member added group={} member={} created={} created_by={created_by}",
                row.group_name, row.member, row.created
            );
            let status = if row.created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            return Ok((
                status,
                Json(MemberRow {
                    group_name: row.group_name,
                    member: row.member,
                    created_at: row.created_at,
                    created_by: row.created_by,
                }),
            ));
        }
    }
    Err(GroupsError::Internal(format!(
        "group_members upsert for ({name:?}, {:?}) returned no row after a retry",
        body.member
    )))
}

/// Row shape shared by both branches of `add_member`'s CTE -- mirrors
/// `audience_grants.rs::UpsertedRow`'s reasoning (a single statement that can return zero rows on
/// a create/create race; the caller retries once, then treats a second zero-row result as
/// internal).
#[derive(sqlx::FromRow)]
struct UpsertedMemberRow {
    group_name: String,
    member: String,
    created_at: DateTime<Utc>,
    created_by: String,
    created: bool,
}

#[derive(Deserialize)]
struct RemoveMemberQuery {
    member: String,
}

/// `DELETE {base_path}/api/groups/{name}/members?member=` -- `204`; `404` unknown; `409` when it
/// would remove the last row of `admins` (the only lockout protection: removing it would leave
/// admin reachable only through `psql`).
async fn remove_member(
    Extension(state): Extension<GroupsState>,
    AdminUser(_user): AdminUser,
    Path(name): Path<String>,
    Query(query): Query<RemoveMemberQuery>,
) -> Result<StatusCode, GroupsError> {
    let pool = require_pool(&state)?;

    if name == ADMINS_GROUP {
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM group_members WHERE group_name = $1")
                .bind(&name)
                .fetch_one(&pool)
                .await?;
        let this_row_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM group_members WHERE group_name = $1 AND member = $2)",
        )
        .bind(&name)
        .bind(&query.member)
        .fetch_one(&pool)
        .await?;
        if this_row_exists && remaining <= 1 {
            return Err(GroupsError::Conflict(
                "removing the last member of admins would leave admin reachable only through \
                 direct database access"
                    .to_string(),
            ));
        }
    }

    let rows_affected =
        sqlx::query("DELETE FROM group_members WHERE group_name = $1 AND member = $2")
            .bind(&name)
            .bind(&query.member)
            .execute(&pool)
            .await?
            .rows_affected();
    if rows_affected == 0 {
        return Err(GroupsError::NotFound);
    }
    info!("group member removed group={name} member={}", query.member);
    Ok(StatusCode::NO_CONTENT)
}

/// Routes only -- [`GroupsState`] is layered separately in `web_server.rs::build_protected_routes`.
pub fn groups_router(base_path: &str) -> Router {
    Router::new()
        .route(
            &format!("{base_path}/api/groups"),
            get(list_groups).post(create_group),
        )
        .route(
            &format!("{base_path}/api/groups/{{name}}"),
            axum::routing::delete(delete_group),
        )
        .route(
            &format!("{base_path}/api/groups/{{name}}/members"),
            get(list_members).post(add_member).delete(remove_member),
        )
}
