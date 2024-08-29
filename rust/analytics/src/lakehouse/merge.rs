use super::{
    partition::{write_partition_from_rows, PartitionRowSet},
    partition_source_data::hash_to_object_count,
    view::View,
};
use crate::response_writer::ResponseWriter;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use datafusion::parquet::arrow::{
    async_reader::ParquetObjectReader, ParquetRecordBatchStreamBuilder,
};
use futures::stream::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use object_store::{path::Path, ObjectMeta};
use sqlx::Row;
use std::sync::Arc;
use xxhash_rust::xxh32::xxh32;

pub async fn create_merged_partition(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    response_writer: Arc<ResponseWriter>,
) -> Result<()> {
    let view_set_name = view.get_view_set_name().to_string();
    let view_instance_id = view.get_view_instance_id().to_string();
    let desc = format!(
        "[{}, {}] {view_set_name} {view_instance_id}",
        begin.to_rfc3339(),
        end.to_rfc3339()
    );
    // we are not looking for intersecting partitions, but only those that fit completely in the range
    let rows = sqlx::query(
        "SELECT file_path, file_size, updated, file_schema_hash, source_data_hash, begin_insert_time, end_insert_time, min_event_time, max_event_time
         FROM lakehouse_partitions
         WHERE view_set_name = $1
         AND view_instance_id = $2
         AND begin_insert_time >= $3
         AND end_insert_time <= $4
         ;",
    )
    .bind(&view_set_name)
    .bind(&view_instance_id)
    .bind(begin)
    .bind(end)
    .fetch_all(&lake.db_pool)
    .await
    .with_context(|| "fetching partitions to merge")?;
    if rows.len() < 2 {
        response_writer
            .write_string(&format!("{desc}: not enough partitions to merge"))
            .await?;
        return Ok(());
    }
    let latest_file_schema_hash = view.get_file_schema_hash();
    let mut sum_size: i64 = 0;
    let mut source_hash: i64 = 0;
    for r in &rows {
        // for some time all the hashes will actually be the number of events in the source data
        // when views have different hash algos, we should delegate to the view the creation of the merged hash
        let source_data_hash: Vec<u8> = r.try_get("source_data_hash")?;
        source_hash = if source_data_hash.len() == std::mem::size_of::<i64>() {
            source_hash + hash_to_object_count(&source_data_hash)?
        } else {
            //previous hash algo
            xxh32(&source_data_hash, source_hash as u32).into()
        };

        let file_size: i64 = r.try_get("file_size")?;
        sum_size += file_size;

        let file_schema_hash: Vec<u8> = r.try_get("file_schema_hash")?;
        if file_schema_hash != latest_file_schema_hash {
            let begin_insert_time: DateTime<Utc> = r.try_get("begin_insert_time")?;
            let end_insert_time: DateTime<Utc> = r.try_get("end_insert_time")?;
            response_writer
                .write_string(&format!(
                    "{desc}: incompatible file schema with [{},{}]",
                    begin_insert_time.to_rfc3339(),
                    end_insert_time.to_rfc3339()
                ))
                .await?;
            return Ok(());
        }
    }
    response_writer
        .write_string(&format!(
            "{desc}: merging {} partitions sum_size={sum_size}",
            rows.len()
        ))
        .await?;

    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let join_handle = tokio::spawn(write_partition_from_rows(
        lake.clone(),
        view.get_meta(),
        begin,
        end,
        source_hash.to_le_bytes().to_vec(),
        rx,
        sum_size as usize,
        response_writer.clone(),
    ));
    for r in &rows {
        let file_path: String = r.try_get("file_path")?;
        let file_size: i64 = r.try_get("file_size")?;
        response_writer
            .write_string(&format!("reading path={file_path} size={file_size}"))
            .await?;
        let min_time_row: DateTime<Utc> = r.try_get("min_event_time")?;
        let max_time_row: DateTime<Utc> = r.try_get("max_event_time")?;

        let updated: DateTime<Utc> = r.try_get("updated")?;
        let meta = ObjectMeta {
            location: Path::from(file_path),
            last_modified: updated,
            size: file_size as usize,
            e_tag: None,
            version: None,
        };
        let reader = ParquetObjectReader::new(lake.blob_storage.inner(), meta);
        let builder = ParquetRecordBatchStreamBuilder::new(reader)
            .await
            .with_context(|| "ParquetRecordBatchStreamBuilder::new")?;
        let mut rbstream = builder
            .with_batch_size(1024 * 1024) // the default is 1024, which seems low
            .build()
            .with_context(|| "builder.build()")?;
        while let Some(rb_res) = rbstream.next().await {
            let rows = rb_res?;
            tx.send(PartitionRowSet {
                min_time_row,
                max_time_row,
                rows,
            })
            .await?;
        }
    }
    drop(tx);
    join_handle.await??;

    Ok(())
}
