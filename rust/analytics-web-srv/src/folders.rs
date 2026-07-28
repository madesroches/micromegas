//! Handlers for the screen-folder hierarchy.
//!
//! A folder "exists" if it has a row in `folders` or appears as a prefix of some
//! `screens.folder_path` — see `folder_exists` and `compute_folder_infos`.

use crate::app_db::{
    CreateFolderRequest, Folder, FolderInfo, UpdateFolderRequest, ValidationError,
    validate_folder_path,
};
use crate::auth::ValidatedUser;
use axum::{
    Extension, Json,
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use micromegas::tracing::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;

/// Error response for folder operations.
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

/// Unified error type for folder handlers.
#[derive(Debug)]
pub enum FolderError {
    NotFound(String),
    BadRequest(ErrorResponse),
    Database(sqlx::Error),
}

impl IntoResponse for FolderError {
    fn into_response(self) -> Response {
        match self {
            FolderError::NotFound(path) => {
                let body = ErrorResponse::new("NOT_FOUND", &format!("Folder '{path}' not found"));
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            FolderError::BadRequest(err) => (StatusCode::BAD_REQUEST, Json(err)).into_response(),
            FolderError::Database(err) => {
                error!("Database error: {}", err);
                let body = ErrorResponse::new("DATABASE_ERROR", "Internal database error");
                (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
            }
        }
    }
}

impl From<sqlx::Error> for FolderError {
    fn from(err: sqlx::Error) -> Self {
        FolderError::Database(err)
    }
}

impl From<ValidationError> for FolderError {
    fn from(err: ValidationError) -> Self {
        FolderError::BadRequest(ErrorResponse::new(&err.code, &err.message))
    }
}

type FolderResult<T> = Result<T, FolderError>;

/// Query params for `DELETE /folders`.
#[derive(Debug, Deserialize)]
pub struct DeleteFolderParams {
    pub path: String,
}

// ============================================================================
// Folder-info derivation (pure, DB-agnostic — takes flat query results)
// ============================================================================

/// Expands a path into the list of itself and all of its ancestors, including
/// root (`""`). E.g. `"team/dashboards"` -> `["", "team", "team/dashboards"]`.
fn expand_prefixes(path: &str) -> Vec<String> {
    let mut result = vec![String::new()];
    if !path.is_empty() {
        let mut cur = String::new();
        for segment in path.split('/') {
            cur = if cur.is_empty() {
                segment.to_string()
            } else {
                format!("{cur}/{segment}")
            };
            result.push(cur.clone());
        }
    }
    result
}

fn ensure_entry(map: &mut HashMap<String, FolderInfo>, path: &str) {
    map.entry(path.to_string()).or_insert_with(|| FolderInfo {
        path: path.to_string(),
        screen_count: 0,
        subfolder_count: 0,
    });
}

/// Computes the union of explicit `folders` rows and implicit prefixes derived
/// from `screens.folder_path`, with recursive `screen_count` and direct
/// `subfolder_count`. Root (`""`) is excluded from the result.
fn compute_folder_infos(
    folder_paths: &[String],
    screen_folder_paths: &[String],
) -> Vec<FolderInfo> {
    let mut map: HashMap<String, FolderInfo> = HashMap::new();
    ensure_entry(&mut map, "");

    for path in folder_paths {
        for prefix in expand_prefixes(path) {
            ensure_entry(&mut map, &prefix);
        }
    }

    for folder_path in screen_folder_paths {
        for prefix in expand_prefixes(folder_path) {
            ensure_entry(&mut map, &prefix);
            map.get_mut(&prefix)
                .expect("just inserted above")
                .screen_count += 1;
        }
    }

    // subfolder_count: direct children only, over a snapshot of paths already
    // known at this point (every path's parent is guaranteed present, since
    // expand_prefixes always walked down from root).
    let existing_paths: Vec<String> = map.keys().cloned().collect();
    for path in &existing_paths {
        if path.is_empty() {
            continue;
        }
        let parent = match path.rfind('/') {
            Some(idx) => path[..idx].to_string(),
            None => String::new(),
        };
        ensure_entry(&mut map, &parent);
        map.get_mut(&parent)
            .expect("just inserted above")
            .subfolder_count += 1;
    }

    let mut result: Vec<FolderInfo> = map.into_values().filter(|f| !f.path.is_empty()).collect();
    result.sort_by(|a, b| a.path.cmp(&b.path));
    result
}

/// Whether `path` "exists" — an explicit `folders` row (including a nested
/// explicit subfolder of `path`), or a prefix of some `screens.folder_path`
/// (covers folders that were never explicitly created but contain screens).
async fn folder_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    path: &str,
) -> Result<bool, sqlx::Error> {
    if path.is_empty() {
        return Ok(true);
    }
    let prefix_pattern = format!("{path}/%");
    let explicit = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM folders WHERE path = $1 OR path LIKE $2)",
    )
    .bind(path)
    .bind(&prefix_pattern)
    .fetch_one(&mut **tx)
    .await?;
    if explicit {
        return Ok(true);
    }
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM screens WHERE folder_path = $1 OR folder_path LIKE $2)",
    )
    .bind(path)
    .bind(&prefix_pattern)
    .fetch_one(&mut **tx)
    .await
}

