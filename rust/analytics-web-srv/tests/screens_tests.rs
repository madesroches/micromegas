//! Integration tests for screens.rs — folder_path on create, and partial
//! update_screen (config-only, folder_path-only, or both).
//!
//! Requires a live `micromegas_app` database (`MICROMEGAS_APP_SQL_CONNECTION_STRING`).
//! Not run by default CI — run manually: `cargo test --test screens_tests -- --ignored`

use analytics_web_srv::app_db::{CreateScreenRequest, UpdateScreenRequest};
use analytics_web_srv::auth::ValidatedUser;
use analytics_web_srv::screens::{create_screen, update_screen};
use axum::extract::{Extension, Json, Path};
use axum::http::StatusCode;
use std::time::Duration;

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

fn test_user() -> ValidatedUser {
    ValidatedUser {
        subject: "test-subject".to_string(),
        email: Some("test@example.com".to_string()),
        issuer: "test".to_string(),
        is_admin: true,
    }
}

#[ignore]
#[tokio::test]
async fn create_screen_stores_folder_path() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let (_, screen) = create_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateScreenRequest {
            name: "filed-screen".to_string(),
            screen_type: "notebook".to_string(),
            config: serde_json::json!({"a": 1}),
            managed_by: None,
            folder_path: "team/dashboards".to_string(),
        }),
    )
    .await
    .expect("create screen with folder_path");
    assert_eq!(screen.0.folder_path, "team/dashboards");
}

#[ignore]
#[tokio::test]
async fn create_screen_defaults_folder_path_to_root() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let (_, screen) = create_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateScreenRequest {
            name: "root-screen".to_string(),
            screen_type: "notebook".to_string(),
            config: serde_json::json!({}),
            managed_by: None,
            folder_path: "".to_string(),
        }),
    )
    .await
    .expect("create screen with default folder_path");
    assert_eq!(screen.0.folder_path, "");
}

#[ignore]
#[tokio::test]
async fn update_screen_folder_path_only_leaves_config_untouched() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let _ = create_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateScreenRequest {
            name: "move-me".to_string(),
            screen_type: "notebook".to_string(),
            config: serde_json::json!({"keep": "me"}),
            managed_by: None,
            folder_path: "".to_string(),
        }),
    )
    .await
    .expect("create screen");

    let updated = update_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Path("move-me".to_string()),
        Json(UpdateScreenRequest {
            config: None,
            managed_by: None,
            folder_path: Some("team/new-home".to_string()),
        }),
    )
    .await
    .expect("folder-only move");

    assert_eq!(updated.0.folder_path, "team/new-home");
    assert_eq!(updated.0.config, serde_json::json!({"keep": "me"}));
}

#[ignore]
#[tokio::test]
async fn update_screen_config_only_leaves_folder_path_untouched() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let _ = create_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateScreenRequest {
            name: "keep-folder".to_string(),
            screen_type: "notebook".to_string(),
            config: serde_json::json!({}),
            managed_by: None,
            folder_path: "team".to_string(),
        }),
    )
    .await
    .expect("create screen");

    let updated = update_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Path("keep-folder".to_string()),
        Json(UpdateScreenRequest {
            config: Some(serde_json::json!({"updated": true})),
            managed_by: None,
            folder_path: None,
        }),
    )
    .await
    .expect("config-only update");

    assert_eq!(updated.0.folder_path, "team");
    assert_eq!(updated.0.config, serde_json::json!({"updated": true}));
}

#[ignore]
#[tokio::test]
async fn update_screen_can_change_config_and_folder_path_together() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let _ = create_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateScreenRequest {
            name: "both-fields".to_string(),
            screen_type: "notebook".to_string(),
            config: serde_json::json!({}),
            managed_by: None,
            folder_path: "".to_string(),
        }),
    )
    .await
    .expect("create screen");

    let updated = update_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Path("both-fields".to_string()),
        Json(UpdateScreenRequest {
            config: Some(serde_json::json!({"new": "config"})),
            managed_by: None,
            folder_path: Some("team/moved".to_string()),
        }),
    )
    .await
    .expect("combined update");

    assert_eq!(updated.0.folder_path, "team/moved");
    assert_eq!(updated.0.config, serde_json::json!({"new": "config"}));
}

#[ignore]
#[tokio::test]
async fn update_screen_rejects_invalid_folder_path() {
    let pool = connect().await;
    clear_tables(&pool).await;

    let _ = create_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Json(CreateScreenRequest {
            name: "invalid-move".to_string(),
            screen_type: "notebook".to_string(),
            config: serde_json::json!({}),
            managed_by: None,
            folder_path: "".to_string(),
        }),
    )
    .await
    .expect("create screen");

    let result = update_screen(
        Extension(pool.clone()),
        Extension(test_user()),
        Path("invalid-move".to_string()),
        Json(UpdateScreenRequest {
            config: None,
            managed_by: None,
            folder_path: Some("Team_Invalid".to_string()),
        }),
    )
    .await;
    assert!(result.is_err(), "invalid folder_path must be rejected");
}

// Proves create_screen actually takes the same `pg_advisory_xact_lock(hashtext($1))`
// on the destination folder_path that `update_folder`/`delete_folder` take in
// folders.rs, by holding that lock key via a session-scoped advisory lock on a
// separate connection and observing create_screen block until it's released.
#[ignore]
#[tokio::test]
async fn create_screen_serializes_on_destination_folder_advisory_lock() {
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
        create_screen(
            Extension(pool_for_task),
            Extension(test_user()),
            Json(CreateScreenRequest {
                name: "blocked-widget".to_string(),
                screen_type: "notebook".to_string(),
                config: serde_json::json!({}),
                managed_by: None,
                folder_path: "locked-folder".to_string(),
            }),
        )
        .await
    });

    // Poll (bounded) until the create_screen backend shows up waiting on a
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
        "create_screen must block waiting on the destination folder's advisory lock"
    );
    assert!(
        !create_task.is_finished(),
        "create_screen must not complete while the folder lock is held"
    );

    sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
        .bind("locked-folder")
        .execute(&mut *lock_conn)
        .await
        .expect("releasing session advisory lock");
    drop(lock_conn);

    let (status, screen) = tokio::time::timeout(Duration::from_secs(5), create_task)
        .await
        .expect("create_screen should complete promptly after the lock is released")
        .expect("task join")
        .expect("create_screen should succeed once unblocked");
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(screen.0.folder_path, "locked-folder");
}
