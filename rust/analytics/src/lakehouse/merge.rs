use super::view::View;
use anyhow::{Context, Result};
use bytes::BufMut;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::parquet::{
    arrow::{async_reader::ParquetObjectReader, ArrowWriter, ParquetRecordBatchStreamBuilder},
    basic::Compression,
    file::properties::{WriterProperties, WriterVersion},
};
use futures::stream::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use object_store::{path::Path, ObjectMeta};
use sqlx::Row;
use std::sync::Arc;

async fn create_merged_partition(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<()> {
    let table_set_name = &*(view.get_table_set_name());
    let table_instance_id = &*(view.get_table_instance_id());
    // we are not looking for intersecting partitions, but only those that fit completely in the range
    let rows = sqlx::query(
        "SELECT file_path, file_size, updated, file_schema_hash, begin_insert_time, end_insert_time
         FROM lakehouse_partitions
         WHERE table_set_name = $1
         AND table_instance_id = $2
         AND begin_insert_time >= $3
         AND end_insert_time <= $4
         ;",
    )
    .bind(table_set_name)
    .bind(table_instance_id)
    .bind(begin)
    .bind(end)
    .fetch_all(&lake.db_pool)
    .await
    .with_context(|| "fetching partitions to merge")?;
    if rows.len() < 2 {
        info!("not enough partitions to merge");
        return Ok(());
    }
    let latest_file_schema_hash = view.get_file_schema_hash();
    let mut sum_size: i64 = 0;
    for r in &rows {
        let file_schema_hash: Vec<u8> = r.try_get("file_schema_hash")?;
        let file_size: i64 = r.try_get("file_size")?;
        sum_size += file_size;
        if file_schema_hash != latest_file_schema_hash {
            let begin_insert_time: DateTime<Utc> = r.try_get("begin_insert_time")?;
            let end_insert_time: DateTime<Utc> = r.try_get("end_insert_time")?;
            warn!("can't merge partition table_set_name={table_set_name} table_instance_id={table_instance_id} begin_insert_time={begin_insert_time} end_insert_time={end_insert_time}");
            return Ok(());
        }
    }
    info!(
        "merging {} partitions sum_size={sum_size} bytes",
        rows.len()
    );

    // buffer the whole parquet in memory until https://github.com/apache/arrow-rs/issues/5766 is done
    // Impl AsyncFileWriter by object_store #5766
    let mut buffer_writer = bytes::BytesMut::with_capacity(sum_size as usize).writer();
    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::LZ4_RAW)
        .build();
    let mut arrow_writer =
        ArrowWriter::try_new(&mut buffer_writer, view.get_file_schema(), Some(props))?;

    for r in &rows {
        let file_path: String = r.try_get("file_path")?;
        let file_size: i64 = r.try_get("file_size")?;
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
            // .with_batch_size(1024 * 1024) the default is 1024, which seems low
            .build()
            .with_context(|| "builder.build()")?;
        while let Some(rb_res) = rbstream.next().await {
            let record_batch = rb_res?;
            arrow_writer
                .write(&record_batch)
                .with_context(|| "arrow_writer.write")?;
        }
    }
    arrow_writer.close().with_context(|| "arrow_writer.close")?;

    Ok(())
}

pub async fn merge_partitions(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    begin_range: DateTime<Utc>,
    end_range: DateTime<Utc>,
    partition_time_delta: TimeDelta,
) -> Result<()> {
    let mut begin_part = begin_range;
    let mut end_part = begin_part + partition_time_delta;
    while end_part <= end_range {
        create_merged_partition(lake.clone(), view.clone(), begin_part, end_part)
            .await
            .with_context(|| "create_merged_partition")?;
        begin_part = end_part;
        end_part = begin_part + partition_time_delta;
    }
    Ok(())
}
