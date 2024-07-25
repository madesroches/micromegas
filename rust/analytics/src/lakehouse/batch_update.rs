use crate::{
    lakehouse::partition::write_partition,
    log_entries_table::{log_table_schema, LogEntriesRecordBuilder},
    log_entry::for_each_log_entry_in_block,
    metadata::{block_from_row, stream_from_row},
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use bytes::BufMut;
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use datafusion::{
    arrow::array::RecordBatch,
    parquet::{
        arrow::ArrowWriter,
        basic::Compression,
        file::properties::{WriterProperties, WriterVersion},
    },
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::{
    blob_storage::BlobStorage, stream_info::StreamInfo, types::block::BlockMetadata,
};
use micromegas_tracing::{error, info};
use sqlx::Row;
use std::sync::Arc;
use xxhash_rust::xxh32::xxh32;

use super::partition::Partition;

struct PartitionSourceBlock {
    pub block: BlockMetadata,
    pub stream: StreamInfo,
    pub process_start_time: DateTime<Utc>,
    pub process_start_ticks: i64,
    pub process_tsc_frequency: i64,
}

struct PartitionSourceData {
    pub blocks: Vec<PartitionSourceBlock>,
    pub block_ids_hash: Vec<u8>,
}

async fn fetch_partition_source_data(
    pool: &sqlx::PgPool,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    source_stream_tag: &str,
) -> Result<PartitionSourceData> {
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
    .bind(source_stream_tag)
    .bind(begin_insert)
    .bind(end_insert)
    .fetch_all(pool)
    .await
    .with_context(|| "listing source blocks")?;

    info!("nb_source_blocks: {}", src_blocks.len());
    let mut block_ids_hash = 0;
    let mut partition_src_blocks = vec![];
    for src_block in &src_blocks {
        let block = block_from_row(src_block).with_context(|| "block_from_row")?;
        block_ids_hash = xxh32(block.block_id.as_bytes(), block_ids_hash);
        partition_src_blocks.push(PartitionSourceBlock {
            block,
            stream: stream_from_row(src_block).with_context(|| "stream_from_row")?,
            process_start_time: src_block.try_get("start_time")?,
            process_start_ticks: src_block.try_get("start_ticks")?,
            process_tsc_frequency: src_block.try_get("tsc_frequency")?,
        });
    }
    Ok(PartitionSourceData {
        blocks: partition_src_blocks,
        block_ids_hash: block_ids_hash.to_le_bytes().to_vec(),
    })
}

async fn count_equal_partitions(
    pool: &sqlx::PgPool,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    table_set_name: &str,
    table_instance_id: &str,
    block_ids_hash: &[u8],
) -> Result<i64> {
    let count: i64 = sqlx::query(
        "SELECT count(*) as count
         FROM lakehouse_partitions
         WHERE table_set_name = $1
         AND table_instance_id = $2
         AND begin_insert_time = $3
         AND end_insert_time = $4
         AND source_data_hash = $5
         ;",
    )
    .bind(table_set_name)
    .bind(table_instance_id)
    .bind(begin_insert)
    .bind(end_insert)
    .bind(block_ids_hash)
    .fetch_one(pool)
    .await
    .with_context(|| "counting matching partitions")?
    .try_get("count")?;
    Ok(count)
}

pub struct PartitionRowSet {
    pub min_time_row: i64,
    pub max_time_row: i64,
    pub rows: RecordBatch,
}

async fn fetch_log_block_row_set(
    blob_storage: Arc<BlobStorage>,
    src_block: &PartitionSourceBlock,
) -> Result<Option<PartitionRowSet>> {
    let convert_ticks = ConvertTicks::from_meta_data(
        src_block.process_start_ticks,
        src_block
            .process_start_time
            .timestamp_nanos_opt()
            .unwrap_or_default(),
        src_block.process_tsc_frequency,
    );
    let nb_log_entries = src_block.block.nb_objects;
    let mut record_builder = LogEntriesRecordBuilder::with_capacity(nb_log_entries as usize);

    for_each_log_entry_in_block(
        blob_storage,
        &convert_ticks,
        &src_block.stream,
        &src_block.block,
        |log_entry| {
            record_builder.append(&log_entry)?;
            Ok(true) // continue
        },
    )
    .await
    .with_context(|| "for_each_log_entry_in_block")?;

    if let Some(time_range) = record_builder.get_time_range() {
        let record_batch = record_builder.finish()?;
        Ok(Some(PartitionRowSet {
            min_time_row: time_range.0,
            max_time_row: time_range.1,
            rows: record_batch,
        }))
    } else {
        Ok(None)
    }
}

async fn create_or_update_partition(
    lake: &DataLakeConnection,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    table_set_name: &str,
    table_instance_id: &str,
    source_stream_tag: &str,
) -> Result<()> {
    info!("updating {table_set_name} partition from {begin_insert} to {end_insert}");

    let source_data =
        fetch_partition_source_data(&lake.db_pool, begin_insert, end_insert, source_stream_tag)
            .await
            .with_context(|| "FetchPartitionSourceData")?;

    let nb_equal_partitions = count_equal_partitions(
        &lake.db_pool,
        begin_insert,
        end_insert,
        table_set_name,
        table_instance_id,
        &source_data.block_ids_hash,
    )
    .await?;

    if nb_equal_partitions == 1 {
        info!("partition up to date, no need to replace it");
        return Ok(());
    }
    if nb_equal_partitions > 1 {
        error!("too many partitions for the same time range");
        return Ok(()); // continue with the rest of the process, the error has been logged
    }

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
    for src_block in source_data.blocks {
        if let Some(row_set) =
            fetch_log_block_row_set(lake.blob_storage.clone(), &src_block).await?
        {
            min_time = Some(
                min_time
                    .unwrap_or(row_set.min_time_row)
                    .min(row_set.min_time_row),
            );
            max_time = Some(
                max_time
                    .unwrap_or(row_set.max_time_row)
                    .max(row_set.max_time_row),
            );
            arrow_writer.write(&row_set.rows)?;
        }
    }
    arrow_writer.close()?;

    let file_id = uuid::Uuid::new_v4();
    let file_path = format!(
        "views/{}/{}/minutes/{}/{file_id}.parquet",
        table_set_name,
        table_instance_id,
        begin_insert.format("%Y-%m-%d-%H-%M-%S")
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
            begin_insert_time: begin_insert,
            end_insert_time: end_insert,
            min_event_time: min_time.map(DateTime::<Utc>::from_timestamp_nanos).unwrap(),
            max_event_time: max_time.map(DateTime::<Utc>::from_timestamp_nanos).unwrap(),
            updated: sqlx::types::chrono::Utc::now(),
            file_path,
            file_size: buffer.len() as i64,
            file_schema_hash: vec![0],
            source_data_hash: source_data.block_ids_hash,
        },
        buffer,
    )
    .await?;

    Ok(())
}

pub async fn create_or_update_minute_partitions(
    lake: &DataLakeConnection,
    table_set_name: &str,
    table_instance_id: &str,
    source_stream_tag: &str,
) -> Result<()> {
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
        create_or_update_partition(
            lake,
            start_partition,
            end_partition,
            table_set_name,
            table_instance_id,
            source_stream_tag,
        )
        .await
        .with_context(|| "update_log_partition")?;
    }
    Ok(())
}