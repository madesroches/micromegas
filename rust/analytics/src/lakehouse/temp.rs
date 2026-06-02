use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::Row;
use std::sync::Arc;

use super::partition_metadata::delete_partition_metadata_batch;

async fn delete_expired_temporary_files_batch(
    lake: &DataLakeConnection,
    now: DateTime<Utc>,
) -> Result<bool> {
    let batch_size: i32 = 1000;
    let mut tr = lake.db_pool.begin().await?;
    let rows = sqlx::query(
        "DELETE FROM temporary_files
         WHERE file_path IN (
             SELECT file_path FROM temporary_files
             WHERE expiration < $1
             LIMIT $2
         )
         RETURNING file_path;",
    )
    .bind(now)
    .bind(batch_size)
    .fetch_all(&mut *tr)
    .await
    .with_context(|| "deleting expired temporary files batch")?;

    if rows.is_empty() {
        return Ok(false);
    }

    let to_delete: Vec<String> = rows
        .iter()
        .map(|r| r.try_get("file_path"))
        .collect::<Result<_, _>>()?;

    for file_path in &to_delete {
        debug!("deleting expired temporary file {file_path}");
    }

    delete_partition_metadata_batch(&mut tr, &to_delete)
        .await
        .with_context(|| "deleting partition metadata for expired temporary files")?;

    lake.blob_storage.delete_batch(&to_delete).await?;
    tr.commit().await?;
    info!("deleted {} expired temporary files", to_delete.len());
    Ok(true)
}

pub async fn delete_expired_temporary_files(lake: Arc<DataLakeConnection>) -> Result<()> {
    let now = Utc::now();
    while delete_expired_temporary_files_batch(&lake, now).await? {}
    Ok(())
}
