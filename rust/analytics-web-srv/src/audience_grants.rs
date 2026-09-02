//! Audience grant API for `analytics-web-srv`.
//!
//! Directly mirrors `ingestion_keys.rs`'s shape (`AudienceGrantsState { pool: Option<PgPool> }`,
//! an `IntoResponse` error enum) over the new `audience_grants` table (migration v7,
//! `rust/ingestion/src/sql_migration.rs`). This is the write surface for the grant store
//! `micromegas-auth::db_audience_grants::DbAudienceGrantsSource` reads from -- the store's own
//! snapshot cache picks up rows written here within its cache TTL.
//!
//! **Duplication, accepted.** This duplicates most of `ingestion_keys.rs`/`analytics_keys.rs`'s
//! validation/SQL/error shape -- deliberately, per those modules' own doc comments: a generic
//! abstraction over a handful of near-identical handlers differing only in which table they
//! target is a shape this codebase already declines elsewhere
//! (`data_sources.rs`/`screens.rs`/`folders.rs`).
//!
//! **Gating, not admin-only.** `create_grant`/`delete_grant` are gated by [`GrantGate`]: an
//! admin acts unconditionally (exactly the old `AdminUser` behavior); a non-admin is admitted
//! only when `self_service_mint_enabled` is on (the same knob `MintGate` and `/my-audiences`
//! already gate on -- sharing an audience is the second half of the self-service feature that
//! introduced it), and then further constrained inside each handler by a per-pair hold/ownership
//! check that needs the parsed body/query (see `create_grant`/`delete_grant`'s own doc comments).
//! `GET .../my-audiences` stays [`AuthenticatedUser`]-gated with its own knob check, unchanged.
//! `GET .../visible` is [`AuthenticatedUser`]-gated with no admin requirement at all -- it is the
//! Audience Access page's own list read, and narrows for a non-admin when the knob is off (see
//! its own doc comment); it carries none of the old paginated `list_grants`'s confidentiality
//! concern differently -- that route is deleted outright, superseded by the caller-scoped
//! `list_audience_grants()` SQL table function for ad-hoc auditing (`micromegas-query`) and by
//! this route for the page's own display.

use crate::auth::{AuthenticatedUser, Unauthenticated};
use axum::extract::{Extension, FromRequestParts, Query};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use micromegas::auth::policy::{caller_selectors, is_valid_audience, valid_selector};
use micromegas::auth::types::AuthContext;
use micromegas::tracing::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

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
    /// Off-by-default self-service gate. Resolved once at startup from `MICROMEGAS_SELF_SERVICE_MINT`
    /// (`web_server.rs`, the same knob resolved onto `IngestionKeysState`), default `false`.
    /// Gates [`GrantGate`] (`create_grant`/`delete_grant`), `GET .../my-audiences`, and the
    /// non-admin narrowing on `GET .../visible` -- all new non-admin surface introduced by the
    /// same self-service feature, so none of it must widen on upgrade any more than the mint
    /// route itself does.
    pub self_service_mint_enabled: bool,
    /// `MICROMEGAS_SELF_SERVICE_MAX_GRANTS_PER_CALLER`, default 50 -- caps how many rows one
    /// non-admin caller may have created in `audience_grants` (`created_by = <caller>`,
    /// counted across every audience/axis/selector, not just the pair being shared into, but
    /// excluding the caller's own `user:<email>` rows -- those are claim/self-access rows, not
    /// shares), checked in `create_grant` before the insert. Mirrors `IngestionKeysState`'s
    /// `max_claims_per_caller`/`max_keys_per_caller` bounds on the mint side of the same
    /// `MICROMEGAS_SELF_SERVICE_MINT` knob: without it, a non-admin holding one grant on a pair
    /// could plant unlimited `group:<arbitrary-id>` rows on it. Best-effort under concurrency,
    /// not a hard ceiling -- same caveat as those two bounds. Skipped for an admin.
    pub max_grants_per_caller: i64,
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
    /// `GrantGate`'s knob-off denial for a non-admin caller, `/my-audiences`'s identical check,
    /// a non-admin `create_grant` naming `selector: "*"`, or a non-admin's per-pair
    /// hold/ownership check failing on `create_grant`/`delete_grant`.
    Forbidden(String),
    /// [`GrantGate`]/[`AuthenticatedUser`] found no `AuthContext` extension in the request --
    /// normally unreachable once routing is wired correctly; mirrors
    /// `ingestion_keys.rs::IngestionKeyError::Unauthenticated`.
    Unauthenticated(String),
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
            AudienceGrantError::Forbidden(msg) => (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse::new("FORBIDDEN", msg)),
            )
                .into_response(),
            AudienceGrantError::Unauthenticated(msg) => (
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse::new("UNAUTHENTICATED", msg)),
            )
                .into_response(),
        }
    }
}

