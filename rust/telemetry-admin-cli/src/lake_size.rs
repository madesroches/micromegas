use anyhow::Result;
use micromegas::chrono::Days;
use micromegas::chrono::Utc;
use micromegas::ingestion::data_lake_connection::DataLakeConnection;
use micromegas::sqlx::{query, Row};
use micromegas::tracing::prelude::*;
use micromegas::uuid::Uuid;

// returns true if there is more data to delete
pub async fn delete_old_blocks_batch(lake: &DataLakeConnection, min_days_old: i32) -> Result<bool> {
    let now = Utc::now();
    let expiration = now.checked_sub_days(Days::new(min_days_old as u64));
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

pub async fn delete_old_blocks(lake: &DataLakeConnection, min_days_old: i32) -> Result<()> {
    while delete_old_blocks_batch(lake, min_days_old).await? {}
    Ok(())
}
