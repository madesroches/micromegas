//! Handlers for user-defined screens CRUD operations.

use analytics_web_srv::app_db::{
    CreateScreenRequest, Screen, UpdateScreenRequest, ValidationError, normalize_screen_name,
    validate_screen_name,
};
use analytics_web_srv::screen_types::ScreenType;
use axum::{
    Extension, Json,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
};
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
pub enum ScreenError {
    NotFound(String),
    BadRequest(ErrorResponse),
    Database(sqlx::Error),
}

impl IntoResponse for ScreenError {
    fn into_response(self) -> Response {
        match self {
            ScreenError::NotFound(name) => {
                let body = ErrorResponse::new("NOT_FOUND", &format!("Screen '{name}' not found"));
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            ScreenError::BadRequest(err) => (StatusCode::BAD_REQUEST, Json(err)).into_response(),
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

type ScreenResult<T> = Result<T, ScreenError>;

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
    let screen_type: ScreenType = type_name.parse().map_err(|_| {
        ScreenError::BadRequest(ErrorResponse::new(
            "INVALID_SCREEN_TYPE",
            &format!("Invalid screen type: {type_name}"),
        ))
    })?;

    Ok(Json(screen_type.default_config()))
}

// ============================================================================
// Screens CRUD
// ============================================================================

/// List all screens.
#[span_fn]
pub async fn list_screens(Extension(pool): Extension<PgPool>) -> ScreenResult<Json<Vec<Screen>>> {
    let screens = sqlx::query_as::<_, Screen>(
        "SELECT name, screen_type, config, created_by, created_at, updated_at
         FROM screens
         ORDER BY name",
    )
    .fetch_all(&pool)
    .await?;

    Ok(Json(screens))
}

/// Get a screen by name.
#[span_fn]
pub async fn get_screen(
    Extension(pool): Extension<PgPool>,
    Path(name): Path<String>,
) -> ScreenResult<Json<Screen>> {
    let screen = sqlx::query_as::<_, Screen>(
        "SELECT name, screen_type, config, created_by, created_at, updated_at
         FROM screens
         WHERE name = $1",
    )
    .bind(&name)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ScreenError::NotFound(name))?;

    Ok(Json(screen))
}

/// Create a new screen.
#[span_fn]
pub async fn create_screen(
    Extension(pool): Extension<PgPool>,
    Json(request): Json<CreateScreenRequest>,
) -> ScreenResult<(StatusCode, Json<Screen>)> {
    // Normalize and validate name
    let name = normalize_screen_name(&request.name);
    validate_screen_name(&name)?;

    // Validate screen type
    let _screen_type: ScreenType = request.screen_type.parse().map_err(|_| {
        ScreenError::BadRequest(ErrorResponse::new(
            "INVALID_SCREEN_TYPE",
            &format!("Invalid screen type: {}", request.screen_type),
        ))
    })?;

    // Check for duplicate name
    let exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM screens WHERE name = $1)")
            .bind(&name)
            .fetch_one(&pool)
            .await?;

    if exists {
        return Err(ScreenError::BadRequest(ErrorResponse::new(
            "DUPLICATE_NAME",
            &format!("Screen with name '{name}' already exists"),
        )));
    }

    // Insert screen
    let screen = sqlx::query_as::<_, Screen>(
        "INSERT INTO screens (name, screen_type, config)
         VALUES ($1, $2, $3)
         RETURNING name, screen_type, config, created_by, created_at, updated_at",
    )
    .bind(&name)
    .bind(&request.screen_type)
    .bind(&request.config)
    .fetch_one(&pool)
    .await?;

    info!("Created screen: {}", name);
    Ok((StatusCode::CREATED, Json(screen)))
}

/// Update an existing screen.
#[span_fn]
pub async fn update_screen(
    Extension(pool): Extension<PgPool>,
    Path(name): Path<String>,
    Json(request): Json<UpdateScreenRequest>,
) -> ScreenResult<Json<Screen>> {
    let screen = sqlx::query_as::<_, Screen>(
        "UPDATE screens
         SET config = $1, updated_at = NOW()
         WHERE name = $2
         RETURNING name, screen_type, config, created_by, created_at, updated_at",
    )
    .bind(&request.config)
    .bind(&name)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ScreenError::NotFound(name.clone()))?;

    info!("Updated screen: {}", name);
    Ok(Json(screen))
}

/// Delete a screen.
#[span_fn]
pub async fn delete_screen(
    Extension(pool): Extension<PgPool>,
    Path(name): Path<String>,
) -> ScreenResult<StatusCode> {
    let result = sqlx::query("DELETE FROM screens WHERE name = $1")
        .bind(&name)
        .execute(&pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ScreenError::NotFound(name));
    }

    info!("Deleted screen: {}", name);
    Ok(StatusCode::NO_CONTENT)
}
