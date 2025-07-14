use crate::sql_telemetry_db::create_tables;
use anyhow::{Context, Result};
use micromegas_tracing::prelude::*;
use sqlx::Executor;
use sqlx::Row;

/// The latest schema version for the data lake.
pub const LATEST_DATA_LAKE_SCHEMA_VERSION: i32 = 2;

/// Reads the current schema version from the database.
pub async fn read_data_lake_schema_version(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> i32 {
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
                "Error reading data lake schema version, assuming version 0: {}",
                e
            );
            0
        }
    }
}

/// Upgrades the data lake schema to version 2.
pub async fn upgrade_data_lake_schema_v2(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    tr.execute("ALTER TABLE blocks ADD insert_time TIMESTAMPTZ;")
        .await
        .with_context(|| "adding column insert_time to blocks table")?;
    tr.execute("UPDATE blocks SET insert_time=end_time WHERE insert_time is NULL;")
        .await
        .with_context(|| "use end_time as insert_time to backfill missing data")?;
    tr.execute("CREATE INDEX block_begin_time on blocks(begin_time);")
        .await
        .with_context(|| "adding index block_begin_time")?;
    tr.execute("CREATE INDEX block_end_time on blocks(end_time);")
        .await
        .with_context(|| "adding index block_end_time")?;
    tr.execute("CREATE INDEX block_insert_time on blocks(insert_time);")
        .await
        .with_context(|| "adding index block_insert_time")?;
    tr.execute("CREATE INDEX process_insert_time on processes(insert_time);")
        .await
        .with_context(|| "adding index process_insert_time")?;
    tr.execute("UPDATE migration SET version=2;")
        .await
        .with_context(|| "Updating data lake schema version to 2")?;
    Ok(())
}

/// Executes the database migration.
pub async fn execute_migration(pool: sqlx::Pool<sqlx::Postgres>) -> Result<()> {
    let mut current_version = read_data_lake_schema_version(&mut pool.begin().await?).await;
    if 0 == current_version {
        info!("creating v1 data_lake_schema");
        let mut tr = pool.begin().await?;
        create_tables(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 1 == current_version {
        info!("upgrading data_lake_schema to v2");
        let mut tr = pool.begin().await?;
        upgrade_data_lake_schema_v2(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    assert_eq!(current_version, LATEST_DATA_LAKE_SCHEMA_VERSION);
    Ok(())
}