impl From<sqlx::Error> for AudienceGrantError {
    fn from(err: sqlx::Error) -> Self {
        AudienceGrantError::Database(err)
    }
}

/// `GrantGate`'s fallback when `AuthenticatedUser` itself finds no `AuthContext` extension --
/// reached only in the same normally-unreachable case that extractor's own doc comment
/// documents.
impl From<Unauthenticated> for AudienceGrantError {
    fn from(_: Unauthenticated) -> Self {
        AudienceGrantError::Unauthenticated("authentication required".to_string())
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

/// `caller`'s own identity for `created_by`/ownership purposes: email if present, else subject --
/// the same resolution every mint/revoke/import handler in this crate uses.
fn caller_identity(caller: &AuthContext) -> String {
    caller
        .email
        .clone()
        .unwrap_or_else(|| caller.subject.clone())
}

/// `FromRequestParts` extractor for `create_grant`/`delete_grant`, modeled directly on
/// `ingestion_keys.rs`'s `MintGate`: yields the caller's `AuthContext` after enforcing the
/// off-by-default self-service knob for a non-admin caller. Because this is a
/// `FromRequestParts` impl, axum runs it -- like `AuthenticatedUser` itself -- before
/// `Json<CreateGrantRequest>` (or the delete query) ever parses, so a knob-off non-admin caller
/// is rejected before the body is ever touched (`auth/handlers.rs`'s own rationale for why
/// `AdminUser` is an extractor, not an in-body check).
///
/// This only enforces the knob half of the gate. The per-pair hold/ownership check below it
/// genuinely needs the parsed body/query, so it stays in each handler, run after `GrantGate` has
/// already admitted the request.
struct GrantGate(AuthContext);

impl<S: Send + Sync> FromRequestParts<S> for GrantGate {
    type Rejection = AudienceGrantError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let AuthenticatedUser(caller) = AuthenticatedUser::from_request_parts(parts, state).await?;
        let grants_state = parts
            .extensions
            .get::<AudienceGrantsState>()
            .cloned()
            // Reachable only if routing is misconfigured; the 503 body's wording doesn't
            // literally describe this cause, but a 503 fail-closed beats a panic for a case that
            // should never happen in a correctly wired router -- mirrors `MintGate`'s identical
            // `.ok_or(...)`.
            .ok_or(AudienceGrantError::NotConfigured)?;
        if !caller.is_admin() && !grants_state.self_service_mint_enabled {
            return Err(AudienceGrantError::Forbidden(
                "self-service grant management is disabled".to_string(),
            ));
        }
        Ok(GrantGate(caller))
    }
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

/// Whether `caller` holds `(audience, axis)` via an identity selector -- `SELECT
/// EXISTS(... selector = ANY($3))` with the leading `"*"` stripped from `caller_selectors`.
/// A `*` row on the pair makes it publicly readable/mintable, but must not let
/// every authenticated user plant durable `user:`/`group:` rows on that pair, which would outlive
/// the `*` grant once an admin deletes it, silently re-widening access the admin thought they'd
/// just closed.
async fn caller_holds_pair(
    pool: &PgPool,
    audience: &str,
    axis: &str,
    caller: &AuthContext,
) -> Result<bool, AudienceGrantError> {
    let identity_selectors: Vec<String> = caller_selectors(caller)
        .into_iter()
        .filter(|s| s != "*")
        .collect();
    let held: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM audience_grants WHERE audience = $1 AND axis = $2 \
         AND selector = ANY($3))",
    )
    .bind(audience)
    .bind(axis)
    .bind(&identity_selectors)
    .fetch_one(pool)
    .await?;
    Ok(held)
}

