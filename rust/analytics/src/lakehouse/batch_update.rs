use crate::{
    log_entries_table::{log_table_schema, LogEntriesRecordBuilder},
    log_entry::for_each_log_entry_in_block,
    metadata::{block_from_row, stream_from_row},
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use bytes::BufMut;
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use datafusion::parquet::{
    arrow::ArrowWriter,
    basic::Compression,
    file::properties::{WriterProperties, WriterVersion},
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::info;
use sqlx::Row;
use std::sync::Arc;
use xxhash_rust::xxh32::xxh32;

pub struct Partition {
    pub table_set_name: String,
    pub table_instance_id: String,
    pub begin_insert_time: chrono::DateTime<chrono::Utc>,
    pub end_insert_time: chrono::DateTime<chrono::Utc>,
    pub min_event_time: chrono::DateTime<chrono::Utc>,
    pub max_event_time: chrono::DateTime<chrono::Utc>,
    pub updated: chrono::DateTime<chrono::Utc>,
    pub file_path: String,
    pub file_size: i64,
    pub file_schema_hash: Vec<u8>,
    pub source_data_hash: Vec<u8>,
}

async fn write_partition(
    lake: &DataLakeConnection,
    partition_metadata: &Partition,
    contents: bytes::Bytes,
) -> Result<()> {
    lake.blob_storage
        .put(&partition_metadata.file_path, contents)
        .await
        .with_context(|| "writing partition to object storage")?;
    let mut tr = lake.db_pool.begin().await?;
    sqlx::query(
        "INSERT INTO lakehouse_partitions VALUES($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11);",
    )
    .bind(&partition_metadata.table_set_name)
    .bind(&partition_metadata.table_instance_id)
    .bind(partition_metadata.begin_insert_time)
    .bind(partition_metadata.end_insert_time)
    .bind(partition_metadata.min_event_time)
    .bind(partition_metadata.max_event_time)
    .bind(partition_metadata.updated)
    .bind(&partition_metadata.file_path)
    .bind(partition_metadata.file_size)
    .bind(&partition_metadata.file_schema_hash)
    .bind(&partition_metadata.source_data_hash)
    .execute(&mut *tr)
    .await
    .with_context(|| "inserting into lakehouse_partitions")?;
    tr.commit().await.with_context(|| "commit")?;
    Ok(())
}

async fn update_log_partition(
    lake: &DataLakeConnection,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<()> {
    info!("updating log partition from {begin} to {end}");
    // this can scale to thousands, but not millions
    let src_blocks = sqlx::query(
        "SELECT block_id, streams.stream_id, processes.process_id, blocks.begin_time, blocks.begin_ticks, blocks.end_time, blocks.end_ticks, blocks.nb_objects, blocks.object_offset, blocks.payload_size,
           streams.dependencies_metadata, streams.objects_metadata, streams.tags, streams.properties,
           processes.start_time, processes.start_ticks, processes.tsc_frequency
         FROM blocks, streams, processes
         WHERE blocks.stream_id = streams.stream_id
         AND streams.process_id = processes.process_id
         AND array_position(tags, $1) is not NULL
         AND blocks.insert_time >= $2
         AND blocks.insert_time < $3
         ;",
    )
    .bind("log")
    .bind(begin)
    .bind(end)
    .fetch_all(&lake.db_pool)
    .await
    .with_context(|| "listing source blocks")?;

    // todo: find existing partition in metadata

    info!("nb_source_blocks: {}", src_blocks.len());

    // buffer the whole parquet in memory until https://github.com/apache/arrow-rs/issues/5766 is done
    // Impl AsyncFileWriter by object_store #5766
    let mut buffer_writer = bytes::BytesMut::with_capacity(1024 * 1024).writer();
    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::LZ4_RAW)
        .build();

    let schema = Arc::new(log_table_schema());

    let mut arrow_writer = ArrowWriter::try_new(&mut buffer_writer, schema, Some(props))?;

    let mut min_time = None;
    let mut max_time = None;
    let mut block_ids_hash = 0;
    for src_block in src_blocks {
        let block = block_from_row(&src_block).with_context(|| "block_from_row")?;
        block_ids_hash = xxh32(block.block_id.as_bytes(), block_ids_hash);
        let stream = stream_from_row(&src_block).with_context(|| "stream_from_row")?;
        let process_start_time: chrono::DateTime<chrono::Utc> = src_block.try_get("start_time")?;
        let process_start_ticks: i64 = src_block.try_get("start_ticks")?;
        let tsc_frequency: i64 = src_block.try_get("tsc_frequency")?;
        let convert_ticks = ConvertTicks::from_meta_data(
            process_start_ticks,
            process_start_time.timestamp_nanos_opt().unwrap_or_default(),
            tsc_frequency,
        );
        let nb_log_entries: i32 = src_block.try_get("nb_objects")?;
        let mut record_builder = LogEntriesRecordBuilder::with_capacity(nb_log_entries as usize);

        for_each_log_entry_in_block(
            lake.blob_storage.clone(),
            &convert_ticks,
            &stream,
            &block,
            |log_entry| {
                record_builder.append(&log_entry)?;
                Ok(true) // continue
            },
        )
        .await
        .with_context(|| "for_each_log_entry_in_block")?;

        if let Some(time_range) = record_builder.get_time_range() {
            min_time = Some(min_time.unwrap_or(time_range.0).min(time_range.0));
            max_time = Some(max_time.unwrap_or(time_range.1).max(time_range.1));
            let record_batch = record_builder.finish()?;
            arrow_writer.write(&record_batch)?;
        }
    }
    arrow_writer.close()?;

    let table_set_name = "logs";
    let table_instance_id = "global";

    let file_id = uuid::Uuid::new_v4();
    let file_path = format!(
        "views/{}/{}/minutes/{}/{file_id}.parquet",
        table_set_name,
        table_instance_id,
        begin.format("%Y-%m-%d-%H-%M-%S")
    );
    if min_time.is_none() || max_time.is_none() {
        info!("no data for {file_path} partition, not writing the object");
        return Ok(());
    }
    let buffer: bytes::Bytes = buffer_writer.into_inner().into();
    write_partition(
        lake,
        &Partition {
            table_set_name: table_set_name.to_owned(),
            table_instance_id: table_instance_id.to_owned(),
            begin_insert_time: begin,
            end_insert_time: end,
            min_event_time: min_time.map(DateTime::<Utc>::from_timestamp_nanos).unwrap(),
            max_event_time: max_time.map(DateTime::<Utc>::from_timestamp_nanos).unwrap(),
            updated: sqlx::types::chrono::Utc::now(),
            file_path,
            file_size: buffer.len() as i64,
            file_schema_hash: vec![0],
            source_data_hash: block_ids_hash.to_le_bytes().to_vec(),
        },
        buffer,
    )
    .await?;

    Ok(())
}

pub async fn update_partitions(lake: &DataLakeConnection) -> Result<()> {
    let now = Utc::now();
    let one_minute = TimeDelta::try_minutes(1).with_context(|| "making a minute")?;
    let truncated = now.duration_trunc(one_minute)?;
    let nb_minute_partitions: i32 = 15;
    let start = truncated
        - TimeDelta::try_minutes(nb_minute_partitions as i64)
            .with_context(|| "making time delta")?;
    for index in 0..nb_minute_partitions {
        let start_partition = start + one_minute * index;
        let end_partition = start + one_minute * (index + 1);
        update_log_partition(lake, start_partition, end_partition)
            .await
            .with_context(|| "update_log_partition")?;
    }
    Ok(())
}
