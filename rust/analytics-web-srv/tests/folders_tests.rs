//! Tests for folders.rs handlers.
//!
//! The root/self-nesting guard tests run against a lazily-connected pool (they
//! return before touching the database, same trick as
//! `ingestion/tests/readiness.rs`) and are part of default CI.
//!
//! The rest require a live `micromegas_app` database
//! (`MICROMEGAS_APP_SQL_CONNECTION_STRING`) and are `#[ignore]`d — run manually:
//! `cargo test --test folders_tests -- --ignored`

use analytics_web_srv::app_db::{CreateFolderRequest, CreateScreenRequest, UpdateFolderRequest};
use analytics_web_srv::auth::ValidatedUser;
use analytics_web_srv::folders::{DeleteFolderParams, create_folder, delete_folder, update_folder};
use analytics_web_srv::screens::create_screen;
use axum::extract::{Extension, Json, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::time::Duration;

fn lazy_pool() -> sqlx::PgPool {
    sqlx::PgPool::connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible")
}

fn test_user() -> ValidatedUser {
    ValidatedUser {
        subject: "test-subject".to_string(),
        email: Some("test@example.com".to_string()),
        issuer: "test".to_string(),
        is_admin: true,
    }
}

async fn error_code(resp: axum::response::Response) -> String {
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("reading error body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parsing error body json");
    json["code"]
        .as_str()
        .expect("error body has a code field")
        .to_string()
}

// ---------------------------------------------------------------------------
// Guard checks that never touch the database (root / self-nesting) — no
// live Postgres required.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_folder_rejects_root_as_source() {
    let result = update_folder(
        Extension(lazy_pool()),
        Json(UpdateFolderRequest {
            path: "".to_string(),
            new_path: "team".to_string(),
        }),
    )
    .await;
    let code = error_code(
        result
            .expect_err("root rename must be rejected")
            .into_response(),
    )
    .await;
    assert_eq!(code, "ROOT_NOT_ALLOWED");
}

#[tokio::test]
async fn update_folder_rejects_self_nesting() {
    let result = update_folder(
        Extension(lazy_pool()),
        Json(UpdateFolderRequest {
            path: "team".to_string(),
            new_path: "team/archive/team".to_string(),
        }),
    )
    .await;
    let code = error_code(
        result
            .expect_err("moving a folder into its own descendant must be rejected")
            .into_response(),
    )
    .await;
    assert_eq!(code, "SELF_NESTING");
}

#[tokio::test]
async fn update_folder_rejects_renaming_to_itself() {
    let result = update_folder(
        Extension(lazy_pool()),
        Json(UpdateFolderRequest {
            path: "team".to_string(),
            new_path: "team".to_string(),
        }),
    )
    .await;
    let code = error_code(
        result
            .expect_err("renaming a folder to itself must be rejected")
            .into_response(),
    )
    .await;
    assert_eq!(code, "SELF_NESTING");
}

#[tokio::test]
async fn delete_folder_rejects_root() {
    let result = delete_folder(
        Extension(lazy_pool()),
        Query(DeleteFolderParams {
            path: "".to_string(),
        }),
    )
    .await;
    let code = error_code(
        result
            .expect_err("root delete must be rejected")
            .into_response(),
    )
    .await;
    assert_eq!(code, "ROOT_NOT_ALLOWED");
}

// ---------------------------------------------------------------------------
// DB-backed tests — require MICROMEGAS_APP_SQL_CONNECTION_STRING.
// ---------------------------------------------------------------------------

async fn connect() -> sqlx::PgPool {
    let conn_str = std::env::var("MICROMEGAS_APP_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_APP_SQL_CONNECTION_STRING must be set to run this test");
    let pool = sqlx::PgPool::connect(&conn_str)
        .await
        .expect("connecting to micromegas_app");
    analytics_web_srv::app_db::execute_migration(pool.clone())
        .await
        .expect("running migrations");
    pool
}

