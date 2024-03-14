use anyhow::Result;
use lgn_blob_storage::BlobStorage;
use sqlx::Row;
use std::sync::Arc;

pub async fn delete_old_blocks(
    connection: &mut sqlx::PgConnection,
    blob_storage: Arc<dyn BlobStorage>,
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
        let block_id: String = r.try_get("block_id")?;
        let payload_block_id: Option<String> = r.try_get("payload_block_id")?;
        println!("Deleting block {}", block_id);
        if let Some(_id) = payload_block_id {
            sqlx::query("DELETE FROM payloads WHERE block_id = ?;")
                .bind(&block_id)
                .execute(&mut *connection)
                .await?;
        } else {
            blob_storage.delete_blob(&block_id).await?;
        }
        sqlx::query("DELETE FROM blocks WHERE block_id = ?;")
            .bind(block_id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}
