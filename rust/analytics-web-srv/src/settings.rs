//! Settings admin API for `analytics-web-srv`.
//!
//! Admin-only REST routes over the `settings` table (migration v12). The registry of known keys
//! and the single validation rule live in `micromegas::settings`; this module only reads and
//! writes rows and audits every change. The read route is [`AdminUser`]-gated directly; the two
//! mutation routes go through `SettingsAdminGate`, which additionally emits a control-plane audit
//! record on rejection, the same shape as `groups.rs`.

use crate::auth::{AdminRequired, AdminUser, ValidatedUser};
use crate::mutation_audit::{AuditOutcome, ClientIp, MutationAudit, action};
use axum::extract::{Extension, FromRequestParts, Path};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use micromegas::settings::{SETTINGS, SettingSpec, find_spec};
use micromegas::tracing::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// Holds the (possibly absent) telemetry-DB pool for the settings routes. `None` only when
/// `MICROMEGAS_SQL_CONNECTION_STRING` is unset -- the routes stay registered either way and
/// return 503 per-request in that case, the same shape `GroupsState` uses.
#[derive(Clone)]
pub struct SettingsState {
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

/// Errors this API returns. No `Forbidden` variant: the admin gate is the [`AdminUser`]
/// extractor, whose rejection renders as `AdminRequired`'s own 403 body.
pub enum SettingsError {
    BadRequest(String),
    NotFound,
    Database(sqlx::Error),
    NotConfigured,
    /// A stored value that does not parse as JSON.
    Internal(String),
}

impl IntoResponse for SettingsError {
    fn into_response(self) -> Response {
        match self {
            SettingsError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new("BAD_REQUEST", msg)),
            )
                .into_response(),
            SettingsError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse::new("NOT_FOUND", "setting not found")),
            )
                .into_response(),
            SettingsError::Database(err) => {
                error!("settings: database error: {err}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new(
                        "DATABASE_ERROR",
                        "internal database error",
                    )),
                )
                    .into_response()
            }
            SettingsError::NotConfigured => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "NOT_CONFIGURED",
                    "settings store not configured: set MICROMEGAS_SQL_CONNECTION_STRING",
                )),
            )
                .into_response(),
            SettingsError::Internal(msg) => {
                error!("settings: internal error: {msg}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new("INTERNAL_ERROR", "internal error")),
                )
                    .into_response()
            }
        }
    }
}

impl From<sqlx::Error> for SettingsError {
    fn from(err: sqlx::Error) -> Self {
        SettingsError::Database(err)
    }
}

impl AuditOutcome for SettingsError {
    fn audit_outcome(&self) -> (&'static str, Option<String>) {
        match self {
            SettingsError::BadRequest(msg) => ("denied", Some(msg.clone())),
            SettingsError::NotFound => ("denied", Some("setting not found".to_string())),
            // The raw `sqlx::Error` text can carry SQL/connection detail, so the record gets the
            // same fixed string the client does.
            SettingsError::Database(_) => ("error", Some("internal database error".to_string())),
            SettingsError::NotConfigured => (
                "error",
                Some(
                    "settings store not configured: set MICROMEGAS_SQL_CONNECTION_STRING"
                        .to_string(),
                ),
            ),
            SettingsError::Internal(_) => ("error", Some("internal error".to_string())),
        }
    }
}

fn require_pool(state: &SettingsState) -> Result<PgPool, SettingsError> {
    state.pool.clone().ok_or(SettingsError::NotConfigured)
}

/// `caller`'s own identity for `updated_by` -- the same resolution every admin route in this
/// crate uses.
fn caller_identity(caller: &ValidatedUser) -> String {
    caller
        .email
        .clone()
        .unwrap_or_else(|| caller.subject.clone())
}

fn settings_gate_action(parts: &Parts) -> &'static str {
    if parts.method == Method::DELETE {
        action::RESET_SETTING
    } else {
        action::SET_SETTING
    }
}