async fn clear_tables(pool: &sqlx::PgPool) {
    sqlx::query("TRUNCATE screens, folders")
        .execute(pool)
        .await
        .expect("truncating screens/folders");
}

fn screen_request(name: &str, folder_path: &str) -> CreateScreenRequest {
    CreateScreenRequest {
        name: name.to_string(),
        screen_type: "notebook".to_string(),
        config: serde_json::json!({}),
        managed_by: None,
        folder_path: folder_path.to_string(),
    }
}

#[ignore]
#[tokio::test]
async fn create_folder_is_idempotent() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let (status, _) = create_folder(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateFolderRequest {
            path: "team".to_string(),
        }),
    )
    .await
    .expect("first create");
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = create_folder(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateFolderRequest {
            path: "team".to_string(),
        }),
    )
    .await
    .expect("second create of the same path must be a no-op, not an error");
    assert_eq!(status, StatusCode::CREATED);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM folders WHERE path = $1")
        .bind("team")
        .fetch_one(&pool)
        .await
        .expect("counting folder rows");
    assert_eq!(count, 1);
}

// Proves create_folder actually takes a `pg_advisory_xact_lock(hashtext($1))`
// on the destination path (not just on an ancestor prefix, and not skipped
// altogether), by holding that lock key via a session-scoped advisory lock
// on a separate connection and observing create_folder block until it's
// released — same pattern as
// `create_screen_serializes_on_destination_folder_advisory_lock` in
// screens_tests.rs.
#[ignore]
#[tokio::test]
async fn create_folder_serializes_on_destination_advisory_lock() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let mut lock_conn = pool.acquire().await.expect("acquiring lock connection");
    sqlx::query("SELECT pg_advisory_lock(hashtext($1))")
        .bind("locked-folder")
        .execute(&mut *lock_conn)
        .await
        .expect("taking session advisory lock");

    let pool_for_task = pool.clone();
    let create_task = tokio::spawn(async move {
        create_folder(
            Extension(pool_for_task),
            Extension(test_user()),
            Json(CreateFolderRequest {
                path: "locked-folder".to_string(),
            }),
        )
        .await
    });

    // Poll (bounded) until the create_folder backend shows up waiting on a
    // lock, proving it contends on our held advisory lock instead of racing
    // past it.
    let mut waiting = false;
    for _ in 0..100 {
        if create_task.is_finished() {
            break;
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query ILIKE '%pg_advisory_xact_lock%'",
        )
        .fetch_one(&pool)
        .await
        .expect("checking pg_stat_activity");
        if count > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        waiting,
        "create_folder must block waiting on the destination path's advisory lock"
    );
    assert!(
        !create_task.is_finished(),
        "create_folder must not complete while the folder lock is held"
    );

    sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
        .bind("locked-folder")
        .execute(&mut *lock_conn)
        .await
        .expect("releasing session advisory lock");
    drop(lock_conn);

    let (status, _) = tokio::time::timeout(Duration::from_secs(5), create_task)
        .await
        .expect("create_folder should complete promptly after the lock is released")
        .expect("task join")
        .expect("create_folder should succeed once unblocked");
    assert_eq!(status, StatusCode::CREATED);
}

