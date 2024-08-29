use super::view::ViewMetadata;
use crate::response_writer::ResponseWriter;
use anyhow::{Context, Result};
use bytes::BufMut;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::array::RecordBatch,
    parquet::{
        arrow::ArrowWriter,
        basic::Compression,
        file::properties::{WriterProperties, WriterVersion},
    },
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use sqlx::Row;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;

/// RecordBatch + time range associated with the events
pub struct PartitionRowSet {
    pub min_time_row: DateTime<Utc>,
    pub max_time_row: DateTime<Utc>,
    pub rows: RecordBatch,
}

/// Partition to be written
pub struct Partition {
    pub view_metadata: ViewMetadata,
    pub begin_insert_time: DateTime<Utc>,
    pub end_insert_time: DateTime<Utc>,
    pub min_event_time: DateTime<Utc>,
    pub max_event_time: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub file_size: i64,
    pub source_data_hash: Vec<u8>,
}

/// retire_partitions removes out of date partitions from the active set.
/// Overlap is determined by the insert_time of the telemetry.
pub async fn retire_partitions(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    view_set_name: &str,
    view_instance_id: &str,
    begin_insert_time: DateTime<Utc>,
    end_insert_time: DateTime<Utc>,
    writer: Arc<ResponseWriter>,
) -> Result<()> {
    // this is not an overlap test, we need to assume that we are not making a new smaller partition
    // where a bigger one existed
    // its gets tricky in the jit case where a partition can have only one block and begin_insert == end_insert
    let old_partitions = sqlx::query(
        "SELECT file_path, file_size
         FROM lakehouse_partitions
         WHERE view_set_name = $1
         AND view_instance_id = $2
         AND begin_insert_time >= $3
         AND end_insert_time <= $4
         ;",
    )
    .bind(view_set_name)
    .bind(view_instance_id)
    .bind(begin_insert_time)
    .bind(end_insert_time)
    .fetch_all(&mut **transaction)
    .await
    .with_context(|| "listing old partitions")?;
    for old_part in old_partitions {
        let file_path: String = old_part.try_get("file_path")?;
        let file_size: i64 = old_part.try_get("file_size")?;
        let expiration = Utc::now() + TimeDelta::try_hours(1).with_context(|| "making one hour")?;
        writer
            .write_string(&format!(
                "adding out of date partition {file_path} to temporary files to be deleted"
            ))
            .await?;
        sqlx::query("INSERT INTO temporary_files VALUES ($1, $2, $3);")
            .bind(file_path)
            .bind(file_size)
            .bind(expiration)
            .execute(&mut **transaction)
            .await
            .with_context(|| "adding old partition to temporary files to be deleted")?;
    }

    sqlx::query(
        "DELETE from lakehouse_partitions
         WHERE view_set_name = $1
         AND view_instance_id = $2
         AND begin_insert_time >= $3
         AND end_insert_time <= $4
         ;",
    )
    .bind(view_set_name)
    .bind(view_instance_id)
    .bind(begin_insert_time)
    .bind(end_insert_time)
    .execute(&mut **transaction)
    .await
    .with_context(|| "deleting out of date partitions")?;
    Ok(())
}