/// `POST {base_path}/api/audience-grants` -- creates (or reports the pre-existing) grant row.
/// `201` when this call created it, `200` when it already existed.
///
/// Gated by [`GrantGate`] (the knob check) plus, for a non-admin caller, four further checks
/// that need the parsed body and so live here rather than in the gate:
///
/// 1. `selector` must be `user:…`/`group:…` -- `*` is refused with 403, since a caller who can
///    read an audience must not be able to open it to every authenticated principal.
/// 2. `(axis, selector)` must not be the caller's own `mint`/`user:<email>` claim marker -- that
///    row is what `try_claim_and_mint` (`ingestion_keys.rs`) writes for a real claim, and letting
///    Share plant a byte-identical one would make it permanently undeletable by its own creator
///    and silently burn a `max_claims_per_caller` slot.
/// 3. The caller must hold `(audience, axis)` via an identity selector ([`caller_holds_pair`]).
///    Delegation is per axis: a `read` grant lets you share `read`, a `mint` grant lets you share
///    `mint`, and neither confers the other.
/// 4. The caller must be under `max_grants_per_caller` distinct rows already created
///    (`created_by = <caller>`, counted across every pair, excluding the caller's own
///    `user:<email>` identity-selector rows -- see the check's own comment) -- otherwise a
///    caller who holds one grant could plant unlimited `group:<arbitrary-id>` rows on that pair.
///
/// An admin bypasses all four checks entirely, exactly as before.
async fn create_grant(
    Extension(state): Extension<AudienceGrantsState>,
    GrantGate(caller): GrantGate,
    Json(body): Json<CreateGrantRequest>,
) -> Result<(StatusCode, Json<GrantResponse>), AudienceGrantError> {
    let pool = require_pool(&state)?;
    validate_audience(&body.audience)?;
    validate_axis(&body.axis)?;
    validate_selector(&body.selector)?;
    if let Some(group_name) = body.selector.strip_prefix("group:") {
        let group_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM groups WHERE name = $1)")
                .bind(group_name)
                .fetch_one(&pool)
                .await?;
        if !group_exists {
            return Err(AudienceGrantError::NotFound);
        }
    }
    let created_by = caller_identity(&caller);

    if !caller.is_admin() {
        if body.selector == "*" {
            return Err(AudienceGrantError::Forbidden(
                "non-admin callers may not grant '*' (every authenticated principal) access"
                    .to_string(),
            ));
        }
        // Refuse a caller planting their own `mint`/`user:<email>` row via Share: it is
        // byte-identical to the claim marker `try_claim_and_mint` (`ingestion_keys.rs`) writes,
        // which makes it permanently undeletable by its own creator (`delete_grant`'s own-claim
        // guard above) and lets it silently consume one of the caller's `max_claims_per_caller`
        // slots without ever claiming anything. Route them to the mint endpoint instead.
        if let Some(email) = &caller.email
            && body.axis == "mint"
            && body.selector == format!("user:{email}")
        {
            return Err(AudienceGrantError::Forbidden(
                "you cannot create your own mint claim marker via self-service; claim the \
                 audience through the mint route instead"
                    .to_string(),
            ));
        }
        if !caller_holds_pair(&pool, &body.audience, &body.axis, &caller).await? {
            return Err(AudienceGrantError::Forbidden(format!(
                "you have no {} grant on {} to share",
                body.axis, body.audience
            )));
        }
        // Per-caller bound (mirrors `IngestionKeysState::max_keys_per_caller` on the mint side
        // of this same knob): without it, a caller who holds one grant on a pair could plant
        // unlimited `group:<arbitrary-id>` rows on it -- the PK only blocks exact duplicates.
        // Best-effort under concurrency, not a hard ceiling -- same caveat as the mint bounds.
        //
        // Excludes the caller's own `user:<email>` identity-selector rows: `try_claim_and_mint`
        // (`ingestion_keys.rs`) writes two such rows (`mint` and `read`) per lazy claim, also
        // with `created_by = <caller>`, and those rows represent claiming, not sharing -- this
        // bound exists to cap the latter (per the comment above: unlimited `group:<arbitrary-id>`
        // rows), not to make claiming and sharing compete for the same budget. Mirrors
        // `delete_grant`'s own has-email/no-email split for the same `user:<email>` predicate.
        let grant_count: i64 = if let Some(email) = &caller.email {
            let own_selector = format!("user:{email}");
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM audience_grants WHERE created_by = $1 AND selector <> $2",
            )
            .bind(&created_by)
            .bind(&own_selector)
            .fetch_one(&pool)
            .await?
        } else {
            sqlx::query_scalar("SELECT COUNT(*) FROM audience_grants WHERE created_by = $1")
                .bind(&created_by)
                .fetch_one(&pool)
                .await?
        };
        if grant_count >= state.max_grants_per_caller {
            return Err(AudienceGrantError::Forbidden(format!(
                "you have created the maximum number of grants ({})",
                state.max_grants_per_caller
            )));
        }
    }

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