// Proves delete_folder locks the *ancestor* prefix chain, not just the exact
// path being deleted — mirroring create_folder's/update_folder's ancestor-
// chain locking. Without this, a concurrent rename of an ancestor (e.g.
// "team" -> "x") could commit between delete_folder's existence check and its
// DELETE, moving "team/sub" to "x/sub" and making the DELETE match zero rows
// while delete_folder still reports success. We prove the lock is actually
// taken on the ancestor "team" (not just on "team/sub") by holding a
// session-scoped advisory lock on "team" and observing delete_folder block
// until it's released.
#[ignore]
#[tokio::test]
async fn delete_folder_serializes_on_ancestor_advisory_lock() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let (status, _) = create_folder(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateFolderRequest {
            path: "team/sub".to_string(),
        }),
    )
    .await
    .expect("creating folder to delete");
    assert_eq!(status, StatusCode::CREATED);

    let mut lock_conn = pool.acquire().await.expect("acquiring lock connection");
    sqlx::query("SELECT pg_advisory_lock(hashtext($1))")
        .bind("team")
        .execute(&mut *lock_conn)
        .await
        .expect("taking session advisory lock on ancestor");

    let pool_for_task = pool.clone();
    let delete_task = tokio::spawn(async move {
        delete_folder(
            Extension(pool_for_task),
            Query(DeleteFolderParams {
                path: "team/sub".to_string(),
            }),
        )
        .await
    });

    // Poll (bounded) until the delete_folder backend shows up waiting on a
    // lock, proving it contends on the ancestor's advisory lock instead of
    // racing past it.
    let mut waiting = false;
    for _ in 0..100 {
        if delete_task.is_finished() {
            break;
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query ILIKE '%pg_advisory_xact_lock%'",
        )
        .fetch_one(&pool)
        .await
        .expect("checking pg_stat_activity");
        if count > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        waiting,
        "delete_folder must block waiting on the ancestor path's advisory lock"
    );
    assert!(
        !delete_task.is_finished(),
        "delete_folder must not complete while the ancestor lock is held"
    );

    sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
        .bind("team")
        .execute(&mut *lock_conn)
        .await
        .expect("releasing session advisory lock");
    drop(lock_conn);

    let status = tokio::time::timeout(Duration::from_secs(5), delete_task)
        .await
        .expect("delete_folder should complete promptly after the lock is released")
        .expect("task join")
        .expect("delete_folder should succeed once unblocked");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[ignore]
#[tokio::test]
async fn rename_folder_cascades_to_descendants_and_screens() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let _ = create_folder(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateFolderRequest {
            path: "team".to_string(),
        }),
    )
    .await
    .expect("create team");
    let _ = create_folder(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateFolderRequest {
            path: "team/archive".to_string(),
        }),
    )
    .await
    .expect("create team/archive");
    let _ = create_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(screen_request("widget", "team/archive")),
    )
    .await
    .expect("create screen under team/archive");

    let renamed = update_folder(
        Extension(pool.clone()),
        Json(UpdateFolderRequest {
            path: "team".to_string(),
            new_path: "squad".to_string(),
        }),
    )
    .await
    .expect("rename team -> squad");
    assert_eq!(renamed.0.path, "squad");

    let subfolder_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM folders WHERE path = $1)")
            .bind("squad/archive")
            .fetch_one(&pool)
            .await
            .expect("checking descendant folder");
    assert!(
        subfolder_exists,
        "descendant folder must be rewritten under the new path"
    );

    let screen_folder: String =
        sqlx::query_scalar("SELECT folder_path FROM screens WHERE name = $1")
            .bind("widget")
            .fetch_one(&pool)
            .await
            .expect("reading screen folder_path");
    assert_eq!(screen_folder, "squad/archive");
}

#[ignore]
#[tokio::test]
async fn rename_folder_conflict_is_rejected() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let _ = create_folder(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateFolderRequest {
            path: "team".to_string(),
        }),
    )
    .await
    .expect("create team");
    let _ = create_folder(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateFolderRequest {
            path: "squad".to_string(),
        }),
    )
    .await
    .expect("create squad");

    let result = update_folder(
        Extension(pool.clone()),
        Json(UpdateFolderRequest {
            path: "team".to_string(),
            new_path: "squad".to_string(),
        }),
    )
    .await;
    let code = error_code(
        result
            .expect_err("renaming onto an existing folder must be rejected")
            .into_response(),
    )
    .await;
    assert_eq!(code, "ALREADY_EXISTS");
}