// ============================================================================
// Handlers
// ============================================================================

/// GET /folders — list all folders (explicit + implicit), with derived counts.
#[span_fn]
pub async fn list_folders(
    Extension(pool): Extension<PgPool>,
) -> FolderResult<Json<Vec<FolderInfo>>> {
    let folder_paths = instrument_named!(
        sqlx::query_scalar::<_, String>("SELECT path FROM folders").fetch_all(&pool),
        "sql_select_folders"
    )
    .await?;

    let screen_folder_paths = instrument_named!(
        sqlx::query_scalar::<_, String>("SELECT folder_path FROM screens").fetch_all(&pool),
        "sql_select_screen_folder_paths"
    )
    .await?;

    Ok(Json(compute_folder_infos(
        &folder_paths,
        &screen_folder_paths,
    )))
}

/// POST /folders — create a folder. Idempotent: creating an already-existing
/// path is a no-op, not an error.
#[span_fn]
pub async fn create_folder(
    Extension(pool): Extension<PgPool>,
    Extension(user): Extension<ValidatedUser>,
    Json(request): Json<CreateFolderRequest>,
) -> FolderResult<(StatusCode, Json<Folder>)> {
    validate_folder_path(&request.path)?;

    let user_id = user.email.as_deref().unwrap_or(&user.subject);

    let mut tx = pool.begin().await?;

    // Lock the destination path *and all of its ancestors* so a concurrent
    // delete/rename of any ancestor folder can't race past this create and
    // leave a folder that was just reported deleted/renamed reappearing
    // implicitly (via `compute_folder_infos`'s prefix derivation). Locked
    // shortest-prefix-first (root-to-leaf), matching `update_folder`'s
    // deadlock-avoidance convention.
    for lock_path in expand_prefixes(&request.path)
        .into_iter()
        .filter(|p| !p.is_empty())
    {
        instrument_named!(
            sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
                .bind(&lock_path)
                .execute(&mut *tx),
            "sql_folder_advisory_lock"
        )
        .await?;
    }

    instrument_named!(
        sqlx::query(
            "INSERT INTO folders (path, created_by) VALUES ($1, $2) ON CONFLICT (path) DO NOTHING"
        )
        .bind(&request.path)
        .bind(user_id)
        .execute(&mut *tx),
        "sql_insert_folder"
    )
    .await?;

    tx.commit().await?;

    info!("Created folder: {} by {}", request.path, user_id);
    Ok((StatusCode::CREATED, Json(Folder { path: request.path })))
}