/// `FromRequestParts` extractor for the two mutation routes: delegates to [`AdminUser`] and on
/// rejection emits a `"denied"` mutation-audit record before returning `AdminRequired`
/// unchanged.
struct SettingsAdminGate(ValidatedUser);

impl<S: Send + Sync> FromRequestParts<S> for SettingsAdminGate {
    type Rejection = AdminRequired;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let action = settings_gate_action(parts);
        let ClientIp(client_ip) = ClientIp::from_request_parts(parts, state)
            .await
            .expect("ClientIp is infallible");
        match AdminUser::from_request_parts(parts, state).await {
            Ok(AdminUser(user)) => Ok(SettingsAdminGate(user)),
            Err(rejection) => {
                let (actor, is_admin) = match parts.extensions.get::<ValidatedUser>() {
                    Some(user) => (caller_identity(user), user.is_admin),
                    None => ("unauthenticated".to_string(), false),
                };
                MutationAudit::new(action, actor, is_admin, client_ip)
                    .emit_gate_outcome("denied", "Admin access required");
                Err(rejection)
            }
        }
    }
}

/// One stored `settings` row.
pub struct StoredSetting {
    pub key: String,
    pub value: serde_json::Value,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

/// One entry of the `GET /api/settings` response.
#[derive(Debug, Serialize)]
pub struct SettingEntry {
    pub key: &'static str,
    pub kind: &'static str,
    pub env_var: &'static str,
    pub default: serde_json::Value,
    pub description: &'static str,
    pub value: Option<serde_json::Value>,
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by: Option<String>,
}

/// Turns the registry plus the stored rows into response entries, one per registered setting.
/// Stored rows for unregistered keys (written by a newer replica) are not listed.
pub fn build_entries(specs: &[SettingSpec], rows: &[StoredSetting]) -> Vec<SettingEntry> {
    specs
        .iter()
        .map(|spec| {
            let stored = rows.iter().find(|r| r.key == spec.key);
            SettingEntry {
                key: spec.key,
                kind: spec.kind.as_str(),
                env_var: spec.env_var,
                default: serde_json::from_str(spec.default)
                    .expect("SETTINGS defaults are valid JSON"),
                description: spec.description,
                value: stored.map(|r| r.value.clone()),
                updated_at: stored.map(|r| r.updated_at),
                updated_by: stored.map(|r| r.updated_by.clone()),
            }
        })
        .collect()
}

fn parse_stored(key: &str, raw: &str) -> Result<serde_json::Value, SettingsError> {
    serde_json::from_str(raw)
        .map_err(|e| SettingsError::Internal(format!("stored value for settings.{key}: {e}")))
}

/// Reads the table directly, not through the cache, so the page shows what was just written.
async fn load_rows(pool: &PgPool) -> Result<Vec<StoredSetting>, SettingsError> {
    let rows: Vec<(String, String, DateTime<Utc>, String)> =
        sqlx::query_as("SELECT key, value::text, updated_at, updated_by FROM settings")
            .fetch_all(pool)
            .await?;
    rows.into_iter()
        .map(|(key, raw, updated_at, updated_by)| {
            Ok(StoredSetting {
                value: parse_stored(&key, &raw)?,
                key,
                updated_at,
                updated_by,
            })
        })
        .collect()
}

/// `GET {base_path}/api/settings` -- one entry per registered setting.
async fn list_settings(
    Extension(state): Extension<SettingsState>,
    AdminUser(_user): AdminUser,
) -> Result<Json<Vec<SettingEntry>>, SettingsError> {
    let pool = require_pool(&state)?;
    let rows = load_rows(&pool).await?;
    Ok(Json(build_entries(SETTINGS, &rows)))
}

#[derive(Deserialize)]
struct PutSettingRequest {
    value: serde_json::Value,
}

/// `PUT {base_path}/api/settings/{key}` -- `404` on an unregistered key, `400` on a value that
/// fails the key's validation.
async fn put_setting(
    Extension(state): Extension<SettingsState>,
    SettingsAdminGate(user): SettingsAdminGate,
    client_ip: ClientIp,
    Path(key): Path<String>,
    Json(body): Json<PutSettingRequest>,
) -> Result<Json<SettingEntry>, SettingsError> {
    let audit = MutationAudit::new(
        action::SET_SETTING,
        caller_identity(&user),
        user.is_admin,
        client_ip.0,
    )
    .setting(&key)
    .value(&body.value.to_string());
    let result = put_setting_inner(state, user, &key, body.value).await;
    let audit = match &result {
        Ok((_, Some(previous))) => audit.previous_value(&previous.to_string()),
        _ => audit,
    };
    audit.emit(&result);
    result.map(|(entry, _)| Json(entry))
}

async fn put_setting_inner(
    state: SettingsState,
    user: ValidatedUser,
    key: &str,
    value: serde_json::Value,
) -> Result<(SettingEntry, Option<serde_json::Value>), SettingsError> {
    let spec = find_spec(key).ok_or(SettingsError::NotFound)?;
    spec.validate(&value)
        .map_err(|e| SettingsError::BadRequest(format!("{e:#}")))?;
    let pool = require_pool(&state)?;
    let updated_by = caller_identity(&user);

    let mut tx = pool.begin().await?;
    let previous_raw: Option<String> =
        sqlx::query_scalar("SELECT value::text FROM settings WHERE key = $1 FOR UPDATE")
            .bind(key)
            .fetch_optional(&mut *tx)
            .await?;
    let previous = previous_raw
        .map(|raw| parse_stored(key, &raw))
        .transpose()?;
    let (raw, updated_at, updated_by): (String, DateTime<Utc>, String) = sqlx::query_as(
        "INSERT INTO settings (key, value, updated_at, updated_by)
         VALUES ($1, $2::text::jsonb, now(), $3)
         ON CONFLICT (key) DO UPDATE
           SET value = EXCLUDED.value, updated_at = EXCLUDED.updated_at,
               updated_by = EXCLUDED.updated_by
         RETURNING value::text, updated_at, updated_by",
    )
    .bind(key)
    .bind(value.to_string())
    .bind(&updated_by)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    let stored = StoredSetting {
        key: key.to_string(),
        value: parse_stored(key, &raw)?,
        updated_at,
        updated_by,
    };
    let entry = build_entries(std::slice::from_ref(spec), &[stored])
        .pop()
        .expect("one spec yields one entry");
    Ok((entry, previous))
}

/// `DELETE {base_path}/api/settings/{key}` -- the key reverts to its env var or default. `204`;
/// `404` on an unregistered key or one with no row.
async fn delete_setting(
    Extension(state): Extension<SettingsState>,
    SettingsAdminGate(user): SettingsAdminGate,
    client_ip: ClientIp,
    Path(key): Path<String>,
) -> Result<StatusCode, SettingsError> {
    let audit = MutationAudit::new(
        action::RESET_SETTING,
        caller_identity(&user),
        user.is_admin,
        client_ip.0,
    )
    .setting(&key);
    let result = delete_setting_inner(state, &key).await;
    let audit = match &result {
        Ok(previous) => audit.previous_value(&previous.to_string()),
        Err(_) => audit,
    };
    audit.emit(&result);
    result.map(|_| StatusCode::NO_CONTENT)
}

async fn delete_setting_inner(
    state: SettingsState,
    key: &str,
) -> Result<serde_json::Value, SettingsError> {
    find_spec(key).ok_or(SettingsError::NotFound)?;
    let pool = require_pool(&state)?;
    let raw: Option<String> =
        sqlx::query_scalar("DELETE FROM settings WHERE key = $1 RETURNING value::text")
            .bind(key)
            .fetch_optional(&pool)
            .await?;
    let raw = raw.ok_or(SettingsError::NotFound)?;
    parse_stored(key, &raw)
}

pub fn settings_router(base_path: &str) -> Router {
    Router::new()
        .route(&format!("{base_path}/api/settings"), get(list_settings))
        .route(
            &format!("{base_path}/api/settings/{{key}}"),
            axum::routing::put(put_setting).delete(delete_setting),
        )
}
