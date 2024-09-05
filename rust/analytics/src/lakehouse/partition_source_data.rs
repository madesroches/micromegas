use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use micromegas_telemetry::{stream_info::StreamInfo, types::block::BlockMetadata};
use micromegas_tracing::prelude::*;

use crate::metadata::{block_from_row, process_from_row, stream_from_row};

pub struct PartitionSourceBlock {
    pub block: BlockMetadata,
    pub stream: Arc<StreamInfo>,
    pub process: Arc<ProcessInfo>,
}

pub struct PartitionSourceDataBlocks {
    pub blocks: Vec<Arc<PartitionSourceBlock>>,
    pub block_ids_hash: Vec<u8>,
}

pub fn hash_to_object_count(hash: &[u8]) -> Result<i64> {
    Ok(i64::from_le_bytes(
        hash.try_into().with_context(|| "hash_to_object_count")?,
    ))
}

pub async fn fetch_partition_source_data(
    pool: &sqlx::PgPool,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    source_stream_tag: &str,
) -> Result<PartitionSourceDataBlocks> {
    let desc = format!(
        "[{}, {}] {source_stream_tag}",
        begin_insert.to_rfc3339(),
        end_insert.to_rfc3339()
    );

    // this can scale to thousands, but not millions
    let src_blocks = sqlx::query(
        "SELECT block_id, streams.stream_id, processes.process_id, blocks.begin_time, blocks.begin_ticks, blocks.end_time, blocks.end_ticks, blocks.nb_objects, blocks.object_offset, blocks.payload_size, blocks.insert_time as block_insert_time,
           streams.dependencies_metadata, streams.objects_metadata, streams.tags, streams.properties,
           processes.start_time, processes.start_ticks, processes.tsc_frequency, processes.exe, processes.username, processes.realname, processes.computer, processes.distro, processes.cpu_brand, processes.parent_process_id, processes.properties as process_properties
         FROM blocks, streams, processes
         WHERE blocks.stream_id = streams.stream_id
         AND streams.process_id = processes.process_id
         AND array_position(tags, $1) is not NULL
         AND blocks.insert_time >= $2
         AND blocks.insert_time < $3
         ORDER BY blocks.insert_time, blocks.block_id
         ;",
    )
    .bind(source_stream_tag)
    .bind(begin_insert)
    .bind(end_insert)
    .fetch_all(pool)
    .await
    .with_context(|| "listing source blocks")?;

    info!("{desc} nb_source_blocks={}", src_blocks.len());
    let mut block_ids_hash: i64 = 0;
    let mut partition_src_blocks = vec![];
    for src_block in &src_blocks {
        let block = block_from_row(src_block).with_context(|| "block_from_row")?;
        let process = Arc::new(process_from_row(src_block).with_context(|| "process_from_row")?);
        block_ids_hash += block.nb_objects as i64;
        let stream = Arc::new(stream_from_row(src_block).with_context(|| "stream_from_row")?);
        partition_src_blocks.push(Arc::new(PartitionSourceBlock {
            block,
            stream,
            process,
        }));
    }
    info!("{desc} block_ids_hash={block_ids_hash}");
    Ok(PartitionSourceDataBlocks {
        blocks: partition_src_blocks,
        block_ids_hash: block_ids_hash.to_le_bytes().to_vec(),
    })
}
