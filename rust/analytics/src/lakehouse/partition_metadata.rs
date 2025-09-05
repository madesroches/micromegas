use anyhow::{Context, Result};
use bytes::Bytes;
use micromegas_tracing::prelude::*;
use sqlx::{PgPool, Row};
use std::sync::Arc;

use crate::arrow_utils::parse_parquet_metadata;
use datafusion::parquet::file::metadata::ParquetMetaData;

/// Load partition metadata by file path from the dedicated metadata table
#[span_fn]
pub async fn load_partition_metadata(
    pool: &PgPool,
    file_path: &str,
) -> Result<Arc<ParquetMetaData>> {
    let row = sqlx::query("SELECT metadata FROM partition_metadata WHERE file_path = $1")
        .bind(file_path)
        .fetch_one(pool)
        .await
        .with_context(|| format!("loading metadata for file: {}", file_path))?;

    let metadata_bytes: Vec<u8> = row.try_get("metadata")?;
    let metadata = parse_parquet_metadata(&Bytes::from(metadata_bytes))
        .with_context(|| format!("parsing metadata for file: {}", file_path))?;
    Ok(Arc::new(metadata))
}

/// Delete multiple partition metadata entries in a single transaction
/// Uses PostgreSQL's ANY() with array to avoid placeholder limits
#[span_fn]
pub async fn delete_partition_metadata_batch(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    file_paths: &[String],
) -> Result<()> {
    if file_paths.is_empty() {
        return Ok(());
    }

    // Use PostgreSQL's ANY() with array - no placeholder limits
    let result = sqlx::query("DELETE FROM partition_metadata WHERE file_path = ANY($1)")
        .bind(file_paths)
        .execute(&mut **tr)
        .await
        .with_context(|| format!("deleting {} metadata entries", file_paths.len()))?;

    debug!("deleted {} metadata entries", result.rows_affected());
    Ok(())
}
