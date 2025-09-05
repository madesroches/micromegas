use anyhow::{Context, Result};
use chrono::Utc;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::Row;
use std::sync::Arc;

use super::partition_metadata::delete_partition_metadata_batch;

/// Deletes expired temporary files from the data lake.
pub async fn delete_expired_temporary_files(lake: Arc<DataLakeConnection>) -> Result<()> {
    let mut tr = lake.db_pool.begin().await?;
    let now = Utc::now();
    let rows = sqlx::query(
        "DELETE FROM temporary_files
         WHERE expiration < $1
         RETURNING file_path;",
    )
    .bind(now)
    .fetch_all(&mut *tr)
    .await
    .with_context(|| "listing expired temporary files")?;
    let mut to_delete = vec![];
    for r in rows {
        let file_path: String = r.try_get("file_path")?;
        info!("deleting expired file {file_path}");
        to_delete.push(file_path);
    }

    // Delete metadata for expired temporary files
    if !to_delete.is_empty() {
        delete_partition_metadata_batch(&mut tr, &to_delete)
            .await
            .with_context(|| "deleting partition metadata for expired temporary files")?;
    }

    lake.blob_storage.delete_batch(&to_delete).await?;
    tr.commit().await?;
    Ok(())
}