/// Row shape shared by `GET .../visible` and (via `micromegas-query`) the SQL table function's
/// column order -- kept as its own type since `GET .../visible` is a distinct route with its own
/// visibility rule, not the same handler.
#[derive(Serialize, sqlx::FromRow)]
struct VisibleGrantRow {
    audience: String,
    axis: String,
    selector: String,
    created_at: DateTime<Utc>,
    created_by: String,
}

#[derive(Deserialize)]
struct DeleteGrantQuery {
    audience: String,
    axis: String,
    selector: String,
}

/// `DELETE {base_path}/api/audience-grants?audience=&axis=&selector=` -- natural key passed as
/// query parameters, not path segments: `valid_selector` places no charset restriction on a
/// `group:<id>` selector (a hierarchical IdP group name can contain `/`), so encoding it as a raw
/// path segment the way every other route's `Uuid` id does would be unsafe here. `204` / `404`.
///
/// Gated by [`GrantGate`] (the knob check). An admin deletes any row unconditionally, including
/// their own -- admin access never depends on grant rows, so this is not a restriction on them.
/// A non-admin may delete only their own direct `user:<email>` row ("remove my access" -- never
/// offered for `group:`/`*` rows, since those would affect other principals) or a row they
/// themselves created (the revoke-a-share counterpart of `create_grant`) -- **except**
/// their own `mint`/`user:<email>` row, which "remove my access" no longer covers. That row is
/// exactly the shape `try_claim_and_mint`'s (`ingestion_keys.rs`) self-service claim marker takes,
/// and its own per-caller claim count is read straight from `audience_grants` on that predicate;
/// letting a non-admin delete it would let them claim, delete, and re-claim to dodge
/// `max_claims_per_caller` for free. Their `read`-axis own-row removal is unaffected, and so is
/// removing any row -- `mint` axis included -- that they merely created for someone else rather
/// than for themselves. If the natural key names no row at all, `404`; if it names a row but no
/// condition matches, `403` -- but only when the caller holds a grant on `(audience, axis)` at all
/// ([`caller_holds_pair`]); otherwise `404`, so a non-admin can't use the 403-vs-404 split to
/// probe for the existence of a grant on a pair they can't otherwise see via
/// `/visible`/`list_audience_grants()`.
async fn delete_grant(
    Extension(state): Extension<AudienceGrantsState>,
    GrantGate(caller): GrantGate,
    Query(query): Query<DeleteGrantQuery>,
) -> Result<StatusCode, AudienceGrantError> {
    let pool = require_pool(&state)?;
    validate_axis(&query.axis)?;

    let deleted_by = caller_identity(&caller);
    let rows_affected = if caller.is_admin() {
        sqlx::query(
            "DELETE FROM audience_grants WHERE audience = $1 AND axis = $2 AND selector = $3",
        )
        .bind(&query.audience)
        .bind(&query.axis)
        .bind(&query.selector)
        .execute(&pool)
        .await?
        .rows_affected()
    } else if let Some(email) = &caller.email {
        let own_selector = format!("user:{email}");
        // The trailing `AND NOT ($2 = 'mint' AND selector = $4)` carves the caller's own
        // `mint`/`user:<email>` claim-marker row out of *both* preceding arms, not just the
        // `selector = $4` one -- a self-claim also sets `created_by` to the same caller, so
        // leaving the `created_by = $5` arm unguarded would still let it through. See this
        // function's own doc comment for why that row specifically must stay undeletable by its
        // own subject.
        sqlx::query(
            "DELETE FROM audience_grants WHERE audience = $1 AND axis = $2 AND selector = $3 \
             AND (selector = $4 OR created_by = $5) \
             AND NOT ($2 = 'mint' AND selector = $4)",
        )
        .bind(&query.audience)
        .bind(&query.axis)
        .bind(&query.selector)
        .bind(&own_selector)
        .bind(&deleted_by)
        .execute(&pool)
        .await?
        .rows_affected()
    } else {
        // No email at all -- the own-row arm can never match (there is no `user:<email>`
        // selector to be), so it's dropped from the predicate entirely rather than bound to a
        // sentinel that could never occur naturally but would still read as a real comparison.
        sqlx::query(
            "DELETE FROM audience_grants WHERE audience = $1 AND axis = $2 AND selector = $3 \
             AND created_by = $4",
        )
        .bind(&query.audience)
        .bind(&query.axis)
        .bind(&query.selector)
        .bind(&deleted_by)
        .execute(&pool)
        .await?
        .rows_affected()
    };

    if rows_affected == 0 {
        if caller.is_admin() {
            return Err(AudienceGrantError::NotFound);
        }
        // Distinguish "no such row" (404) from "exists, but not yours" (403) with a follow-up
        // read -- the delete above already told us this caller's predicate didn't match. This
        // must not become an unscoped existence oracle: a non-admin who holds no grant on
        // `(audience, axis)` at all gets 404 regardless of whether the row actually exists,
        // exactly as they'd learn nothing from `/visible` or `list_audience_grants()` either.
        // `caller_holds_pair` is the same "what pairs can this caller act on" check
        // `create_grant`'s hold check runs, not the wider read-visibility query.
        if !caller_holds_pair(&pool, &query.audience, &query.axis, &caller).await? {
            return Err(AudienceGrantError::NotFound);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM audience_grants WHERE audience = $1 AND axis = $2 \
             AND selector = $3)",
        )
        .bind(&query.audience)
        .bind(&query.axis)
        .bind(&query.selector)
        .fetch_one(&pool)
        .await?;
        let is_own_mint_claim_marker = query.axis == "mint"
            && caller
                .email
                .as_deref()
                .is_some_and(|email| query.selector == format!("user:{email}"));
        return Err(if !exists {
            AudienceGrantError::NotFound
        } else if is_own_mint_claim_marker {
            AudienceGrantError::Forbidden(
                "your own mint claim marker cannot be removed via self-service; ask an admin"
                    .to_string(),
            )
        } else {
            AudienceGrantError::Forbidden(
                "this grant is neither your own direct access nor one you created".to_string(),
            )
        });
    }

    info!(
        "deleted audience grant audience={} axis={} selector={} deleted_by={deleted_by}",
        query.audience, query.axis, query.selector
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `GET {base_path}/api/audience-grants/visible` -- the unpaginated read backing the Audience
/// Access page's own list. [`AuthenticatedUser`]-gated, no admin
/// requirement -- built the same way `/my-audiences` already is: no pagination, reading
/// `AudienceGrantsState.pool` directly.
///
/// **Visibility, by caller:**
/// - **Admin**: every row.
/// - **Non-admin, self-service knob on**: every grant on each `(audience, axis)` pair the caller
///   holds a matching grant on -- the same held-pair `EXISTS` query `list_audience_grants()`
///   (the SQL table function) runs, bound to the caller's identity selectors with the leading
///   `"*"` stripped from `caller_selectors` (the same [`caller_holds_pair`] convention): binding
///   the unfiltered list would match every pair carrying a `*` grant row, leaking every sibling
///   row on it to a caller who holds nothing there directly.
/// - **Non-admin, self-service knob off**: narrows to the caller's own rows only
///   (`selector = ANY(...)`, `"*"` stripped the same way) -- never a sibling's
///   `selector`/`created_by`. This route (unlike the table function) *can* read the knob, since
///   it lives in this same crate, and a default knob-off deployment must not hand a browsing
///   non-admin the wider disclosure the held-pair query carries (the same reasoning
///   `/my-audiences` already applies).
///
/// This function is what makes the two read paths deliberately asymmetric:
/// `list_audience_grants()` cannot check this knob at all (a different crate/service), so it
/// always runs the wider held-pair query for a non-admin -- an accepted, intentional exception,
/// not a bug.
async fn visible_grants(
    Extension(state): Extension<AudienceGrantsState>,
    AuthenticatedUser(caller): AuthenticatedUser,
) -> Result<Json<Vec<VisibleGrantRow>>, AudienceGrantError> {
    let pool = require_pool(&state)?;
    let rows = if caller.is_admin() {
        sqlx::query_as::<_, VisibleGrantRow>(
            "SELECT audience, axis, selector, created_at, created_by
             FROM audience_grants
             ORDER BY audience, axis, selector",
        )
        .fetch_all(&pool)
        .await?
    } else if state.self_service_mint_enabled {
        // `"*"` stripped before binding -- the same `caller_holds_pair` convention. Left in, it
        // would match every pair carrying a `*` grant row and leak every sibling row on it to a
        // caller who holds nothing there directly.
        let identity_selectors: Vec<String> = caller_selectors(&caller)
            .into_iter()
            .filter(|s| s != "*")
            .collect();
        sqlx::query_as::<_, VisibleGrantRow>(
            "SELECT g.audience, g.axis, g.selector, g.created_at, g.created_by
             FROM audience_grants g
             WHERE EXISTS (
               SELECT 1 FROM audience_grants h
               WHERE h.audience = g.audience AND h.axis = g.axis AND h.selector = ANY($1)
             )
             ORDER BY g.audience, g.axis, g.selector",
        )
        .bind(&identity_selectors)
        .fetch_all(&pool)
        .await?
    } else {
        // `"*"` stripped before binding -- otherwise this would return every `*`-selector row in
        // the entire store rather than just the caller's own `user:`/`group:` rows.
        let identity_selectors: Vec<String> = caller_selectors(&caller)
            .into_iter()
            .filter(|s| s != "*")
            .collect();
        sqlx::query_as::<_, VisibleGrantRow>(
            "SELECT audience, axis, selector, created_at, created_by
             FROM audience_grants
             WHERE selector = ANY($1)
             ORDER BY audience, axis, selector",
        )
        .bind(&identity_selectors)
        .fetch_all(&pool)
        .await?
    };
    Ok(Json(rows))
}

/// Derives the caller-scoped namespace prefix used to *suggest* a fresh audience name -- the web
/// app's Mint dialog composes it live before commit, and `micromegas-setup-telemetry`'s CLI uses
/// it only to render a concrete `--claim <prefix><name>` suggestion in its zero-match error; the
/// CLI's own `--claim` claims the name it is given verbatim, never prefixing it itself.
/// `pub`, not module-private, and pure/sync -- no DB, no `AuthContext` needed beyond the plain
/// `Option<String>` email -- so the whole sanitization is unit-testable directly, the same reason
/// `ingestion_keys::resolve_audience` is `pub`.
///
/// Takes the local part of `email` (everything before the first `@`), lowercases it, replaces
/// every character outside `[a-z0-9_-]` with `-`, collapses any run of `-` to a single `-`, trims
/// leading/trailing `-`, and appends one more `-` as the separator. `None` when `email` is `None`
/// (a client-credentials service-account caller -- the same condition the lazy claim path itself
/// gates on) or when sanitizing leaves an empty string.
///
/// Deliberately not injective (`alice.smith@x` and `alice-smith@x` both yield `alice-smith-`,
/// and two different domains with the same local part collide too) -- harmless, since
/// authorization is still the exact `user:<email>` selector the lazy claim writes, indifferent to
/// the prefix; a collision just means the second caller's claim attempt hits the ordinary
/// "audience already exists" denial.
pub fn mint_prefix_for(email: &Option<String>) -> Option<String> {
    let email = email.as_deref()?;
    let local = email.split('@').next().unwrap_or("");
    let mut sanitized = String::with_capacity(local.len() + 1);
    let mut last_was_dash = false;
    for ch in local.chars() {
        let lower = ch.to_ascii_lowercase();
        let mapped = if lower.is_ascii_alphanumeric() || lower == '_' || lower == '-' {
            lower
        } else {
            '-'
        };
        if mapped == '-' {
            if last_was_dash {
                continue;
            }
            last_was_dash = true;
        } else {
            last_was_dash = false;
        }
        sanitized.push(mapped);
    }
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        None
    } else {
        Some(format!("{trimmed}-"))
    }
}

