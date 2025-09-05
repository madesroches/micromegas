use crate::arrow_utils::parse_parquet_metadata;
use anyhow::{Context, Result};
use micromegas_ingestion::remote_data_lake::acquire_lock;
use micromegas_tracing::prelude::*;
use sqlx::Executor;
use sqlx::Row;

pub const LATEST_LAKEHOUSE_SCHEMA_VERSION: i32 = 4;

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

/// Migrates the lakehouse schema to the latest version.
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

/// Executes the lakehouse migration steps.
async fn execute_lakehouse_migration(pool: sqlx::Pool<sqlx::Postgres>) -> Result<()> {
    let mut current_version = read_lakehouse_schema_version(&mut pool.begin().await?).await;
    if 0 == current_version {
        info!("creating v1 lakehouse_schema");
        let mut tr = pool.begin().await?;
        create_tables(&mut tr).await?;
        current_version = read_lakehouse_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 1 == current_version {
        info!("upgrade lakehouse schema to v2");
        let mut tr = pool.begin().await?;
        upgrade_v1_to_v2(&mut tr).await?;
        current_version = read_lakehouse_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 2 == current_version {
        info!("upgrade lakehouse schema to v3");
        let mut tr = pool.begin().await?;
        upgrade_v2_to_v3(&mut tr).await?;
        current_version = read_lakehouse_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 3 == current_version {
        info!("upgrade lakehouse schema to v4");
        let mut tr = pool.begin().await?;
        upgrade_v3_to_v4(&mut tr).await?;
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

async fn upgrade_v1_to_v2(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    // file_metadata is meant to be the serialized ParquetMetaData
    // which can be found in the footer of the file
    tr.execute("ALTER TABLE lakehouse_partitions ADD file_metadata bytea;")
        .await
        .with_context(|| "adding column file_metadata to lakehouse_partitions table")?;
    tr.execute("UPDATE lakehouse_migration SET version=2;")
        .await
        .with_context(|| "Updating lakehouse schema version to 2")?;
    Ok(())
}

async fn upgrade_v2_to_v3(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    // Add num_rows column to store row count separately from file_metadata
    tr.execute("ALTER TABLE lakehouse_partitions ADD num_rows BIGINT;")
        .await
        .with_context(|| "adding column num_rows to lakehouse_partitions table")?;

    // Add index on file_path for efficient on-demand metadata loading
    tr.execute("CREATE INDEX lakehouse_partitions_file_path ON lakehouse_partitions(file_path);")
        .await
        .with_context(|| "creating index on file_path")?;

    // Populate num_rows column for existing partitions
    populate_num_rows_column(tr)
        .await
        .with_context(|| "populating num_rows column")?;

    // Make num_rows NOT NULL after populating existing data
    tr.execute("ALTER TABLE lakehouse_partitions ALTER COLUMN num_rows SET NOT NULL;")
        .await
        .with_context(|| "setting num_rows column to NOT NULL")?;

    tr.execute("UPDATE lakehouse_migration SET version=3;")
        .await
        .with_context(|| "Updating lakehouse schema version to 3")?;
    Ok(())
}

async fn populate_num_rows_column(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    info!("populating num_rows column for existing partitions");

    let mut total_count = 0;
    let batch_size = 1000;

    loop {
        // Fetch partitions in batches to avoid loading all metadata into memory
        let rows = sqlx::query("SELECT file_path, file_metadata FROM lakehouse_partitions WHERE file_metadata IS NOT NULL AND num_rows IS NULL LIMIT $1")
            .bind(batch_size)
            .fetch_all(&mut **tr)
            .await?;

        if rows.is_empty() {
            break;
        }

        let mut batch_count = 0;
        for row in rows {
            let file_path: String = row.try_get("file_path")?;
            let file_metadata_buffer: Vec<u8> = row.try_get("file_metadata")?;

            // Parse metadata only for this partition
            match parse_parquet_metadata(&file_metadata_buffer.into()) {
                Ok(file_metadata) => {
                    let num_rows = file_metadata.file_metadata().num_rows();

                    // Update just this partition
                    if let Err(e) = sqlx::query(
                        "UPDATE lakehouse_partitions SET num_rows = $1 WHERE file_path = $2",
                    )
                    .bind(num_rows)
                    .bind(&file_path)
                    .execute(&mut **tr)
                    .await
                    {
                        error!(
                            "failed to update num_rows for partition {}: {}",
                            file_path, e
                        );
                        continue;
                    }

                    batch_count += 1;
                }
                Err(e) => {
                    error!(
                        "failed to parse metadata for partition {}: {}",
                        file_path, e
                    );
                    // For partitions with unparseable metadata, set num_rows to 0 as a fallback
                    if let Err(e2) = sqlx::query(
                        "UPDATE lakehouse_partitions SET num_rows = 0 WHERE file_path = $1",
                    )
                    .bind(&file_path)
                    .execute(&mut **tr)
                    .await
                    {
                        error!(
                            "failed to set fallback num_rows for partition {}: {}",
                            file_path, e2
                        );
                    }
                }
            }
        }

        total_count += batch_count;
        info!(
            "populated num_rows for {} partitions (total: {})",
            batch_count, total_count
        );
    }

    info!("populated num_rows for {} total partitions", total_count);
    Ok(())
}

async fn upgrade_v3_to_v4(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    // Create dedicated partition_metadata table
    tr.execute(
        "CREATE TABLE partition_metadata(
            file_path VARCHAR(2047) PRIMARY KEY,
            metadata bytea NOT NULL,
            insert_time TIMESTAMPTZ NOT NULL
        );",
    )
    .await
    .with_context(|| "creating partition_metadata table")?;

    // Migrate existing metadata from lakehouse_partitions to partition_metadata
    migrate_metadata_to_new_table(tr)
        .await
        .with_context(|| "migrating metadata to partition_metadata table")?;

    // Drop the file_metadata column from lakehouse_partitions after successful migration
    tr.execute("ALTER TABLE lakehouse_partitions DROP COLUMN file_metadata;")
        .await
        .with_context(|| "dropping file_metadata column from lakehouse_partitions")?;

    tr.execute("UPDATE lakehouse_migration SET version=4;")
        .await
        .with_context(|| "Updating lakehouse schema version to 4")?;
    Ok(())
}

async fn migrate_metadata_to_new_table(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    info!("migrating metadata to partition_metadata table");

    // First, get all file paths that have metadata (small data)
    let file_paths: Vec<String> = sqlx::query_scalar(
        "SELECT file_path 
         FROM lakehouse_partitions 
         WHERE file_metadata IS NOT NULL
         ORDER BY file_path",
    )
    .fetch_all(&mut **tr)
    .await?;

    let total_to_migrate = file_paths.len();
    info!(
        "found {} partitions with metadata to migrate",
        total_to_migrate
    );

    let mut total_count = 0;
    let batch_size = 10; // Small batch size since metadata can be large

    // Process in batches to avoid loading too much metadata at once
    for chunk in file_paths.chunks(batch_size) {
        // Build a query to fetch just this batch
        let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("${}", i)).collect();
        let query_str = format!(
            "SELECT file_path, file_metadata, updated 
             FROM lakehouse_partitions 
             WHERE file_path IN ({})",
            placeholders.join(", ")
        );

        let mut query = sqlx::query(&query_str);
        for path in chunk {
            query = query.bind(path);
        }

        let rows = query.fetch_all(&mut **tr).await?;

        for row in rows {
            let file_path: String = row.try_get("file_path")?;
            let file_metadata: Vec<u8> = row.try_get("file_metadata")?;
            let updated: chrono::DateTime<chrono::Utc> = row.try_get("updated")?;

            // Insert into new partition_metadata table (with ON CONFLICT for migration safety)
            if let Err(e) = sqlx::query(
                "INSERT INTO partition_metadata (file_path, metadata, insert_time) 
                 VALUES ($1, $2, $3)
                 ON CONFLICT (file_path) DO NOTHING",
            )
            .bind(&file_path)
            .bind(&file_metadata)
            .bind(updated)
            .execute(&mut **tr)
            .await
            {
                error!(
                    "failed to migrate metadata for partition {}: {}",
                    file_path, e
                );
                continue;
            }

            total_count += 1;
        }

        if total_count % 100 == 0 || total_count == total_to_migrate {
            info!(
                "migrated {}/{} partition metadata entries",
                total_count, total_to_migrate
            );
        }
    }

    info!("migrated metadata for {} total partitions", total_count);
    Ok(())
}
