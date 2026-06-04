use anyhow::{Context, Result};
use chrono::{DateTime, Days, Utc};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::{Row, query};
use uuid::Uuid;

use crate::lakehouse::write_partition::retire_expired_partitions;

/// Deletes a batch of expired blocks from the data lake.
/// Returns `true` if there are more blocks to delete, `false` otherwise.
#[span_fn]
pub async fn delete_expired_blocks_batch(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<bool> {
    let batch_size: i32 = 1000;
    let mut transaction = lake.db_pool.begin().await?;
    let rows = query(
        "DELETE FROM blocks
         WHERE block_id IN (
             SELECT block_id FROM blocks WHERE insert_time <= $1 LIMIT $2
         )
         RETURNING process_id, stream_id, block_id;",
    )
    .bind(expiration)
    .bind(batch_size)
    .fetch_all(&mut *transaction)
    .await?;
    if rows.is_empty() {
        return Ok(false);
    }
    let mut paths = vec![];
    for r in &rows {
        let process_id: Uuid = r.try_get("process_id")?;
        let stream_id: Uuid = r.try_get("stream_id")?;
        let block_id: Uuid = r.try_get("block_id")?;
        let path = format!("blobs/{process_id}/{stream_id}/{block_id}");
        paths.push(path);
    }
    info!("deleting {} blocks", paths.len());
    lake.blob_storage.delete_batch(&paths).await?;
    transaction.commit().await.with_context(|| "commit")?;
    Ok(paths.len() == batch_size as usize)
}

/// Deletes all expired blocks from the data lake.
#[span_fn]
pub async fn delete_expired_blocks(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<()> {
    while delete_expired_blocks_batch(lake, expiration).await? {}
    Ok(())
}

/// Deletes a batch of empty streams from the data lake.
/// Returns `true` if there are more streams to delete, `false` otherwise.
#[span_fn]
pub async fn delete_empty_streams_batch(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<bool> {
    let batch_size: i32 = 1000;
    let rows = query(
        "WITH batch AS (
             SELECT stream_id FROM streams
             WHERE  insert_time <= $1
             AND    NOT EXISTS (SELECT 1 FROM blocks WHERE blocks.stream_id = streams.stream_id LIMIT 1)
             LIMIT  $2
         )
         DELETE FROM streams
         WHERE stream_id IN (SELECT stream_id FROM batch)
         RETURNING stream_id;",
    )
    .bind(expiration)
    .bind(batch_size)
    .fetch_all(&lake.db_pool)
    .await?;
    let count = rows.len();
    info!("deleted {count} empty streams");
    Ok(count == batch_size as usize)
}

/// Deletes all empty streams from the data lake.
#[span_fn]
pub async fn delete_empty_streams(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<()> {
    while delete_empty_streams_batch(lake, expiration).await? {}
    Ok(())
}

/// Deletes a batch of empty processes from the data lake.
/// Returns `true` if there are more processes to delete, `false` otherwise.
#[span_fn]
pub async fn delete_empty_processes_batch(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<bool> {
    let batch_size: i32 = 1000;
    let rows = query(
        "WITH batch AS (
             SELECT process_id FROM processes
             WHERE  insert_time <= $1
             AND    NOT EXISTS (SELECT 1 FROM streams WHERE streams.process_id = processes.process_id LIMIT 1)
             LIMIT  $2
         )
         DELETE FROM processes
         WHERE process_id IN (SELECT process_id FROM batch)
         RETURNING process_id;",
    )
    .bind(expiration)
    .bind(batch_size)
    .fetch_all(&lake.db_pool)
    .await?;
    let count = rows.len();
    info!("deleted {count} empty processes");
    Ok(count == batch_size as usize)
}

/// Deletes all empty processes from the data lake.
#[span_fn]
pub async fn delete_empty_processes(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<()> {
    while delete_empty_processes_batch(lake, expiration).await? {}
    Ok(())
}

/// Deletes all data older than a specified number of days from the data lake.
#[span_fn]
pub async fn delete_old_data(lake: &DataLakeConnection, min_days_old: i32) -> Result<()> {
    let now = Utc::now();
    let expiration = now
        .checked_sub_days(Days::new(min_days_old as u64))
        .with_context(|| "computing expiration date/time")?;
    delete_expired_blocks(lake, expiration)
        .await
        .with_context(|| "delete_expired_blocks")?;
    delete_empty_streams(lake, expiration)
        .await
        .with_context(|| "delete_empty_streams")?;
    delete_empty_processes(lake, expiration)
        .await
        .with_context(|| "delete_empty_processes")?;
    retire_expired_partitions(lake, expiration)
        .await
        .with_context(|| "retire_expired_partitions")?;
    Ok(())
}