/// PUT /folders — rename/move a folder (materialized-path prefix rewrite).
#[span_fn]
pub async fn update_folder(
    Extension(pool): Extension<PgPool>,
    Json(request): Json<UpdateFolderRequest>,
) -> FolderResult<Json<Folder>> {
    validate_folder_path(&request.path)?;
    validate_folder_path(&request.new_path)?;

    if request.path.is_empty() {
        return Err(FolderError::BadRequest(ErrorResponse::new(
            "ROOT_NOT_ALLOWED",
            "The root folder cannot be renamed or moved",
        )));
    }

    if request.new_path == request.path
        || request.new_path.starts_with(&format!("{}/", request.path))
    {
        return Err(FolderError::BadRequest(ErrorResponse::new(
            "SELF_NESTING",
            "Cannot move a folder into itself or one of its own descendants",
        )));
    }

    let mut tx = pool.begin().await?;

    // Lock the source and destination paths *and all of their ancestors* so
    // that two concurrent renames targeting the same destination (e.g.
    // "team1" -> "target" and "team2" -> "target"), or a concurrent
    // delete/rename of an ancestor of either path, serialize against this
    // rename instead of racing past the `folder_exists` checks below. Lock
    // in a fixed order (sorted, deduped) to avoid lock-order deadlocks
    // between concurrent renames that cross paths.
    let mut lock_paths: Vec<String> = expand_prefixes(&request.path)
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    lock_paths.extend(
        expand_prefixes(&request.new_path)
            .into_iter()
            .filter(|p| !p.is_empty()),
    );
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

    if !folder_exists(&mut tx, &request.path).await? {
        return Err(FolderError::NotFound(request.path.clone()));
    }

    if folder_exists(&mut tx, &request.new_path).await? {
        return Err(FolderError::BadRequest(ErrorResponse::new(
            "ALREADY_EXISTS",
            &format!("Folder '{}' already exists", request.new_path),
        )));
    }

    let prefix_pattern = format!("{}/%", request.path);

    instrument_named!(
        sqlx::query("UPDATE folders SET path = $1 WHERE path = $2")
            .bind(&request.new_path)
            .bind(&request.path)
            .execute(&mut *tx),
        "sql_rename_folder"
    )
    .await?;

    instrument_named!(
        sqlx::query(
            "UPDATE folders SET path = $1 || '/' || substring(path from length($2)+2) WHERE path LIKE $3"
        )
        .bind(&request.new_path)
        .bind(&request.path)
        .bind(&prefix_pattern)
        .execute(&mut *tx),
        "sql_rename_subfolders"
    )
    .await?;

    instrument_named!(
        sqlx::query("UPDATE screens SET folder_path = $1 WHERE folder_path = $2")
            .bind(&request.new_path)
            .bind(&request.path)
            .execute(&mut *tx),
        "sql_move_screens"
    )
    .await?;

    instrument_named!(
        sqlx::query(
            "UPDATE screens SET folder_path = $1 || '/' || substring(folder_path from length($2)+2) WHERE folder_path LIKE $3"
        )
        .bind(&request.new_path)
        .bind(&request.path)
        .bind(&prefix_pattern)
        .execute(&mut *tx),
        "sql_move_descendant_screens"
    )
    .await?;

    tx.commit().await?;

    info!("Moved folder: {} -> {}", request.path, request.new_path);
    Ok(Json(Folder {
        path: request.new_path,
    }))
}

/// DELETE /folders?path=... — delete a folder. Requires it to be empty (no
/// screens, no subfolders).
#[span_fn]
pub async fn delete_folder(
    Extension(pool): Extension<PgPool>,
    Query(params): Query<DeleteFolderParams>,
) -> FolderResult<StatusCode> {
    let path = params.path;
    validate_folder_path(&path)?;

    if path.is_empty() {
        return Err(FolderError::BadRequest(ErrorResponse::new(
            "ROOT_NOT_ALLOWED",
            "The root folder cannot be deleted",
        )));
    }

    let mut tx = pool.begin().await?;

    // Lock the path *and all of its ancestors* so a concurrent rename of any
    // ancestor folder can't race past this delete and silently no-op it (the
    // rename's UPDATE would relocate this path out from under the DELETE's
    // WHERE clause, leaving the folder still existing under its new path
    // while this handler reports it deleted). Locked shortest-prefix-first
    // (root-to-leaf), matching `create_folder`'s and `update_folder`'s
    // deadlock-avoidance convention.
    for lock_path in expand_prefixes(&path).into_iter().filter(|p| !p.is_empty()) {
        instrument_named!(
            sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
                .bind(&lock_path)
                .execute(&mut *tx),
            "sql_folder_advisory_lock"
        )
        .await?;
    }

    if !folder_exists(&mut tx, &path).await? {
        return Err(FolderError::NotFound(path));
    }

    let prefix_pattern = format!("{path}/%");
    let not_empty = instrument_named!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM screens WHERE folder_path = $1 OR folder_path LIKE $2)
                OR EXISTS(SELECT 1 FROM folders WHERE path LIKE $2)"
        )
        .bind(&path)
        .bind(&prefix_pattern)
        .fetch_one(&mut *tx),
        "sql_folder_nonempty_check"
    )
    .await?;

    if not_empty {
        return Err(FolderError::BadRequest(ErrorResponse::new(
            "FOLDER_NOT_EMPTY",
            "Folder must be empty (no screens, no subfolders) before it can be deleted",
        )));
    }

    instrument_named!(
        sqlx::query("DELETE FROM folders WHERE path = $1")
            .bind(&path)
            .execute(&mut *tx),
        "sql_delete_folder"
    )
    .await?;

    tx.commit().await?;

    info!("Deleted folder: {}", path);
    Ok(StatusCode::NO_CONTENT)
}
