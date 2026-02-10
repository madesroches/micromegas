use analytics_web_srv::app_db::{
    CreateDataSourceRequest, DataSource, DataSourceSummary, UpdateDataSourceRequest,
    ValidationError, validate_data_source_config,
};
use analytics_web_srv::auth::ValidatedUser;
use analytics_web_srv::data_source_cache::DataSourceCache;
use axum::{
    Extension, Json,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use micromegas::tracing::prelude::*;
use serde::Serialize;
use sqlx::PgPool;

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

pub enum DataSourceError {
    NotFound(String),
    BadRequest(ErrorResponse),
    Forbidden,
    Database(sqlx::Error),
}

impl IntoResponse for DataSourceError {
    fn into_response(self) -> Response {
        match self {
            DataSourceError::NotFound(name) => {
                let body =
                    ErrorResponse::new("NOT_FOUND", &format!("Data source '{name}' not found"));
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            DataSourceError::BadRequest(err) => {
                (StatusCode::BAD_REQUEST, Json(err)).into_response()
            }
            DataSourceError::Forbidden => {
                let body = ErrorResponse::new("FORBIDDEN", "Admin access required");
                (StatusCode::FORBIDDEN, Json(body)).into_response()
            }
            DataSourceError::Database(err) => {
                error!("Database error: {err}");
                let body = ErrorResponse::new("DATABASE_ERROR", "Internal database error");
                (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
            }
        }
    }
}

impl From<sqlx::Error> for DataSourceError {
    fn from(err: sqlx::Error) -> Self {
        DataSourceError::Database(err)
    }
}

impl From<ValidationError> for DataSourceError {
    fn from(err: ValidationError) -> Self {
        DataSourceError::BadRequest(ErrorResponse::new(&err.code, &err.message))
    }
}

type DataSourceResult<T> = Result<T, DataSourceError>;

fn require_admin(user: &ValidatedUser) -> Result<(), DataSourceError> {
    if !user.is_admin {
        return Err(DataSourceError::Forbidden);
    }
    Ok(())
}

/// GET /api/data-sources — list names and default flag (any authenticated user).
#[span_fn]
pub async fn list_data_sources(
    Extension(pool): Extension<PgPool>,
) -> DataSourceResult<Json<Vec<DataSourceSummary>>> {
    let rows = sqlx::query_as::<_, (String, bool)>(
        "SELECT name, is_default FROM data_sources ORDER BY name",
    )
    .fetch_all(&pool)
    .await?;

    let summaries: Vec<DataSourceSummary> = rows
        .into_iter()
        .map(|(name, is_default)| DataSourceSummary { name, is_default })
        .collect();

    Ok(Json(summaries))
}

/// GET /api/data-sources/{name} — get full details (admin only).
#[span_fn]
pub async fn get_data_source(
    Extension(pool): Extension<PgPool>,
    Extension(user): Extension<ValidatedUser>,
    Path(name): Path<String>,
) -> DataSourceResult<Json<DataSource>> {
    require_admin(&user)?;

    let ds = sqlx::query_as::<_, DataSource>(
        "SELECT name, config, is_default, created_by, updated_by, created_at, updated_at
         FROM data_sources
         WHERE name = $1",
    )
    .bind(&name)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| DataSourceError::NotFound(name))?;

    Ok(Json(ds))
}

/// POST /api/data-sources — create (admin only).
#[span_fn]
pub async fn create_data_source(
    Extension(pool): Extension<PgPool>,
    Extension(user): Extension<ValidatedUser>,
    Extension(cache): Extension<DataSourceCache>,
    Json(request): Json<CreateDataSourceRequest>,
) -> DataSourceResult<(StatusCode, Json<DataSource>)> {
    require_admin(&user)?;

    let name = &request.name;
    if name.trim().is_empty() {
        return Err(ValidationError::new("NAME_EMPTY", "Name must not be empty").into());
    }
    validate_data_source_config(&request.config)?;

    let user_id = user.email.as_deref().unwrap_or(&user.subject);

    let mut tx = pool.begin().await?;

    // If this is the first data source, auto-set is_default
    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM data_sources")
        .fetch_one(&mut *tx)
        .await?;
    let is_default = if count == 0 { true } else { request.is_default };

    // If is_default, clear default on other rows
    if is_default {
        sqlx::query("UPDATE data_sources SET is_default = FALSE WHERE is_default = TRUE")
            .execute(&mut *tx)
            .await?;
    }

    let ds = sqlx::query_as::<_, DataSource>(
        "INSERT INTO data_sources (name, config, is_default, created_by, updated_by)
         VALUES ($1, $2, $3, $4, $4)
         RETURNING name, config, is_default, created_by, updated_by, created_at, updated_at",
    )
    .bind(name)
    .bind(&request.config)
    .bind(is_default)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    cache.invalidate(name).await;

    info!("Created data source: {name} by {user_id}");
    Ok((StatusCode::CREATED, Json(ds)))
}

/// PUT /api/data-sources/{name} — update config and/or transfer default (admin only).
#[span_fn]
pub async fn update_data_source(
    Extension(pool): Extension<PgPool>,
    Extension(user): Extension<ValidatedUser>,
    Extension(cache): Extension<DataSourceCache>,
    Path(name): Path<String>,
    Json(request): Json<UpdateDataSourceRequest>,
) -> DataSourceResult<Json<DataSource>> {
    require_admin(&user)?;

    if let Some(ref config) = request.config {
        validate_data_source_config(config)?;
    }

    let user_id = user.email.as_deref().unwrap_or(&user.subject);

    let mut tx = pool.begin().await?;

    // Fetch current row
    let current = sqlx::query_as::<_, DataSource>(
        "SELECT name, config, is_default, created_by, updated_by, created_at, updated_at
         FROM data_sources WHERE name = $1 FOR UPDATE",
    )
    .bind(&name)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DataSourceError::NotFound(name.clone()))?;

    // Reject removing the default flag directly
    if let Some(false) = request.is_default
        && current.is_default
    {
        return Err(DataSourceError::BadRequest(ErrorResponse::new(
            "CANNOT_REMOVE_DEFAULT",
            "Cannot remove default flag — set another data source as default instead",
        )));
    }

    let new_is_default = request.is_default.unwrap_or(current.is_default);
    let new_config = request.config.as_ref().unwrap_or(&current.config);

    // If setting as default, clear default on other rows
    if new_is_default && !current.is_default {
        sqlx::query("UPDATE data_sources SET is_default = FALSE WHERE is_default = TRUE")
            .execute(&mut *tx)
            .await?;
    }

    let ds = sqlx::query_as::<_, DataSource>(
        "UPDATE data_sources
         SET config = $1, is_default = $2, updated_by = $3, updated_at = NOW()
         WHERE name = $4
         RETURNING name, config, is_default, created_by, updated_by, created_at, updated_at",
    )
    .bind(new_config)
    .bind(new_is_default)
    .bind(user_id)
    .bind(&name)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    cache.invalidate(&name).await;

    info!("Updated data source: {name} by {user_id}");
    Ok(Json(ds))
}

/// DELETE /api/data-sources/{name} — delete (admin only).
#[span_fn]
pub async fn delete_data_source(
    Extension(pool): Extension<PgPool>,
    Extension(user): Extension<ValidatedUser>,
    Extension(cache): Extension<DataSourceCache>,
    Path(name): Path<String>,
) -> DataSourceResult<StatusCode> {
    require_admin(&user)?;

    let mut tx = pool.begin().await?;

    // Check + delete in a transaction to prevent TOCTOU race
    let current = sqlx::query_as::<_, DataSource>(
        "SELECT name, config, is_default, created_by, updated_by, created_at, updated_at
         FROM data_sources WHERE name = $1 FOR UPDATE",
    )
    .bind(&name)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DataSourceError::NotFound(name.clone()))?;

    if current.is_default {
        return Err(DataSourceError::BadRequest(ErrorResponse::new(
            "CANNOT_DELETE_DEFAULT",
            "Cannot delete the default data source — set another data source as default first",
        )));
    }

    sqlx::query("DELETE FROM data_sources WHERE name = $1")
        .bind(&name)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    cache.invalidate(&name).await;

    let user_id = user.email.as_deref().unwrap_or(&user.subject);
    info!("Deleted data source: {name} by {user_id}");
    Ok(StatusCode::NO_CONTENT)
}