#[derive(Serialize)]
struct MyAudiencesResponse {
    is_admin: bool,
    audiences: Vec<String>,
    mint_prefix: Option<String>,
    email: Option<String>,
    /// `"{audience}:{axis}"` for every `(audience, axis)` pair `caller` holds a grant on via an
    /// identity selector -- i.e. the pairs [`caller_holds_pair`] would return `true` for. Lets
    /// the Audience Access page tell "a pair I hold" apart from "a pair I can merely see" (e.g.
    /// visible only via a `*` row, or a `group:` row the caller isn't actually a member of --
    /// the client has no group-membership info of its own to make that call). Always empty for
    /// an admin: `isAdmin` already grants Share everywhere on the client, and every pair is a
    /// held pair for an admin's own writes anyway.
    held_pairs: Vec<String>,
    /// The caller's resolved, transitive local-group membership -- straight off
    /// `AuthContext.memberships`, no query. Lets the CLI and the Audience Access page show why a
    /// caller holds a `group:` grant.
    groups: Vec<String>,
}

/// `GET {base_path}/api/audience-grants/my-audiences` -- audiences `caller` may mint into today,
/// per the DB store's current rows (no cache -- this reads `pool` directly), plus the caller's
/// own `is_admin` flag, `mint_prefix`, and `email`.
///
/// Caller-scoped, so [`AuthenticatedUser`] (any authenticated caller), not admin-gated: this can
/// never reveal another principal's selector, only whether *this* caller's own email/groups
/// match one, plus facts about the caller's own identity. `is_admin`, `mint_prefix`, and `email`
/// all ride on this response because there is no other route reachable from a CLI caller
/// (authenticated purely with a Bearer header) that exposes any of them -- `/auth/me` reads its
/// ID token only from the browser's `id_token` cookie, with no `Authorization: Bearer` fallback.
///
/// Gated on the same off-by-default `self_service_mint_enabled` knob `MintGate`/`GrantGate`
/// enforce, for the same reason: this is new non-admin surface too, and must not widen on
/// upgrade regardless of the knob. An admin caller is exempt, matching `MintGate`'s own
/// `!caller.is_admin()` condition.
async fn my_audiences(
    Extension(state): Extension<AudienceGrantsState>,
    AuthenticatedUser(caller): AuthenticatedUser,
) -> Result<Json<MyAudiencesResponse>, AudienceGrantError> {
    if !caller.is_admin() && !state.self_service_mint_enabled {
        return Err(AudienceGrantError::Forbidden(
            "self-service minting is disabled".to_string(),
        ));
    }
    let pool = require_pool(&state)?;
    // Push the selector test into SQL rather than pulling every `mint` row into Rust and
    // filtering with `selector_matches`: `*` plus the caller's own `user:`/`group:` selectors
    // are exactly the selectors `selector_matches` would accept, and binding them as an array
    // lets Postgres do the filtering instead of materializing the whole (monotonically-growing)
    // mint axis on every call. `caller_selectors` is the shared builder for this exact list.
    let selectors = caller_selectors(&caller);
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT audience FROM audience_grants WHERE axis = 'mint' AND selector = ANY($1)",
    )
    .bind(&selectors)
    .fetch_all(&pool)
    .await?;
    let mut audiences: Vec<String> = rows.into_iter().map(|(audience,)| audience).collect();
    audiences.sort();
    let mint_prefix = mint_prefix_for(&caller.email);
    let email = caller.email.clone();

    // Ground truth for `canShareRow` on the client: the caller's own distinct held
    // `(audience, axis)` pairs, by the exact
    // same rule `caller_holds_pair` checks -- `*` filtered out of `caller_selectors`, matching
    // that write-hold-check convention (a `*` row must not let a non-admin claim they "hold"
    // every pair). An admin needs none of this on the client, so skip the query entirely.
    let held_pairs = if caller.is_admin() {
        Vec::new()
    } else {
        let identity_selectors: Vec<String> = caller_selectors(&caller)
            .into_iter()
            .filter(|s| s != "*")
            .collect();
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT DISTINCT audience, axis FROM audience_grants WHERE selector = ANY($1)",
        )
        .bind(&identity_selectors)
        .fetch_all(&pool)
        .await?;
        rows.into_iter()
            .map(|(audience, axis)| format!("{audience}:{axis}"))
            .collect()
    };

    let groups = caller.memberships.to_vec();

    Ok(Json(MyAudiencesResponse {
        is_admin: caller.is_admin(),
        audiences,
        mint_prefix,
        email,
        held_pairs,
        groups,
    }))
}

/// Routes only -- [`AudienceGrantsState`] is layered separately in
/// `web_server.rs::build_protected_routes`, the same way `analytics_keys_state`/
/// `ingestion_keys_state` are.
///
/// The collection path (`POST`/`DELETE {base_path}/api/audience-grants`) does not answer `GET`
/// -- the Audience Access page's own list lives at `GET {base_path}/api/audience-grants/visible`
/// instead, and ad-hoc auditing goes through `micromegas-query`/`list_audience_grants()`.
pub fn audience_grants_router(base_path: &str) -> Router {
    Router::new()
        .route(
            &format!("{base_path}/api/audience-grants"),
            post(create_grant).delete(delete_grant),
        )
        .route(
            &format!("{base_path}/api/audience-grants/visible"),
            get(visible_grants),
        )
        .route(
            &format!("{base_path}/api/audience-grants/my-audiences"),
            get(my_audiences),
        )
}
