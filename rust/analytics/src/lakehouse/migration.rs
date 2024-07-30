use anyhow::{Context, Result};
use micromegas_ingestion::remote_data_lake::acquire_lock;
use micromegas_tracing::prelude::*;
use sqlx::Executor;
use sqlx::Row;

pub const LATEST_LAKEHOUSE_SCHEMA_VERSION: i32 = 1;

async fn read_lakehouse_schema_version(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> i32 {
    match sqlx::query(
        "SELECT version
         FROM lakehouse_migration;",
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

pub async fn migrate_lakehouse(pool: sqlx::Pool<sqlx::Postgres>) -> Result<()> {
    let mut tr = pool.begin().await?;
    let mut current_version = read_lakehouse_schema_version(&mut tr).await;
    drop(tr);
    info!("current lakehouse schema: {}", current_version);
    if current_version != LATEST_LAKEHOUSE_SCHEMA_VERSION {
        let mut tr = pool.begin().await?;
        acquire_lock(&mut tr, 1).await?;
        current_version = read_lakehouse_schema_version(&mut pool.begin().await?).await;
        if LATEST_LAKEHOUSE_SCHEMA_VERSION == current_version {
            return Ok(());
        }
        if let Err(e) = execute_lakehouse_migration(pool.clone()).await {
            error!("Error migrating database: {}", e);
            return Err(e);
        }
        current_version = read_lakehouse_schema_version(&mut tr).await;
    }
    assert_eq!(current_version, LATEST_LAKEHOUSE_SCHEMA_VERSION);
    Ok(())
}

async fn execute_lakehouse_migration(pool: sqlx::Pool<sqlx::Postgres>) -> Result<()> {
    let mut current_version = read_lakehouse_schema_version(&mut pool.begin().await?).await;
    if 0 == current_version {
        info!("creating v1 lakehouse_schema");
        let mut tr = pool.begin().await?;
        create_tables(&mut tr).await?;
        current_version = read_lakehouse_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    assert_eq!(current_version, LATEST_LAKEHOUSE_SCHEMA_VERSION);
    Ok(())
}

async fn create_partitions_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    // Every table in the set shares the same schema - that schema can change over time, in which case the partition has to be rebuilt.
    // The instance id is a unique key to the table within the table set.
    //    * Example 1: if there is a process_log table for each process, the instance id could be the process_id.
    //      It would not clash with an instance of process_metrics table for the same process.
    //    * Example 2:  if there is a table instance for each metric, the view_instance_id could be the name of the metric.

    // source_data_hash can be used to detect that the partition is out of date (sha1 of the block_ids, for example)
    tr.execute("
         CREATE TABLE lakehouse_partitions(
                  view_set_name VARCHAR(255),
                  view_instance_id VARCHAR(255),
                  begin_insert_time TIMESTAMPTZ,
                  end_insert_time TIMESTAMPTZ,
                  min_event_time TIMESTAMPTZ,
                  max_event_time TIMESTAMPTZ,
                  updated TIMESTAMPTZ,
                  file_path VARCHAR(2047),
                  file_size BIGINT,
                  file_schema_hash bytea,
                  source_data_hash bytea
                  );
         CREATE INDEX lh_part_begin_insert on lakehouse_partitions(view_set_name, view_instance_id, begin_insert_time);
         CREATE INDEX lh_part_end_insert on lakehouse_partitions(view_set_name, view_instance_id, end_insert_time);
         CREATE INDEX lh_part_min_time on lakehouse_partitions(view_set_name, view_instance_id, min_event_time);
         CREATE INDEX lh_part_max_time on lakehouse_partitions(view_set_name, view_instance_id, max_event_time);
")
        .await
        .with_context(|| "Creating table blocks and its indices")?;
    Ok(())
}

async fn create_temp_files_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    // partitions that are out of date can still be referenced until they expire
    tr.execute(
        "
         CREATE TABLE temporary_files(
                  file_path VARCHAR(2047),
                  file_size BIGINT,
                  expiration TIMESTAMPTZ );
         CREATE INDEX temporary_files_expiration on temporary_files(expiration);
",
    )
    .await
    .with_context(|| "Creating temporary_files table")?;
    Ok(())
}

async fn create_migration_table(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    sqlx::query("CREATE table lakehouse_migration(version integer);")
        .execute(&mut **tr)
        .await
        .with_context(|| "Creating table lakehouse_migration")?;
    sqlx::query("INSERT INTO lakehouse_migration VALUES(1);")
        .execute(&mut **tr)
        .await
        .with_context(|| "Recording the initial lakehouse schema version")?;
    Ok(())
}

async fn create_tables(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    create_partitions_table(tr).await?;
    create_temp_files_table(tr).await?;
    create_migration_table(tr).await?;
    Ok(())
}
