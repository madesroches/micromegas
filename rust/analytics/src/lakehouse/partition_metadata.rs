use anyhow::{Context, Result};
use bytes::Bytes;
use micromegas_tracing::prelude::*;
use sqlx::{PgPool, Row};
use std::sync::Arc;

use crate::arrow_utils::parse_parquet_metadata;
use crate::lakehouse::metadata_compat;
use datafusion::parquet::file::metadata::ParquetMetaData;

/// Load partition metadata by file path from the dedicated metadata table
///
/// Uses legacy parser to handle both Arrow 56.0 and 57.0 formats during migration period.
/// The legacy parser will inject the required `num_rows` field from lakehouse_partitions
/// if it's missing in the metadata (Arrow 56.0 format).
#[span_fn]
pub async fn load_partition_metadata(
    pool: &PgPool,
    file_path: &str,
) -> Result<Arc<ParquetMetaData>> {
    // Query both metadata and num_rows from joined tables
    let row = sqlx::query(
        "SELECT pm.metadata, lp.num_rows
         FROM partition_metadata pm
         JOIN lakehouse_partitions lp ON pm.file_path = lp.file_path
         WHERE pm.file_path = $1",
    )
    .bind(file_path)
    .fetch_one(pool)
    .await
    .with_context(|| format!("loading metadata for file: {}", file_path))?;

    let metadata_bytes: Vec<u8> = row.try_get("metadata")?;
    let num_rows: i64 = row.try_get("num_rows")?;

    debug!(
        "loading metadata for {} with num_rows={}",
        file_path, num_rows
    );

    // Try standard Arrow 57.0 parser first (for new metadata)
    let metadata = match parse_parquet_metadata(&Bytes::from(metadata_bytes.clone())) {
        Ok(meta) => {
            debug!(
                "successfully loaded metadata using standard parser for {}",
                file_path
            );
            meta
        }
        Err(e) => {
            // Fall back to legacy parser (for Arrow 56.0 metadata)
            debug!("standard parser failed, trying legacy parser: {}", e);
            metadata_compat::parse_legacy_and_upgrade(&metadata_bytes, num_rows)
                .with_context(|| format!("parsing metadata for file: {}", file_path))?
        }
    };

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
