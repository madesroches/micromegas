use anyhow::{Context, Result};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use micromegas_tracing::prelude::*;
use sqlx::{PgPool, Row};
use std::sync::Arc;

use crate::arrow_utils::{parse_parquet_metadata, serialize_parquet_metadata};
use datafusion::parquet::file::metadata::ParquetMetaData;

/// Represents metadata stored in the partition_metadata table
#[derive(Debug, Clone)]
pub struct StoredPartitionMetadata {
    pub file_path: String,
    pub metadata: Arc<ParquetMetaData>,
    pub insert_time: DateTime<Utc>,
}

/// Load partition metadata by file path from the dedicated metadata table
#[span_fn]
pub async fn load_partition_metadata(
    pool: &PgPool,
    file_path: &str,
) -> Result<Option<Arc<ParquetMetaData>>> {
    let row = sqlx::query("SELECT metadata FROM partition_metadata WHERE file_path = $1")
        .bind(file_path)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("loading metadata for file: {}", file_path))?;

    match row {
        Some(row) => {
            let metadata_bytes: Vec<u8> = row.try_get("metadata")?;
            let metadata = parse_parquet_metadata(&Bytes::from(metadata_bytes))
                .with_context(|| format!("parsing metadata for file: {}", file_path))?;
            Ok(Some(Arc::new(metadata)))
        }
        None => Ok(None),
    }
}

/// Insert partition metadata in the dedicated table
/// Note: Partitions are immutable, so this only inserts, never updates
#[span_fn]
pub async fn insert_partition_metadata(
    pool: &PgPool,
    file_path: &str,
    metadata: &ParquetMetaData,
) -> Result<()> {
    let metadata_bytes =
        serialize_parquet_metadata(metadata).with_context(|| "serializing parquet metadata")?;
    let insert_time = sqlx::types::chrono::Utc::now();

    sqlx::query(
        "INSERT INTO partition_metadata (file_path, metadata, insert_time) 
         VALUES ($1, $2, $3)",
    )
    .bind(file_path)
    .bind(metadata_bytes.as_ref())
    .bind(insert_time)
    .execute(pool)
    .await
    .with_context(|| format!("inserting metadata for file: {}", file_path))?;

    Ok(())
}

/// Delete partition metadata when a partition is removed
#[span_fn]
pub async fn delete_partition_metadata(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    file_path: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM partition_metadata WHERE file_path = $1")
        .bind(file_path)
        .execute(&mut **tr)
        .await
        .with_context(|| format!("deleting metadata for file: {}", file_path))?;

    Ok(())
}

/// Delete multiple partition metadata entries in a single transaction
#[span_fn]
pub async fn delete_partition_metadata_batch(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    file_paths: &[String],
) -> Result<()> {
    if file_paths.is_empty() {
        return Ok(());
    }

    // Build the query with placeholders
    let placeholders: Vec<String> = (1..=file_paths.len()).map(|i| format!("${}", i)).collect();
    let query = format!(
        "DELETE FROM partition_metadata WHERE file_path IN ({})",
        placeholders.join(", ")
    );

    let mut query = sqlx::query(&query);
    for path in file_paths {
        query = query.bind(path);
    }

    query
        .execute(&mut **tr)
        .await
        .with_context(|| format!("deleting {} metadata entries", file_paths.len()))?;

    Ok(())
}

/// Check if metadata exists for a given file path
#[span_fn]
pub async fn metadata_exists(pool: &PgPool, file_path: &str) -> Result<bool> {
    let row = sqlx::query("SELECT 1 FROM partition_metadata WHERE file_path = $1")
        .bind(file_path)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("checking metadata existence for file: {}", file_path))?;

    Ok(row.is_some())
}

/// Get metadata insert time for a given file path
#[span_fn]
pub async fn get_metadata_insert_time(
    pool: &PgPool,
    file_path: &str,
) -> Result<Option<DateTime<Utc>>> {
    let row = sqlx::query("SELECT insert_time FROM partition_metadata WHERE file_path = $1")
        .bind(file_path)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("getting metadata insert time for file: {}", file_path))?;

    match row {
        Some(row) => {
            let insert_time: DateTime<Utc> = row.try_get("insert_time")?;
            Ok(Some(insert_time))
        }
        None => Ok(None),
    }
}