pub async fn write_partition_from_buffer(
    lake: &DataLakeConnection,
    partition_metadata: &Partition,
    contents: bytes::Bytes,
    writer: Arc<ResponseWriter>,
) -> Result<()> {
    let file_id = uuid::Uuid::new_v4();
    let file_path = format!(
        "views/{}/{}/{}/{}_{file_id}.parquet",
        &partition_metadata.view_metadata.view_set_name,
        &partition_metadata.view_metadata.view_instance_id,
        partition_metadata.begin_insert_time.format("%Y-%m-%d"),
        partition_metadata.begin_insert_time.format("%H-%M-%S")
    );

    lake.blob_storage
        .put(&file_path, contents)
        .await
        .with_context(|| "writing partition to object storage")?;
    let mut tr = lake.db_pool.begin().await?;

    // for jit partitions, we assume that the blocks were registered in order
    // since they are built based on begin_ticks, not insert_time
    retire_partitions(
        &mut tr,
        &partition_metadata.view_metadata.view_set_name,
        &partition_metadata.view_metadata.view_instance_id,
        partition_metadata.begin_insert_time,
        partition_metadata.end_insert_time,
        writer,
    )
    .await
    .with_context(|| "retire_partitions")?;

    sqlx::query(
        "INSERT INTO lakehouse_partitions VALUES($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11);",
    )
    .bind(&*partition_metadata.view_metadata.view_set_name)
    .bind(&*partition_metadata.view_metadata.view_instance_id)
    .bind(partition_metadata.begin_insert_time)
    .bind(partition_metadata.end_insert_time)
    .bind(partition_metadata.min_event_time)
    .bind(partition_metadata.max_event_time)
    .bind(partition_metadata.updated)
    .bind(&file_path)
    .bind(partition_metadata.file_size)
    .bind(&partition_metadata.view_metadata.file_schema_hash)
    .bind(&partition_metadata.source_data_hash)
    .execute(&mut *tr)
    .await
    .with_context(|| "inserting into lakehouse_partitions")?;

    tr.commit().await.with_context(|| "commit")?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn write_partition_from_rows(
    lake: Arc<DataLakeConnection>,
    view_metadata: ViewMetadata,
    begin_insert_time: DateTime<Utc>,
    end_insert_time: DateTime<Utc>,
    source_data_hash: Vec<u8>,
    mut rb_stream: Receiver<PartitionRowSet>,
    capacity: usize,
    response_writer: Arc<ResponseWriter>,
) -> Result<()> {
    // buffer the whole parquet in memory until https://github.com/apache/arrow-rs/issues/5766 is done
    // Impl AsyncFileWriter by object_store #5766
    let mut buffer_writer = bytes::BytesMut::with_capacity(capacity).writer();
    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::LZ4_RAW)
        .build();
    let mut arrow_writer = ArrowWriter::try_new(
        &mut buffer_writer,
        view_metadata.file_schema.clone(),
        Some(props),
    )?;

    let mut min_event_time: Option<DateTime<Utc>> = None;
    let mut max_event_time: Option<DateTime<Utc>> = None;
    while let Some(row_set) = rb_stream.recv().await {
        min_event_time = Some(
            min_event_time
                .unwrap_or(row_set.min_time_row)
                .min(row_set.min_time_row),
        );
        max_event_time = Some(
            max_event_time
                .unwrap_or(row_set.max_time_row)
                .max(row_set.max_time_row),
        );
        arrow_writer
            .write(&row_set.rows)
            .with_context(|| "arrow_writer.write")?;
    }

    let desc = format!(
        "[{}, {}] {} {}",
        view_metadata.view_set_name,
        view_metadata.view_instance_id,
        begin_insert_time.to_rfc3339(),
        end_insert_time.to_rfc3339()
    );
    if min_event_time.is_none() || max_event_time.is_none() {
        response_writer
            .write_string(&format!(
                "no data for {desc} partition, not writing the object"
            ))
            .await?;
        // should we check that there is no stale partition left behind?
        return Ok(());
    }

    arrow_writer.close().with_context(|| "arrow_writer.close")?;

    let buffer: bytes::Bytes = buffer_writer.into_inner().into();
    write_partition_from_buffer(
        &lake,
        &Partition {
            view_metadata,
            begin_insert_time,
            end_insert_time,
            min_event_time: min_event_time.unwrap(),
            max_event_time: max_event_time.unwrap(),
            updated: sqlx::types::chrono::Utc::now(),
            file_size: buffer.len() as i64,
            source_data_hash,
        },
        buffer,
        response_writer,
    )
    .await?;

    Ok(())
}
