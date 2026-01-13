use crate::app_db::schema::create_tables;
use anyhow::{Context, Result};
use micromegas::tracing::prelude::*;
use sqlx::Row;

/// The latest schema version for the micromegas_app database.
pub const LATEST_APP_SCHEMA_VERSION: i32 = 1;

/// Reads the current schema version from the database.
async fn read_schema_version(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> i32 {
    match sqlx::query(
        "SELECT version
         FROM migration;",
    )
    .fetch_one(&mut **tr)
    .await
    {
        Ok(row) => row.get("version"),
        Err(e) => {
            info!(
                "Error reading app schema version, assuming version 0: {}",
                e
            );
            0
        }
    }
}

/// Executes the database migration for micromegas_app.
pub async fn execute_migration(pool: sqlx::Pool<sqlx::Postgres>) -> Result<()> {
    let mut current_version = read_schema_version(&mut pool.begin().await?).await;
    if current_version == 0 {
        info!("creating v1 app schema");
        let mut tr = pool.begin().await?;
        create_tables(&mut tr)
            .await
            .with_context(|| "creating initial app schema")?;
        current_version = read_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    assert_eq!(current_version, LATEST_APP_SCHEMA_VERSION);
    info!("app schema version: {}", current_version);
    Ok(())
}
