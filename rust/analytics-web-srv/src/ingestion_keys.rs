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
//! This is also the attribution fix: every mint/revoke/import records the
//! acting caller's own OIDC identity, never a shared service credential the
//! way the removed proxy did. `list_keys`/`revoke_key`/`import_key` still do
//! so via the [`AdminUser`] extractor; `mint_key` (AbAC Stage 6, #1374) now
//! runs through [`MintGate`]/[`AuthenticatedUser`] instead, since minting is
//! no longer purely admin-gated -- see that extractor's own doc comment.
//!
//! **Duplication, accepted.** This duplicates most of `analytics_keys.rs`'s
//! validation/SQL/error shape — deliberately, per that module's own doc
//! comment: sharing it would mean a generic abstraction over a handful of
//! near-identical handlers differing only in which table they target, the
//! same shape the codebase already declines to share between
//! `data_sources.rs`/`screens.rs`/`folders.rs` today.

use crate::auth::{AdminUser, AuthenticatedUser, Unauthenticated};
use axum::extract::{Extension, FromRequestParts, Path, Query};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use micromegas::auth::db_api_key::{generate_key, hash_key};
use micromegas::auth::policy::{
    AudienceGrants, AudienceMintPolicy, GrantAxis, MintPolicy, PUBLIC_AUDIENCE, is_valid_audience,
};
use micromegas::auth::types::AuthContext;
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
    /// Off-by-default self-service mint gate (AbAC Stage 6, #1374). Resolved once at startup
    /// from `MICROMEGAS_SELF_SERVICE_MINT` (`web_server.rs`), default `false`. Checked by
    /// [`MintGate`] for every non-admin caller before `mint_key`'s body runs at all -- with the
    /// knob off, a deployment that upgrades to this stage keeps today's admin-only mint
    /// behavior unchanged.
    pub self_service_mint_enabled: bool,
    /// `MICROMEGAS_SELF_SERVICE_MAX_CLAIMS_PER_CALLER`, default 25 -- caps how many distinct
    /// audiences one non-admin caller may claim via the lazy claim path
    /// ([`try_claim_and_mint`]). Best-effort under concurrency, not a hard ceiling -- see that
    /// function's own doc comment.
    pub max_claims_per_caller: i64,
    /// `MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER`, default 100 -- caps how many *live* keys
    /// one non-admin caller may hold at once, checked in `mint_key` regardless of which path
    /// mints the next one. Best-effort under concurrency, not a hard ceiling.
    pub max_keys_per_caller: i64,
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
/// `Forbidden`/`Unavailable`/`Unauthenticated`/`Conflict` all back the self-service mint path
/// (AbAC Stage 6, #1374): `mint_key` is no longer purely [`AdminUser`]-gated, whose own rejection
/// (`AdminRequired`) used to make a `Forbidden` variant here dead code -- it is now
/// [`MintGate`]/[`AuthenticatedUser`]-gated, and its own denials (the off-by-default gate, a
/// missing-grant/malformed-audience `MintPolicy` denial, a per-caller bound, and lock contention
/// on a lazy claim) need their own status codes. `list_keys`/`revoke_key`/`import_key` stay
/// `AdminUser`-gated and never construct any of the four.
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
    /// Self-service mint denied: the `MICROMEGAS_SELF_SERVICE_MINT` gate is off for a non-admin
    /// caller, `MintPolicy::resolve_audience` denied the request (no matching grant, or a
    /// malformed audience from an admin), the audience is not eligible for a lazy claim, or a
    /// per-caller bound (`max_claims_per_caller`/`max_keys_per_caller`) was reached.
    Forbidden(String),
    /// The audience-grant point query behind `resolve_audience`'s policy call (or the row parse
    /// immediately after it) failed -- a DB outage, not a denial, so it must not be
    /// misattributed as "you have no grant." Distinct from `NotConfigured`, whose message is
    /// specifically about an unset `MICROMEGAS_SQL_CONNECTION_STRING` and would mislead here.
    Unavailable(String),
    /// [`AuthenticatedUser`]/[`MintGate`] found no `AuthContext` extension in the request --
    /// normally unreachable once routing is wired correctly; see `AuthenticatedUser`'s own doc
    /// comment for the (fail-closed) case this covers.
    Unauthenticated(String),
    /// A lazy claim ([`try_claim_and_mint`]) lost the per-audience advisory-lock race to a
    /// concurrent claimant. Transient lock contention, not a denial -- the caller (in particular
    /// `micromegas-setup-telemetry`) should retry, not treat this as "you may not do this."
    Conflict(String),
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
            IngestionKeyError::Forbidden(msg) => (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse::new("FORBIDDEN", msg)),
            )
                .into_response(),
            IngestionKeyError::Unavailable(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new("UNAVAILABLE", msg)),
            )
                .into_response(),
            IngestionKeyError::Unauthenticated(msg) => (
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse::new("UNAUTHENTICATED", msg)),
            )
                .into_response(),
            IngestionKeyError::Conflict(msg) => (
                StatusCode::CONFLICT,
                Json(ErrorResponse::new("CLAIM_CONTENDED", msg)),
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

/// `MintGate`'s fallback when `AuthenticatedUser` itself finds no `AuthContext` extension --
/// reached only in the same normally-unreachable case that extractor's own doc comment
/// documents.
impl From<Unauthenticated> for IngestionKeyError {
    fn from(_: Unauthenticated) -> Self {
        IngestionKeyError::Unauthenticated("authentication required".to_string())
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
///
/// This is format/defaulting validation only -- it runs before `mint_key`'s
/// `MintPolicy::resolve_audience` authorization decision (AbAC Stage 6, #1374, Design §4), and is
/// unaware of grants or claims.
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

/// `FromRequestParts` extractor for `mint_key` specifically -- yields the caller's `AuthContext`
/// after enforcing the off-by-default self-service gate (AbAC Stage 6, #1374, Design §3). Because
/// this is a `FromRequestParts` impl, axum runs it -- like `AuthenticatedUser` itself -- before
/// `Json<MintRequest>` ever parses the request body: a knob-off non-admin caller is rejected
/// before the body is ever touched, mirroring `AdminUser`'s own ordering guarantee.
struct MintGate(AuthContext);

impl<S: Send + Sync> FromRequestParts<S> for MintGate {
    type Rejection = IngestionKeyError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let AuthenticatedUser(caller) = AuthenticatedUser::from_request_parts(parts, state).await?;
        // `.ok_or(...)`, not `.expect(...)`: every other extractor in this crate fails closed
        // with a status code for a missing extension rather than panicking (`AdminUser`'s own
        // `.ok_or(AdminRequired)` for the identical "extension should always be layered"
        // situation) -- `MintGate` should not be the one exception.
        let ingestion_state = parts
            .extensions
            .get::<IngestionKeysState>()
            .cloned()
            .ok_or(IngestionKeyError::NotConfigured)?; // reachable only if routing is
        // misconfigured; the 503 body's wording (about `MICROMEGAS_SQL_CONNECTION_STRING`)
        // doesn't literally describe this cause, but a 503 fail-closed beats a panic for a
        // case that should never happen in a correctly wired router.
        if !caller.is_admin && !ingestion_state.self_service_mint_enabled {
            return Err(IngestionKeyError::Forbidden(
                "self-service minting is disabled".to_string(),
            ));
        }
        Ok(MintGate(caller))
    }
}

/// `POST {base_path}/api/ingestion-api-keys` — mints a new `ingestion_api_keys` row.
///
/// Authorization is `MintGate` (the off-by-default self-service gate, AbAC Stage 6, #1374) plus
/// `MintPolicy::resolve_audience` (a per-request point query against `audience_grants`, Design
/// §4) -- no longer a flat `AdminUser` gate. Format/defaulting validation still runs first,
/// through the untouched free `resolve_audience` function below, which is what keeps this
/// route's pre-stage 400s unchanged.
async fn mint_key(
    Extension(state): Extension<IngestionKeysState>,
    MintGate(caller): MintGate,
    Json(body): Json<MintRequest>,
) -> Result<(StatusCode, Json<MintResponse>), IngestionKeyError> {
    let pool = require_pool(&state)?;
    validate_name(&body.name)?;
    // `fallback: None` -- a new credential must never silently default to `public`; with
    // neither an explicit `audience` nor `MICROMEGAS_DEFAULT_KEY_AUDIENCE` configured, this
    // is a `BadRequest`, not a fail-open publish grant. `?` here is what preserves the exact
    // existing 400 bodies -- `MintPolicy::resolve_audience`'s own `requested: None` /
    // malformed-audience arms are never reached from this route at all.
    let candidate = resolve_audience(&state, body.audience.as_deref(), None)?;

    // Key material, generated here -- before the policy call, not after -- so both the
    // ordinary and the lazy-claim path share one value to insert.
    let key = generate_key();
    let hash = hash_key(&key);
    let key_id = Uuid::new_v4();
    let created_at = Utc::now();

    // Per-caller bound (Design §4a): caps how many *live* keys one non-admin may hold, regardless
    // of which path below mints the next one. Admins are exempt -- this bounds self-service, not
    // administration. Best-effort, not a hard ceiling, under concurrency: this `SELECT COUNT(*)`
    // runs on `pool` outside any transaction, so N concurrent mint requests from the same caller
    // all read the same pre-insert count and can all pass -- the cap is exact only for
    // sequential use from one caller.
    if !caller.is_admin {
        let caller_id = caller.email.as_deref().unwrap_or(&caller.subject);
        let key_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ingestion_api_keys WHERE created_by = $1 AND revoked_at IS NULL",
        )
        .bind(caller_id)
        .fetch_one(&pool)
        .await?;
        if key_count >= state.max_keys_per_caller {
            return Err(IngestionKeyError::Forbidden(format!(
                "caller already holds {key_count} live self-service keys (limit {})",
                state.max_keys_per_caller
            )));
        }
    }

    // Mint authorization is a point query, not a cached snapshot (Design §3): no
    // `DbAudienceGrantsSource` is attached for mint at all. `audience` is the leading column of
    // `audience_grants`'s `PRIMARY KEY (audience, axis, selector)`, so this is an index-only
    // scan.
    let mint_selectors: Vec<String> = sqlx::query_scalar(
        "SELECT selector FROM audience_grants WHERE audience = $1 AND axis = 'mint'",
    )
    .bind(&candidate)
    .fetch_all(&pool)
    .await
    // A failed query is a DB outage, not a denial -- must not be misattributed as "you have no
    // grant," so it is mapped to `Unavailable` explicitly. Logs the real `sqlx::Error`
    // server-side and renders a fixed, generic client message -- the raw error text (connection
    // strings, table/column names) must not reach the client.
    .map_err(|e| {
        error!("ingestion_keys: audience grant point query failed: {e}");
        IngestionKeyError::Unavailable("audience grant store unavailable".to_string())
    })?;

    let grants = AudienceGrants::from_rows(
        mint_selectors
            .into_iter()
            .map(|selector| (candidate.clone(), GrantAxis::Mint, selector)),
    )
    // Every row read back out of `audience_grants` already passed its own `CHECK` constraints on
    // write, so this arm is unreachable in practice; kept as a real status code rather than
    // `.unwrap()`, for the same fail-closed reason `MintGate` gives its own `.ok_or(...)`.
    .map_err(|e| {
        error!("ingestion_keys: audience grant row parse failed: {e}");
        IngestionKeyError::Unavailable("audience grant store unavailable".to_string())
    })?;

    // `store: None` (the `new` default) -- this stage never attaches a `DbAudienceGrantsSource`
    // to a mint policy (Design §3).
    let policy = AudienceMintPolicy::new(grants);

    let audience = match policy.resolve_audience(&caller, Some(&candidate)).await {
        Ok(aud) => aud,
        // Malformed-audience arm; `candidate` is already valid-format (via `resolve_audience`
        // above), so unreachable in practice.
        Err(e) if caller.is_admin => return Err(IngestionKeyError::Forbidden(e.to_string())),
        Err(_) => {
            // Non-admin, no matching `mint` grant for `candidate` among the rows the point query
            // above just read -- try the lazy claim (Design §4a) only when the caller explicitly
            // named this audience (not merely `state.default_audience`), and has an email to
            // claim with.
            let explicit = body.audience.as_deref().filter(|s| !s.is_empty()).is_some();
            match (explicit, caller.email.as_deref()) {
                (true, Some(_email)) => {
                    // Commits its own grant + key rows and returns the finished response
                    // directly -- `mint_key` never reaches the ordinary `INSERT` below for this
                    // path.
                    return try_claim_and_mint(
                        &pool, &state, &candidate, &caller, &body, key, key_id, &hash, created_at,
                    )
                    .await
                    .map(|resp| (StatusCode::CREATED, Json(resp)));
                }
                _ => {
                    return Err(IngestionKeyError::Forbidden(format!(
                        "audience {candidate:?} is not in the caller's mintable set"
                    )));
                }
            }
        }
    };

    // Ordinary (non-claim) path: single INSERT, as today, using the `key`/`hash`/`key_id`/
    // `created_at` generated above.
    let created_by = caller
        .email
        .clone()
        .unwrap_or_else(|| caller.subject.clone());

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

/// The lazy audience claim (AbAC Stage 6, #1374, Design §4a). Reached only from `mint_key`, only
/// for a non-admin caller who explicitly named `audience`, has a known `caller.email`, and whose
/// `MintPolicy::resolve_audience` call was just denied because `audience` carried no matching
/// `mint` grant.
///
/// One transaction, on the same pool `mint_key` already has: takes a non-blocking, per-audience
/// Postgres advisory lock to serialize concurrent claims for the *same* audience name (across
/// processes, since advisory locks are server-instance-wide), then checks both `audience_grants`
/// and `ingestion_api_keys` for any existing row naming this audience -- a genuinely fresh
/// audience has neither. On a fresh audience: rejects the two reserved names (`public` and
/// `state.default_audience`), enforces the per-caller claim bound
/// (`max_claims_per_caller`), and writes `user:<email>` grant rows on **both** the `mint` and
/// `read` axes (so the caller who just claimed the audience can read back what their own new key
/// uploads) plus the `ingestion_api_keys` row itself, all in the same transaction.
#[allow(clippy::too_many_arguments)]
async fn try_claim_and_mint(
    pool: &PgPool,
    state: &IngestionKeysState,
    audience: &str,
    caller: &AuthContext,
    body: &MintRequest,
    key: String,
    key_id: Uuid,
    hash: &[u8],
    created_at: DateTime<Utc>,
) -> Result<MintResponse, IngestionKeyError> {
    let mut tx = pool.begin().await?;

    // `pg_try_advisory_xact_lock` (non-blocking), not the blocking `pg_advisory_xact_lock`: this
    // transaction runs on a 2-connection pool shared with every other admin route in this crate,
    // so a blocking acquire could exhaust it under concurrent contention. The `_try_` form
    // returns immediately instead; Postgres advisory locks are server-instance-wide, so this
    // still correctly serializes two concurrent claims for the *same* audience name across
    // different processes. `_xact_lock` (not the session-level `pg_advisory_lock`) releases
    // automatically at COMMIT/ROLLBACK, so a claim that errors out never leaks a held lock.
    let locked: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(audience)
            .fetch_one(&mut *tx)
            .await?;
    if !locked {
        tx.rollback().await?;
        // `Conflict` (409, `CLAIM_CONTENDED`), not `Forbidden` (403): this is transient lock
        // contention, not a denial -- the caller should retry, not treat this as "you may not do
        // this."
        return Err(IngestionKeyError::Conflict(format!(
            "audience {audience:?} is being claimed by another request -- retry"
        )));
    }

    let caller_email = caller
        .email
        .as_deref()
        .expect("mint_key only calls try_claim_and_mint when caller.email is Some");
    let selector = format!("user:{caller_email}");
    // Same 255-byte limit `audience_grants.rs`'s admin grant-write route enforces: the
    // `audience_grants.selector` column is `VARCHAR(255)`, and an RFC-max email can push
    // `"user:" + email` past that, which would otherwise 500 at the INSERT below instead of
    // failing cleanly.
    if selector.len() > 255 {
        tx.rollback().await?;
        return Err(IngestionKeyError::Forbidden(
            "caller email is too long to form a valid grant selector".to_string(),
        ));
    }

    // "Does this audience already have an owner?" -- true for any grant row (any axis/selector)
    // *or* any existing `ingestion_api_keys` row: `audience_grants` alone would miss an audience
    // an admin minted straight through `AudienceMintPolicy`'s `is_admin` arm before any grant
    // ever existed for it.
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM audience_grants WHERE audience = $1)
            OR EXISTS(SELECT 1 FROM ingestion_api_keys WHERE audience = $1)",
    )
    .bind(audience)
    .fetch_one(&mut *tx)
    .await?;

    if exists {
        // No recheck needed: reaching this function at all already means `resolve_audience` ran
        // a fresh, uncached point query against exactly this audience's `mint` selectors moments
        // earlier in this same request and found no match for the caller.
        tx.rollback().await?;
        return Err(IngestionKeyError::Forbidden(format!(
            "audience {audience:?} already exists and the caller has no grant for it"
        )));
    } else {
        // Genuinely fresh audience -- no grant row and no key row for it at all, so this is a
        // real claim attempt. Reject the two reserved names here, not before the transaction
        // opened: a caller who already holds a genuine `mint` grant on either name never reaches
        // this branch at all -- `resolve_audience`'s point query would already have found that
        // grant and approved the mint directly.
        if audience == PUBLIC_AUDIENCE || Some(audience) == state.default_audience.as_deref() {
            tx.rollback().await?;
            return Err(IngestionKeyError::Forbidden(format!(
                "audience {audience:?} cannot be claimed"
            )));
        }
        // Genuinely fresh audience: this is a real claim, so it counts against the per-caller
        // bound. Best-effort under concurrency: exact against another claim for *this same*
        // audience (serialized by the lock above), but not against concurrent claims by the same
        // caller for other, distinct fresh audience names, which take different locks.
        let claim_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT audience) FROM audience_grants
             WHERE axis = 'mint' AND selector = $1 AND created_by = $2",
        )
        .bind(&selector)
        .bind(caller_email)
        .fetch_one(&mut *tx)
        .await?;
        if claim_count >= state.max_claims_per_caller {
            tx.rollback().await?;
            return Err(IngestionKeyError::Forbidden(format!(
                "caller has already claimed {claim_count} audiences (limit {})",
                state.max_claims_per_caller
            )));
        }
        for axis in ["mint", "read"] {
            sqlx::query(
                "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
                 VALUES ($1, $2, $3, now(), $4)",
            )
            .bind(audience)
            .bind(axis)
            .bind(&selector)
            .bind(caller_email)
            .execute(&mut *tx)
            .await?;
        }
    }

    sqlx::query(
        "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by, audience)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(key_id)
    .bind(hash)
    .bind(&body.name)
    .bind(created_at)
    .bind(caller_email)
    .bind(audience)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    // `mint_key`'s own mint audit line is never reached for this path -- it returns from here
    // directly -- so log both the mint and the claim here instead. The `exists`-true branch above
    // always returns early, so every call that reaches this point took the `else`
    // (genuinely-fresh) branch and wrote both new grant rows -- the claim line is unconditional.
    info!(
        "minted ingestion api key key_id={key_id} name={} created_by={caller_email} audience={audience}",
        body.name
    );
    info!(
        "claimed audience via lazy self-service mint audience={audience} selector={selector} created_by={caller_email} axes=mint,read"
    );

    Ok(MintResponse {
        key_id,
        name: body.name.clone(),
        created_at,
        audience: audience.to_string(),
        key,
    })
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