#[ignore]
#[tokio::test]
async fn rename_missing_folder_returns_not_found() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let result = update_folder(
        Extension(pool.clone()),
        Json(UpdateFolderRequest {
            path: "does-not-exist".to_string(),
            new_path: "also-new".to_string(),
        }),
    )
    .await;
    assert!(matches!(
        result,
        Err(analytics_web_srv::folders::FolderError::NotFound(_))
    ));
}

#[ignore]
#[tokio::test]
async fn delete_folder_blocked_when_not_empty() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let _ = create_folder(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateFolderRequest {
            path: "team".to_string(),
        }),
    )
    .await
    .expect("create team");
    let _ = create_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(screen_request("widget2", "team")),
    )
    .await
    .expect("create screen under team");

    let result = delete_folder(
        Extension(pool.clone()),
        Query(DeleteFolderParams {
            path: "team".to_string(),
        }),
    )
    .await;
    let code = error_code(
        result
            .expect_err("delete must be blocked while the folder has a screen")
            .into_response(),
    )
    .await;
    assert_eq!(code, "FOLDER_NOT_EMPTY");
}

#[ignore]
#[tokio::test]
async fn delete_empty_folder_succeeds() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let _ = create_folder(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateFolderRequest {
            path: "empty-folder".to_string(),
        }),
    )
    .await
    .expect("create empty-folder");

    let status = delete_folder(
        Extension(pool.clone()),
        Query(DeleteFolderParams {
            path: "empty-folder".to_string(),
        }),
    )
    .await
    .expect("deleting an empty folder should succeed");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[ignore]
#[tokio::test]
async fn delete_ancestor_of_nested_explicit_folder_is_not_empty() {
    let pool = connect().await;
    clear_tables(&pool).await;

    // Only "team/sub" is ever created explicitly — "team" itself has no
    // `folders` row and no screens anywhere in its subtree. `GET /folders`
    // (via `compute_folder_infos`) reports "team" as existing and non-empty
    // because "team/sub" is a nested explicit folder; `delete_folder` must
    // agree instead of 404ing on `folder_exists`.
    let _ = create_folder(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateFolderRequest {
            path: "team/sub".to_string(),
        }),
    )
    .await
    .expect("create team/sub");

    let result = delete_folder(
        Extension(pool.clone()),
        Query(DeleteFolderParams {
            path: "team".to_string(),
        }),
    )
    .await;
    let code = error_code(
        result
            .expect_err("deleting an ancestor of a nested explicit folder must be rejected as non-empty, not 404")
            .into_response(),
    )
    .await;
    assert_eq!(code, "FOLDER_NOT_EMPTY");
}

#[ignore]
#[tokio::test]
async fn rename_ancestor_of_nested_explicit_folder_succeeds() {
    let pool = connect().await;
    clear_tables(&pool).await;

    // Same setup as above, but exercised through rename: "team" only exists
    // implicitly via the nested explicit "team/sub" row, so `folder_exists`
    // must recognize it as present rather than 404ing.
    let _ = create_folder(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateFolderRequest {
            path: "team/sub".to_string(),
        }),
    )
    .await
    .expect("create team/sub");

    let renamed = update_folder(
        Extension(pool.clone()),
        Json(UpdateFolderRequest {
            path: "team".to_string(),
            new_path: "squad".to_string(),
        }),
    )
    .await
    .expect("renaming an ancestor that exists only via a nested explicit folder should succeed");
    assert_eq!(renamed.0.path, "squad");

    let subfolder_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM folders WHERE path = $1)")
            .bind("squad/sub")
            .fetch_one(&pool)
            .await
            .expect("checking descendant folder");
    assert!(
        subfolder_exists,
        "nested explicit folder must be rewritten under the new path"
    );
}

#[ignore]
#[tokio::test]
async fn delete_missing_folder_returns_not_found() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let result = delete_folder(
        Extension(pool.clone()),
        Query(DeleteFolderParams {
            path: "does-not-exist".to_string(),
        }),
    )
    .await;
    assert!(matches!(
        result,
        Err(analytics_web_srv::folders::FolderError::NotFound(_))
    ));
}
