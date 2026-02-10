use crate::app_db::schema::{create_data_sources_table, create_tables};
use anyhow::{Context, Result};
use micromegas::tracing::prelude::*;
use sqlx::Row;

/// The latest schema version for the micromegas_app database.
pub const LATEST_APP_SCHEMA_VERSION: i32 = 2;

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

async fn update_schema_version(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    version: i32,
) -> Result<()> {
    sqlx::query("UPDATE migration SET version = $1;")
        .bind(version)
        .execute(&mut **tr)
        .await
        .with_context(|| format!("updating schema version to {version}"))?;
    Ok(())
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
    if current_version == 1 {
        info!("migrating app schema v1 -> v2: adding data_sources table");
        let mut tr = pool.begin().await?;
        create_data_sources_table(&mut tr)
            .await
            .with_context(|| "creating data_sources table")?;
        update_schema_version(&mut tr, 2).await?;
        current_version = read_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    assert_eq!(current_version, LATEST_APP_SCHEMA_VERSION);
    info!("app schema version: {current_version}");
    Ok(())
}
