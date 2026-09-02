//! Group admin API for `analytics-web-srv`.
//!
//! Mirrors `analytics_keys.rs`/`audience_grants.rs`'s shape (`GroupsState { pool: Option<PgPool> }`,
//! an `IntoResponse` error enum, `AdminUser`-gated) over the `groups`/`group_members` tables
//! (migration v10, `rust/ingestion/src/sql_migration.rs`). Every write goes through the
//! [`AdminUser`] extractor's gate, which is `caller.is_admin` today; delegating group ownership
//! later means widening that one gate, not this module's routing or handler shapes.

use crate::auth::{AdminUser, ValidatedUser};
use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use micromegas::auth::groups::{ADMINS_GROUP, GroupGraph, is_valid_group_name};
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
    /// `add_member`'s `group:<name>` member names a nested group that does not exist. Distinct
    /// from `NotFound` (the *target* group missing) so the client can tell the two apart instead
    /// of both rendering as the same generic "group not found". The message names the missing
    /// nested group.
    NestedGroupNotFound(String),
    /// `409` -- the group already exists, is `admins` (undeletable), is still referenced, would
    /// create a cycle, or would leave `admins` unreachable by any principal. The message names
    /// which.
    Conflict(String),
    Database(sqlx::Error),
    NotConfigured,
    /// A non-DB internal error -- e.g. `GroupGraph::from_rows` rejecting a snapshot read
    /// in-transaction.
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
            GroupsError::NestedGroupNotFound(msg) => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse::new("NOT_FOUND", msg)),
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

    let row = sqlx::query_as::<_, GroupSummary>(
        "INSERT INTO groups (name, description, created_at, created_by)
         VALUES ($1, $2, now(), $3)
         ON CONFLICT (name) DO NOTHING
         RETURNING name, description, 0::bigint AS member_count, created_at, created_by",
    )
    .bind(&body.name)
    .bind(&body.description)
    .bind(&created_by)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| GroupsError::Conflict(format!("group {:?} already exists", body.name)))?;

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

    // The referrer checks and the delete run in one transaction, with the `groups` row for
    // `name` locked first: `add_member`'s nested-group check takes the same `FOR UPDATE` lock on
    // that row before inserting a `group:<name>` member row (see its own comment), so a
    // concurrent nest-in and this delete serialize on this row instead of racing -- without that,
    // a `group:<name>` reference inserted between the plain check and the delete would survive,
    // and silently re-activate if the name is later recreated.
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| GroupsError::Internal(format!("starting delete transaction: {e:#}")))?;
    sqlx::query("SELECT 1 FROM groups WHERE name = $1 FOR UPDATE")
        .bind(&name)
        .fetch_optional(&mut *tx)
        .await?;

    let nesting_selector = format!("group:{name}");
    let nested_into: Vec<String> = sqlx::query_scalar(
        "SELECT group_name FROM group_members WHERE member = $1 ORDER BY group_name",
    )
    .bind(&nesting_selector)
    .fetch_all(&mut *tx)
    .await?;
    let granted_to: Vec<String> = sqlx::query_scalar(
        "SELECT audience FROM audience_grants WHERE selector = $1 ORDER BY audience",
    )
    .bind(&nesting_selector)
    .fetch_all(&mut *tx)
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
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if rows_affected == 0 {
        return Err(GroupsError::NotFound);
    }
    tx.commit()
        .await
        .map_err(|e| GroupsError::Internal(format!("committing delete transaction: {e:#}")))?;
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

    // The nested-group existence check and the insert run in one transaction, with the `groups`
    // row(s) involved locked first: `delete_group` takes this same `FOR UPDATE` lock on its own
    // row before scanning for `group:<name>` referrers (see its own comment), so a concurrent
    // delete of the nested group and this insert serialize on that row instead of racing --
    // without that, a `group:<nested>` member row could commit after `delete_group`'s referrer
    // scan already found none, leaving `group_members` pointing at a group name that no longer
    // exists (its FK is on `group_name`, not `member`, so nothing else rejects the orphan).
    //
    // For a `group:<nested>` member, *both* the target's and the nested group's rows are locked
    // `FOR UPDATE` up front, in a fixed (lexicographic) order -- not just the nested row. The
    // insert's FK on `group_name` takes an implicit `FOR KEY SHARE` lock on the target's row,
    // which conflicts with `FOR UPDATE`; locking only the nested row here left
    // `add_member(A, "group:B")` (holds `FOR UPDATE` on B, wants `KEY SHARE` on A) able to
    // deadlock against a concurrent `add_member(B, "group:A")` (holds `FOR UPDATE` on A, wants
    // `KEY SHARE` on B). Every caller taking these two locks in the same order removes that
    // cycle.
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| GroupsError::Internal(format!("starting add-member transaction: {e:#}")))?;

    let nested = body.member.strip_prefix("group:");
    let mut lock_names: Vec<&str> = vec![&name];
    if let Some(nested) = nested {
        lock_names.push(nested);
    }
    lock_names.sort_unstable();
    lock_names.dedup();
    for lock_name in &lock_names {
        sqlx::query("SELECT 1 FROM groups WHERE name = $1 FOR UPDATE")
            .bind(lock_name)
            .fetch_optional(&mut *tx)
            .await?;
    }

    let group_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM groups WHERE name = $1)")
            .bind(&name)
            .fetch_one(&mut *tx)
            .await?;
    if !group_exists {
        return Err(GroupsError::NotFound);
    }

    if let Some(nested) = nested {
        let nested_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM groups WHERE name = $1)")
                .bind(nested)
                .fetch_one(&mut *tx)
                .await?;
        if !nested_exists {
            return Err(GroupsError::NestedGroupNotFound(format!(
                "group {nested:?} not found"
            )));
        }
        // Queried fresh (never the TTL snapshot) and read through this same transaction's
        // connection -- not a second `pool.begin()` via `GroupsLoader::fetch`, which would need a
        // second connection from this same (2-connection) pool while this one is still held,
        // deadlocking two concurrent nested-group adds against each other. Mirrors
        // `remove_member`'s own in-transaction graph read below. No need for `remove_member`'s
        // `admins`-lockout advisory lock here: this only ever adds a member, which can only add
        // reachability to a target, never remove it, so it cannot defeat that guard.
        let group_names: Vec<String> = sqlx::query_scalar("SELECT name FROM groups")
            .fetch_all(&mut *tx)
            .await?;
        let member_rows: Vec<(String, String)> =
            sqlx::query_as("SELECT group_name, member FROM group_members")
                .fetch_all(&mut *tx)
                .await?;
        let graph = GroupGraph::from_rows(group_names, member_rows)
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
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = row {
            tx.commit().await.map_err(|e| {
                GroupsError::Internal(format!("committing add-member transaction: {e:#}"))
            })?;
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

/// `DELETE {base_path}/api/groups/{name}/members?member=` -- `204`; `404` unknown; `409` when
/// removing this member would leave [`ADMINS_GROUP`] unreachable by any principal, direct or
/// nested (the only lockout protection: losing that reachability would leave admin reachable only
/// through `psql`).
async fn remove_member(
    Extension(state): Extension<GroupsState>,
    AdminUser(_user): AdminUser,
    Path(name): Path<String>,
    Query(query): Query<RemoveMemberQuery>,
) -> Result<StatusCode, GroupsError> {
    let pool = require_pool(&state)?;

    // The lockout guard and the delete run in one transaction. The guard is a whole-graph
    // reachability question (`any_principal_reaches(ADMINS_GROUP)`), so a lock on just `name`'s
    // rows isn't enough to serialize it: two concurrent removals from *different* groups that
    // both feed into `admins` (e.g. `admins = [group:eng, group:ops]`) would take disjoint
    // per-group locks, each build its post-removal graph from the other's uncommitted state,
    // both see `admins` still reachable, and both commit -- leaving `admins` reachable by
    // nobody. A transaction-scoped advisory lock on a fixed key, taken before the graph read,
    // closes that: every removal that could affect the guard serializes on this one lock,
    // regardless of which group it targets. `pg_advisory_xact_lock` auto-releases at
    // commit/rollback.
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| GroupsError::Internal(format!("starting remove-member transaction: {e:#}")))?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('micromegas_groups_admins_lockout'))")
        .execute(&mut *tx)
        .await?;

    // Also lock `name`'s own rows: unrelated to the cross-group race above (the advisory lock
    // already covers that), but still serializes this removal against `add_member`'s nested-group
    // insert into the same group name, matching `add_member`/`delete_group`'s own row-locking
    // pattern.
    sqlx::query("SELECT 1 FROM group_members WHERE group_name = $1 FOR UPDATE")
        .bind(&name)
        .fetch_all(&mut *tx)
        .await?;

    // Resolve the group graph as it would look immediately after this removal, read fresh inside
    // this transaction (never the TTL-cached `DbGroupsSource` snapshot `AuthContext` uses for
    // request-time admin checks) -- the same freshly-queried-graph approach `add_member`'s cycle
    // check uses. Refuse the removal if no principal -- no `*` and no `user:` entry, direct or via
    // nesting -- would still reach `admins` afterward. This subsumes the old
    // `name == ADMINS_GROUP`-only special case: emptying a group nested into `admins` (not
    // `admins` itself, e.g. `admins`'s only member is `group:eng-leads`) strands `admins` just as
    // surely as emptying `admins` directly.
    let group_names: Vec<String> = sqlx::query_scalar("SELECT name FROM groups")
        .fetch_all(&mut *tx)
        .await?;
    let member_rows: Vec<(String, String)> =
        sqlx::query_as("SELECT group_name, member FROM group_members")
            .fetch_all(&mut *tx)
            .await?;
    let post_removal_members = member_rows
        .into_iter()
        .filter(|(group_name, member)| !(*group_name == name && *member == query.member));
    let graph = GroupGraph::from_rows(group_names, post_removal_members).map_err(|e| {
        GroupsError::Internal(format!("building the post-removal group graph: {e:#}"))
    })?;
    if !graph.any_principal_reaches(ADMINS_GROUP) {
        return Err(GroupsError::Conflict(
            "removing this member would leave admins reachable only through direct database \
             access"
                .to_string(),
        ));
    }

    let rows_affected =
        sqlx::query("DELETE FROM group_members WHERE group_name = $1 AND member = $2")
            .bind(&name)
            .bind(&query.member)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    if rows_affected == 0 {
        return Err(GroupsError::NotFound);
    }
    tx.commit().await.map_err(|e| {
        GroupsError::Internal(format!("committing remove-member transaction: {e:#}"))
    })?;
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
