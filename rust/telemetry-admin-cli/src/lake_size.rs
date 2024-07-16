use anyhow::{Context, Result};
use chrono::{DateTime, Days, Utc};
use micromegas::chrono;
use micromegas::ingestion::data_lake_connection::DataLakeConnection;
use micromegas::sqlx::{query, Row};
use micromegas::tracing::prelude::*;
use micromegas::uuid::Uuid;

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
    // it would be more efficient to just run a delete statetement, but I want to log what gets deleted and why
    // in the future, we could also delete the associated caches
    let rows = query(
        "SELECT streams.stream_id
         FROM streams
         LEFT OUTER JOIN blocks ON blocks.stream_id = streams.stream_id
         WHERE streams.insert_time <= $1
         GROUP BY streams.stream_id
         HAVING count(blocks.block_id) = 0
         LIMIT $2;",
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
    Ok(())
}
