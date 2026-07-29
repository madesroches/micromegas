//! Integration tests for the app_db schema migration (v3 -> v4: folders table +
//! screens.folder_path column).
//!
//! Requires `MICROMEGAS_APP_SQL_CONNECTION_STRING` to point at a live
//! `micromegas_app` database (e.g. from `local_test_env`). Destructive: drops and
//! recreates the schema from scratch. Not run by default CI — run manually:
//! `cargo test --test migration_test -- --ignored`

use analytics_web_srv::app_db::execute_migration;
use analytics_web_srv::app_db::schema::{
    add_screens_managed_by, create_data_sources_table, create_tables,
};
use analytics_web_srv::app_db::update_schema_version;
use sqlx::Row;

async fn connect() -> sqlx::PgPool {
    let conn_str = std::env::var("MICROMEGAS_APP_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_APP_SQL_CONNECTION_STRING must be set to run this test");
    sqlx::PgPool::connect(&conn_str)
        .await
        .expect("connecting to micromegas_app")
}

/// Drops any existing tables and rebuilds the v3 schema directly via the schema
/// construction functions, pinning the migration table at version 3.
async fn reset_to_v3(pool: &sqlx::PgPool) {
    sqlx::query("DROP TABLE IF EXISTS screens, data_sources, folders, migration CASCADE")
        .execute(pool)
        .await
        .expect("dropping existing tables");

    let mut tr = pool.begin().await.expect("begin transaction");
    create_tables(&mut tr).await.expect("create v1 schema");
    create_data_sources_table(&mut tr)
        .await
        .expect("create data_sources table (v2)");
    add_screens_managed_by(&mut tr)
        .await
        .expect("add managed_by column (v3)");
    update_schema_version(&mut tr, 3)
        .await
        .expect("pin schema version at 3");
    tr.commit().await.expect("commit v3 schema");
}

async fn schema_version(pool: &sqlx::PgPool) -> i32 {
    sqlx::query("SELECT version FROM migration")
        .fetch_one(pool)
        .await
        .expect("reading migration version")
        .get("version")
}

#[ignore]
#[tokio::test]
async fn migration_v3_to_v4_adds_folders_table_and_defaults_existing_screens_to_root() {
    let pool = connect().await;
    reset_to_v3(&pool).await;

    sqlx::query(
        "INSERT INTO screens (name, screen_type, config, created_by, updated_by)
         VALUES ($1, $2, $3, $4, $4)",
    )
    .bind("pre-migration-screen")
    .bind("notebook")
    .bind(serde_json::json!({}))
    .bind("test")
    .execute(&pool)
    .await
    .expect("seeding a pre-migration screen");

    execute_migration(pool.clone())
        .await
        .expect("migrating v3 -> v4");

    assert_eq!(schema_version(&pool).await, 4);

    let folder_path: String = sqlx::query("SELECT folder_path FROM screens WHERE name = $1")
        .bind("pre-migration-screen")
        .fetch_one(&pool)
        .await
        .expect("reading folder_path of pre-existing screen")
        .get("folder_path");
    assert_eq!(
        folder_path, "",
        "pre-existing screens must default to the root folder"
    );

    // The folders table must exist and be usable.
    sqlx::query("INSERT INTO folders (path, created_by) VALUES ($1, $2)")
        .bind("smoke-test-folder")
        .bind("test")
        .execute(&pool)
        .await
        .expect("folders table should exist and accept inserts after migration");
}

#[ignore]
#[tokio::test]
async fn migration_is_idempotent_once_at_latest_version() {
    let pool = connect().await;
    reset_to_v3(&pool).await;

    execute_migration(pool.clone())
        .await
        .expect("first migration run (v3 -> v4)");
    execute_migration(pool.clone())
        .await
        .expect("second migration run at v4 should be a no-op");

    assert_eq!(schema_version(&pool).await, 4);
}
