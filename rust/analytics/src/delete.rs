use anyhow::{Context, Result};
use chrono::{DateTime, Days, Utc};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::{query, Row};
use uuid::Uuid;

use crate::lakehouse::write_partition::retire_expired_partitions;

// returns true if there is more data to delete
pub async fn delete_expired_blocks_batch(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<bool> {
    let batch_size: i32 = 1000;
    let rows = query(
        "SELECT process_id, stream_id, block_id
         FROM   blocks
         WHERE  blocks.insert_time <= $1
         LIMIT $2;",
    )
    .bind(expiration)
    .bind(batch_size)
    .fetch_all(&lake.db_pool)
    .await?;
    let mut paths = vec![];
    let mut block_ids = vec![];
    for r in rows {
        let process_id: Uuid = r.try_get("process_id")?;
        let stream_id: Uuid = r.try_get("stream_id")?;
        let block_id: Uuid = r.try_get("block_id")?;
        let path = format!("blobs/{process_id}/{stream_id}/{block_id}");
        paths.push(path);
        block_ids.push(block_id);
    }
    info!("deleting {:?}", &paths);
    lake.blob_storage.delete_batch(&paths).await?;
    query("DELETE FROM blocks where block_id = ANY($1);")
        .bind(block_ids)
        .execute(&lake.db_pool)
        .await?;
    Ok(paths.len() == batch_size as usize)
}

pub async fn delete_expired_blocks(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<()> {
    while delete_expired_blocks_batch(lake, expiration).await? {}
    Ok(())
}

pub async fn delete_empty_streams_batch(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<bool> {
    let batch_size: i32 = 1000;
    // delete returning would be more efficient
    let rows = query(
        "SELECT streams.stream_id
         FROM streams
         WHERE streams.insert_time <= $1
         AND NOT EXISTS
         (
           SELECT 1
           FROM blocks
           WHERE blocks.stream_id = streams.stream_id
           LIMIT 1
          )
         LIMIT $2
         ;",
    )
    .bind(expiration)
    .bind(batch_size)
    .fetch_all(&lake.db_pool)
    .await?;
    let mut stream_ids = vec![];
    for r in rows {
        let stream_id: Uuid = r.try_get("stream_id")?;
        stream_ids.push(stream_id);
    }

    info!("deleting expired streams {stream_ids:?}");
    query("DELETE FROM streams where stream_id = ANY($1);")
        .bind(&stream_ids)
        .execute(&lake.db_pool)
        .await?;

    Ok(stream_ids.len() == batch_size as usize)
}

pub async fn delete_empty_streams(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<()> {
    while delete_empty_streams_batch(lake, expiration).await? {}
    Ok(())
}

pub async fn delete_empty_processes_batch(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<bool> {
    let batch_size: i32 = 1000;
    // delete returning would be more efficient
    // also, we should remove the group by and use a query similar to delete_empty_streams_batch
    let rows = query(
        "SELECT processes.process_id
         FROM processes
         LEFT OUTER JOIN streams ON streams.process_id = processes.process_id
         WHERE processes.insert_time <= $1
         GROUP BY processes.process_id
         HAVING count(streams.stream_id) = 0
         LIMIT $2;",
    )
    .bind(expiration)
    .bind(batch_size)
    .fetch_all(&lake.db_pool)
    .await?;
    let mut process_ids = vec![];
    for r in rows {
        let process_id: Uuid = r.try_get("process_id")?;
        process_ids.push(process_id);
    }

    info!("deleting expired processes {process_ids:?}");
    query("DELETE FROM processes where process_id = ANY($1);")
        .bind(&process_ids)
        .execute(&lake.db_pool)
        .await?;

    Ok(process_ids.len() == batch_size as usize)
}

pub async fn delete_empty_processes(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<()> {
    while delete_empty_processes_batch(lake, expiration).await? {}
    Ok(())
}

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
