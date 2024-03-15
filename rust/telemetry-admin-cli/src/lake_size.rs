use anyhow::Result;
use micromegas_telemetry::blob_storage::BlobStorage;
use sqlx::Row;
use std::sync::Arc;

pub async fn delete_old_blocks(
    connection: &mut sqlx::PgConnection,
    blob_storage: Arc<BlobStorage>,
    min_days_old: i32,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT blocks.block_id as block_id, payloads.block_id as payload_block_id
         FROM   processes, streams, blocks
         LEFT JOIN payloads ON blocks.block_id = payloads.block_id
         WHERE  streams.process_id = processes.process_id
         AND    blocks.stream_id = streams.stream_id
         AND    DATEDIFF(NOW(), processes.start_time) >= ?",
    )
    .bind(min_days_old)
    .fetch_all(&mut *connection)
    .await?;
    for r in rows {
        let process_id: String = r.try_get("process_id")?;
        let stream_id: String = r.try_get("stream_id")?;
        let block_id: String = r.try_get("block_id")?;
        println!("Deleting block {}", block_id);
        let path = format!("blobs/{process_id}/{stream_id}/{block_id}");
        blob_storage.delete(&path).await?;
        sqlx::query("DELETE FROM blocks WHERE block_id = ?;")
            .bind(block_id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}
