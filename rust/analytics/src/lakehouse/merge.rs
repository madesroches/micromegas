use super::view::View;
use crate::{
    lakehouse::partition::{write_partition, Partition},
    response_writer::ResponseWriter,
};
use anyhow::{Context, Result};
use bytes::BufMut;
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use datafusion::parquet::{
    arrow::{async_reader::ParquetObjectReader, ArrowWriter, ParquetRecordBatchStreamBuilder},
    basic::Compression,
    file::properties::{WriterProperties, WriterVersion},
};
use futures::stream::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use object_store::{path::Path, ObjectMeta};
use sqlx::Row;
use std::sync::Arc;
use xxhash_rust::xxh32::xxh32;

async fn create_merged_partition(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    writer: Arc<ResponseWriter>,
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
        writer
            .write_string(&format!("{desc}: not enough partitions to merge"))
            .await?;
        return Ok(());
    }
    let latest_file_schema_hash = view.get_file_schema_hash();
    let mut sum_size: i64 = 0;
    let mut min_event_time: DateTime<Utc> = rows[0].try_get("min_event_time")?;
    let mut max_event_time: DateTime<Utc> = rows[0].try_get("max_event_time")?;
    let mut source_hash = 0;
    for r in &rows {
        let source_data_hash: Vec<u8> = r.try_get("source_data_hash")?;
        source_hash = xxh32(&source_data_hash, source_hash);

        let file_size: i64 = r.try_get("file_size")?;
        sum_size += file_size;

        let begin_event_time: DateTime<Utc> = r.try_get("min_event_time")?;
        let end_event_time: DateTime<Utc> = r.try_get("max_event_time")?;
        min_event_time = min_event_time.min(begin_event_time);
        max_event_time = max_event_time.max(end_event_time);

        let file_schema_hash: Vec<u8> = r.try_get("file_schema_hash")?;
        if file_schema_hash != latest_file_schema_hash {
            let begin_insert_time: DateTime<Utc> = r.try_get("begin_insert_time")?;
            let end_insert_time: DateTime<Utc> = r.try_get("end_insert_time")?;
            writer
                .write_string(&format!(
                    "{desc}: incompatible file schema with [{},{}]",
                    begin_insert_time.to_rfc3339(),
                    end_insert_time.to_rfc3339()
                ))
                .await?;
            return Ok(());
        }
    }
    writer
        .write_string(&format!(
            "{desc}: merging {} partitions sum_size={sum_size}",
            rows.len()
        ))
        .await?;

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

    let file_id = uuid::Uuid::new_v4();
    let file_path = format!(
        "views/{}/{}/{}/{}_{file_id}.parquet",
        &view_set_name,
        &view_instance_id,
        begin.format("%Y-%m-%d"),
        begin.format("%H-%M-%S")
    );

    let buffer: bytes::Bytes = buffer_writer.into_inner().into();
    write_partition(
        &lake,
        &Partition {
            view_set_name,
            view_instance_id,
            begin_insert_time: begin,
            end_insert_time: end,
            min_event_time,
            max_event_time,
            updated: sqlx::types::chrono::Utc::now(),
            file_path,
            file_size: buffer.len() as i64,
            file_schema_hash: latest_file_schema_hash,
            source_data_hash: source_hash.to_le_bytes().to_vec(),
        },
        buffer,
    )
    .await?;

    Ok(())
}

pub async fn merge_partitions(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    begin_range: DateTime<Utc>,
    end_range: DateTime<Utc>,
    partition_time_delta: TimeDelta,
    writer: Arc<ResponseWriter>,
) -> Result<()> {
    let mut begin_part = begin_range;
    let mut end_part = begin_part + partition_time_delta;
    while end_part <= end_range {
        create_merged_partition(
            lake.clone(),
            view.clone(),
            begin_part,
            end_part,
            writer.clone(),
        )
        .await
        .with_context(|| "create_merged_partition")?;
        begin_part = end_part;
        end_part = begin_part + partition_time_delta;
    }
    Ok(())
}

pub async fn merge_recent_partitions(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    partition_time_delta: TimeDelta,
    nb_partitions: i32,
    min_age: TimeDelta,
    writer: Arc<ResponseWriter>,
) -> Result<()> {
    let now = Utc::now();
    let truncated = now.duration_trunc(partition_time_delta)?;
    let start = truncated - partition_time_delta * nb_partitions;
    for index in 0..nb_partitions {
        let start_partition = start + partition_time_delta * index;
        let end_partition = start + partition_time_delta * (index + 1);
        if (now - end_partition) < min_age {
            return Ok(());
        }
        create_merged_partition(
            lake.clone(),
            view.clone(),
            start_partition,
            end_partition,
            writer.clone(),
        )
        .await
        .with_context(|| "create_or_update_partition")?;
    }
    Ok(())
}
