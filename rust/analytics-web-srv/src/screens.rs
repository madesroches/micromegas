//! Handlers for user-defined screens CRUD operations.

use crate::app_db::{
    CreateScreenRequest, Screen, UpdateScreenRequest, ValidationError, expand_path_prefixes,
    normalize_name, validate_folder_path, validate_name,
};
use crate::auth::ValidatedUser;
use crate::screen_types::ScreenType;
use axum::{
    Extension, Json,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use micromegas::tracing::prelude::*;
use serde::Serialize;
use sqlx::PgPool;

/// Error response for screen operations.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    code: String,
    message: String,
}

impl ErrorResponse {
    fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

/// Unified error type for screen handlers.
#[derive(Debug)]
pub enum ScreenError {
    NotFound(String),
    BadRequest(ErrorResponse),
    /// The stored row changed since the caller read it.
    Conflict(String),
    Database(sqlx::Error),
}

impl ScreenError {
    /// One-line description for callers that do not speak HTTP (the MCP tools).
    pub fn describe(&self) -> String {
        match self {
            ScreenError::NotFound(name) => format!("Screen '{name}' not found"),
            ScreenError::BadRequest(err) => format!("{}: {}", err.code, err.message),
            ScreenError::Conflict(message) => format!("CONFLICT: {message}"),
            ScreenError::Database(_) => "Internal database error".to_string(),
        }
    }
}

impl IntoResponse for ScreenError {
    fn into_response(self) -> Response {
        match self {
            ScreenError::NotFound(name) => {
                let body = ErrorResponse::new("NOT_FOUND", &format!("Screen '{name}' not found"));
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            ScreenError::BadRequest(err) => (StatusCode::BAD_REQUEST, Json(err)).into_response(),
            ScreenError::Conflict(message) => {
                let body = ErrorResponse::new("CONFLICT", &message);
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            ScreenError::Database(err) => {
                error!("Database error: {}", err);
                let body = ErrorResponse::new("DATABASE_ERROR", "Internal database error");
                (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
            }
        }
    }
}

impl From<sqlx::Error> for ScreenError {
    fn from(err: sqlx::Error) -> Self {
        ScreenError::Database(err)
    }
}

impl From<ValidationError> for ScreenError {
    fn from(err: ValidationError) -> Self {
        ScreenError::BadRequest(ErrorResponse::new(&err.code, &err.message))
    }
}

pub type ScreenResult<T> = Result<T, ScreenError>;

// Locking every ancestor prefix of a folder path (not just the exact
// destination path, via `expand_path_prefixes(path, false)`) is what makes
// `create_screen`/`update_screen` serialize against a concurrent
// `update_folder`/`delete_folder` on any ancestor folder: renaming/deleting
// "team" takes a lock on "team" alone, so a screen create/move into
// "team/sub" must also lock "team" (in addition to "team/sub") to contend
// on that same key — otherwise the two transactions target disjoint
// advisory-lock keys and can race under READ COMMITTED. Root (empty path)
// is excluded — it is never locked, matching `folder_exists` in folders.rs.

// ============================================================================
// Screen Types (static)
// ============================================================================

/// List all available screen types.
#[span_fn]
pub async fn list_screen_types() -> Json<Vec<serde_json::Value>> {
    let types: Vec<_> = ScreenType::all()
        .into_iter()
        .map(|t| {
            let info = t.info();
            serde_json::json!({
                "name": info.name,
                "display_name": info.display_name,
                "icon": info.icon,
                "description": info.description
            })
        })
        .collect();

    Json(types)
}

/// Get the default configuration for a screen type.
#[span_fn]
pub async fn get_default_config(
    Path(type_name): Path<String>,
) -> ScreenResult<Json<serde_json::Value>> {
    let screen_type = parse_screen_type(&type_name)?;

    Ok(Json(screen_type.default_config()))
}

// ============================================================================
// Shared store functions (HTTP handlers and MCP tools)
// ============================================================================

/// Parses a screen type name; the single place that maps a bad name to `INVALID_SCREEN_TYPE`.
pub fn parse_screen_type(type_name: &str) -> ScreenResult<ScreenType> {
    type_name.parse().map_err(|_| {
        ScreenError::BadRequest(ErrorResponse::new(
            "INVALID_SCREEN_TYPE",
            &format!("Invalid screen type: {type_name}"),
        ))
    })
}

/// Normalizes and validates the fields of a create request; returns the normalized name.
pub fn validate_create_request(request: &CreateScreenRequest) -> ScreenResult<String> {
    let name = normalize_name(&request.name);
    validate_name(&name)?;
    validate_folder_path(&request.folder_path)?;
    parse_screen_type(&request.screen_type)?;
    Ok(name)
}

/// A screen without its config, for listings.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ScreenSummary {
    pub name: String,
    pub screen_type: String,
    pub folder_path: String,
    pub managed_by: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl From<Screen> for ScreenSummary {
    fn from(screen: Screen) -> Self {
        Self {
            name: screen.name,
            screen_type: screen.screen_type,
            folder_path: screen.folder_path,
            managed_by: screen.managed_by,
            updated_at: screen.updated_at,
        }
    }
}

pub async fn fetch_screens(pool: &PgPool) -> ScreenResult<Vec<Screen>> {
    let query = format!("SELECT {SCREEN_COLUMNS} FROM screens ORDER BY name");
    Ok(instrument_named!(
        sqlx::query_as::<_, Screen>(&query).fetch_all(pool),
        "sql_select_screens"
    )
    .await?)
}

/// Lists screens without their config, optionally limited to a folder and its subfolders.
pub async fn fetch_screen_summaries(
    pool: &PgPool,
    folder_path: Option<&str>,
) -> ScreenResult<Vec<ScreenSummary>> {
    if let Some(folder_path) = folder_path {
        validate_folder_path(folder_path)?;
    }
    // `left()` rather than LIKE so folder names never need wildcard escaping.
    let summaries = instrument_named!(
        sqlx::query_as::<_, ScreenSummary>(
            "SELECT name, screen_type, folder_path, managed_by, updated_at FROM screens
             WHERE $1::text IS NULL OR $1 = '' OR folder_path = $1
                OR left(folder_path, length($1) + 1) = $1 || '/'
             ORDER BY name"
        )
        .bind(folder_path)
        .fetch_all(pool),
        "sql_select_screen_summaries"
    )
    .await?;
    Ok(summaries)
}

pub async fn fetch_screen(pool: &PgPool, name: &str) -> ScreenResult<Screen> {
    let query = format!("SELECT {SCREEN_COLUMNS} FROM screens WHERE name = $1");
    instrument_named!(
        sqlx::query_as::<_, Screen>(&query)
            .bind(name)
            .fetch_optional(pool),
        "sql_select_screen"
    )
    .await?
    .ok_or_else(|| ScreenError::NotFound(name.to_string()))
}

pub async fn insert_screen(
    pool: &PgPool,
    user: &ValidatedUser,
    request: &CreateScreenRequest,
) -> ScreenResult<Screen> {
    let name = validate_create_request(request)?;

    let mut tx = pool.begin().await?;

    // Lock the destination folder path *and all of its ancestors* so a
    // concurrent delete/rename of any ancestor folder (which locks only its
    // own exact path in `folders.rs`) can't race past this create and leave
    // the screen under a folder that was just renamed/deleted away. Locked
    // shortest-prefix-first (root-to-leaf), matching `update_folder`'s
    // sorted-order deadlock-avoidance convention.
    for lock_path in expand_path_prefixes(&request.folder_path, false) {
        instrument_named!(
            sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
                .bind(&lock_path)
                .execute(&mut *tx),
            "sql_folder_advisory_lock"
        )
        .await?;
    }

    let exists = instrument_named!(
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM screens WHERE name = $1)")
            .bind(&name)
            .fetch_one(&mut *tx),
        "sql_select_screen_exists"
    )
    .await?;

    if exists {
        return Err(ScreenError::BadRequest(ErrorResponse::new(
            "DUPLICATE_NAME",
            &format!("Screen with name '{name}' already exists"),
        )));
    }

    // Use email if available, otherwise fall back to subject
    let user_id = user.email.as_deref().unwrap_or(&user.subject);

    let query = format!(
        "INSERT INTO screens (name, screen_type, config, created_by, updated_by, managed_by, folder_path)
         VALUES ($1, $2, $3, $4, $4, $5, $6)
         RETURNING {SCREEN_COLUMNS}"
    );
    let screen = instrument_named!(
        sqlx::query_as::<_, Screen>(&query)
            .bind(&name)
            .bind(&request.screen_type)
            .bind(&request.config)
            .bind(user_id)
            .bind(&request.managed_by)
            .bind(&request.folder_path)
            .fetch_one(&mut *tx),
        "sql_insert_screen"
    )
    .await?;

    tx.commit().await?;

    info!("Created screen: {} by {}", name, user_id);
    Ok(screen)
}

/// Updates a screen. With `expected_updated_at`, the update is refused with `Conflict` when the
/// stored `updated_at` differs, so a concurrent edit is not silently overwritten.
pub async fn modify_screen(
    pool: &PgPool,
    user: &ValidatedUser,
    name: &str,
    request: &UpdateScreenRequest,
    expected_updated_at: Option<DateTime<Utc>>,
) -> ScreenResult<Screen> {
    if let Some(ref folder_path) = request.folder_path {
        validate_folder_path(folder_path)?;
    }

    // Use email if available, otherwise fall back to subject
    let user_id = user.email.as_deref().unwrap_or(&user.subject);

    let mut tx = pool.begin().await?;

    // If the folder is changing, lock both the screen's current folder path
    // and the destination folder path *and all of their ancestors* (sorted,
    // to match `update_folder`'s deadlock-avoidance convention) so a
    // concurrent delete/rename of any ancestor of either folder can't race
    // past this move. Root (empty path) is never locked.
    if let Some(ref new_folder_path) = request.folder_path {
        let current_folder_path = instrument_named!(
            sqlx::query_scalar::<_, String>("SELECT folder_path FROM screens WHERE name = $1")
                .bind(name)
                .fetch_optional(&mut *tx),
            "sql_select_screen_folder_path"
        )
        .await?
        .ok_or_else(|| ScreenError::NotFound(name.to_string()))?;

        let mut lock_paths: Vec<String> = expand_path_prefixes(&current_folder_path, false);
        lock_paths.extend(expand_path_prefixes(new_folder_path, false));
        lock_paths.sort_unstable();
        lock_paths.dedup();

        for lock_path in lock_paths {
            instrument_named!(
                sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
                    .bind(lock_path)
                    .execute(&mut *tx),
                "sql_folder_advisory_lock"
            )
            .await?;
        }
    }

    let query = format!(
        "UPDATE screens
         SET config = COALESCE($1, config), updated_by = $2, updated_at = NOW(),
             managed_by = COALESCE($4, managed_by), folder_path = COALESCE($5, folder_path)
         WHERE name = $3 AND ($6::timestamptz IS NULL OR updated_at = $6)
         RETURNING {SCREEN_COLUMNS}"
    );
    let updated = instrument_named!(
        sqlx::query_as::<_, Screen>(&query)
            .bind(&request.config)
            .bind(user_id)
            .bind(name)
            .bind(&request.managed_by)
            .bind(&request.folder_path)
            .bind(expected_updated_at)
            .fetch_optional(&mut *tx),
        "sql_update_screen"
    )
    .await?;

    let Some(screen) = updated else {
        let exists = instrument_named!(
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM screens WHERE name = $1)")
                .bind(name)
                .fetch_one(&mut *tx),
            "sql_select_screen_exists"
        )
        .await?;
        return Err(if exists {
            ScreenError::Conflict(format!(
                "Screen '{name}' was modified since it was read; fetch it again before updating"
            ))
        } else {
            ScreenError::NotFound(name.to_string())
        });
    };

    tx.commit().await?;

    info!("Updated screen: {} by {}", name, user_id);
    Ok(screen)
}

pub async fn remove_screen(pool: &PgPool, name: &str) -> ScreenResult<()> {
    let result = instrument_named!(
        sqlx::query("DELETE FROM screens WHERE name = $1")
            .bind(name)
            .execute(pool),
        "sql_delete_screen"
    )
    .await?;

    if result.rows_affected() == 0 {
        return Err(ScreenError::NotFound(name.to_string()));
    }

    info!("Deleted screen: {}", name);
    Ok(())
}

// ============================================================================
// Screens CRUD (HTTP)
// ============================================================================

const SCREEN_COLUMNS: &str = "name, screen_type, config, created_by, updated_by, created_at, updated_at, managed_by, folder_path";

/// List all screens.
#[span_fn]
pub async fn list_screens(Extension(pool): Extension<PgPool>) -> ScreenResult<Json<Vec<Screen>>> {
    Ok(Json(fetch_screens(&pool).await?))
}

/// Get a screen by name.
#[span_fn]
pub async fn get_screen(
    Extension(pool): Extension<PgPool>,
    Path(name): Path<String>,
) -> ScreenResult<Json<Screen>> {
    Ok(Json(fetch_screen(&pool, &name).await?))
}

/// Create a new screen.
#[span_fn]
pub async fn create_screen(
    Extension(pool): Extension<PgPool>,
    Extension(user): Extension<ValidatedUser>,
    Json(request): Json<CreateScreenRequest>,
) -> ScreenResult<(StatusCode, Json<Screen>)> {
    let screen = insert_screen(&pool, &user, &request).await?;
    Ok((StatusCode::CREATED, Json(screen)))
}

/// Update an existing screen.
#[span_fn]
pub async fn update_screen(
    Extension(pool): Extension<PgPool>,
    Extension(user): Extension<ValidatedUser>,
    Path(name): Path<String>,
    Json(request): Json<UpdateScreenRequest>,
) -> ScreenResult<Json<Screen>> {
    Ok(Json(
        modify_screen(&pool, &user, &name, &request, None).await?,
    ))
}

/// Delete a screen.
#[span_fn]
pub async fn delete_screen(
    Extension(pool): Extension<PgPool>,
    Path(name): Path<String>,
) -> ScreenResult<StatusCode> {
    remove_screen(&pool, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}
