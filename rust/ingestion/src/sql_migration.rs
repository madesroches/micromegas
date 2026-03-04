use crate::sql_telemetry_db::create_tables;
use anyhow::{Context, Result};
use micromegas_tracing::prelude::*;
use sqlx::Executor;
use sqlx::Row;

/// The latest schema version for the data lake.
pub const LATEST_DATA_LAKE_SCHEMA_VERSION: i32 = 3;

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

/// Upgrades the data lake schema to version 3.
/// Drops old non-unique indexes (superseded by the unique indexes created before this transaction).
pub async fn upgrade_data_lake_schema_v3(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    tr.execute("DROP INDEX IF EXISTS process_id;")
        .await
        .with_context(|| "dropping old non-unique index process_id")?;
    tr.execute("DROP INDEX IF EXISTS stream_id;")
        .await
        .with_context(|| "dropping old non-unique index stream_id")?;
    tr.execute("DROP INDEX IF EXISTS block_id;")
        .await
        .with_context(|| "dropping old non-unique index block_id")?;
    tr.execute("UPDATE migration SET version=3;")
        .await
        .with_context(|| "updating data lake schema version to 3")?;
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
    if 2 == current_version {
        info!("upgrading data_lake_schema to v3");
        // CREATE UNIQUE INDEX CONCURRENTLY cannot run inside a transaction.
        // Run these outside any transaction, then do the rest in a transaction.
        // IF NOT EXISTS makes this idempotent and safe for retries.
        sqlx::query("CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS processes_process_id_unique ON processes(process_id);")
            .execute(&pool)
            .await
            .with_context(|| "creating unique index on processes(process_id)")?;
        sqlx::query("CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS streams_stream_id_unique ON streams(stream_id);")
            .execute(&pool)
            .await
            .with_context(|| "creating unique index on streams(stream_id)")?;
        sqlx::query("CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS blocks_block_id_unique ON blocks(block_id);")
            .execute(&pool)
            .await
            .with_context(|| "creating unique index on blocks(block_id)")?;

        let mut tr = pool.begin().await?;
        upgrade_data_lake_schema_v3(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    assert_eq!(current_version, LATEST_DATA_LAKE_SCHEMA_VERSION);
    Ok(())
}
