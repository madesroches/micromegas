use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use micromegas_telemetry::{stream_info::StreamInfo, types::block::BlockMetadata};
use micromegas_tracing::prelude::*;
use sqlx::Row;
use xxhash_rust::xxh32::xxh32;

use crate::metadata::{block_from_row, process_from_row, stream_from_row};

pub struct PartitionSourceBlock {
    pub block: BlockMetadata,
    pub stream: StreamInfo,
    pub process: Arc<ProcessInfo>,
    pub process_start_time: DateTime<Utc>,
    pub process_start_ticks: i64,
    pub process_tsc_frequency: i64,
}

pub struct PartitionSourceDataBlocks {
    pub blocks: Vec<PartitionSourceBlock>,
    pub block_ids_hash: Vec<u8>,
}

pub async fn fetch_partition_source_data(
    pool: &sqlx::PgPool,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    source_stream_tag: &str,
) -> Result<PartitionSourceDataBlocks> {
    // this can scale to thousands, but not millions
    let src_blocks = sqlx::query(
        "SELECT block_id, streams.stream_id, processes.process_id, blocks.begin_time, blocks.begin_ticks, blocks.end_time, blocks.end_ticks, blocks.nb_objects, blocks.object_offset, blocks.payload_size,
           streams.dependencies_metadata, streams.objects_metadata, streams.tags, streams.properties,
           processes.start_time, processes.start_ticks, processes.tsc_frequency, processes.exe, processes.username, processes.realname, processes.computer, processes.distro, processes.cpu_brand, processes.parent_process_id, processes.properties as process_properties
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
        let process = Arc::new(process_from_row(src_block).with_context(|| "process_from_row")?);
        block_ids_hash = xxh32(block.block_id.as_bytes(), block_ids_hash);
        partition_src_blocks.push(PartitionSourceBlock {
            block,
            stream: stream_from_row(src_block).with_context(|| "stream_from_row")?,
            process,
            process_start_time: src_block.try_get("start_time")?,
            process_start_ticks: src_block.try_get("start_ticks")?,
            process_tsc_frequency: src_block.try_get("tsc_frequency")?,
        });
    }
    Ok(PartitionSourceDataBlocks {
        blocks: partition_src_blocks,
        block_ids_hash: block_ids_hash.to_le_bytes().to_vec(),
    })
}
