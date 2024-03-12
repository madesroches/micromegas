use crate::sql_telemetry_db::create_tables;
use anyhow::{Context, Result};
use sqlx::Executor;
use sqlx::Row;
use tracing::prelude::*;

pub const LATEST_SCHEMA_VERSION: i32 = 2;

pub async fn read_schema_version(tr: &mut sqlx::Transaction<'_, sqlx::Any>) -> i32 {
    match sqlx::query(
        "SELECT version
         FROM migration;",
    )
    .fetch_one(tr)
    .await
    {
        Ok(row) => row.get("version"),
        Err(e) => {
            info!("Error reading schema version, assuming version 0: {}", e);
            0
        }
    }
}

pub async fn upgrade_schema_v2(connection: &mut sqlx::AnyConnection) -> Result<()> {
    connection
        .execute("ALTER TABLE blocks ADD payload_size BIGINT;")
        .await
        .with_context(|| "Adding column payload_size to table blocks")?;
    connection
        .execute("UPDATE migration SET version=2;")
        .await
        .with_context(|| "Updating schema version to 2")?;
    Ok(())
}

pub async fn execute_migration(pool: sqlx::Pool<sqlx::Any>) -> Result<()> {
    let mut current_version = read_schema_version(&mut pool.begin().await?).await;
    if 0 == current_version {
        info!("creating v1 schema");
        let mut tr = pool.begin().await?;
        create_tables(&mut tr).await?;
        current_version = read_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 1 == current_version {
        info!("upgrading schema to v2");
        let mut tr = pool.begin().await?;
        upgrade_schema_v2(&mut tr).await?;
        current_version = read_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    assert_eq!(current_version, LATEST_SCHEMA_VERSION);
    Ok(())
}
