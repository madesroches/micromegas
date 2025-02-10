use super::{
    block_partition_spec::{BlockPartitionSpec, BlockProcessor},
    partition_source_data::{PartitionSourceBlock, PartitionSourceDataBlocks},
    view::ViewMetadata,
};
use crate::lakehouse::view::PartitionSpec;
use crate::{
    lakehouse::partition_source_data::hash_to_object_count, metadata::block_from_row,
    response_writer::ResponseWriter, time::ConvertTicks,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use datafusion::arrow::datatypes::Schema;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_tracing::prelude::*;
use micromegas_tracing::process_info::ProcessInfo;
use sqlx::Row;
use std::sync::Arc;

const NB_OBJECTS_PER_PARTITION: i64 = 20 * 1024 * 1024;

/// generate_jit_partitions lists the partitiions that are needed to cover a time span
/// these partitions may not exist or they could be out of date
pub async fn generate_jit_partitions(
    pool: &sqlx::Pool<sqlx::Postgres>,
    begin_query: DateTime<Utc>,
    end_query: DateTime<Utc>,
    stream: Arc<StreamInfo>,
    process: Arc<ProcessInfo>,
    convert_ticks: &ConvertTicks,
) -> Result<Vec<PartitionSourceDataBlocks>> {
    let relative_begin_ticks = convert_ticks.time_to_delta_ticks(begin_query);
    let relative_end_ticks = convert_ticks.time_to_delta_ticks(end_query);
    // we go though all the blocks before the end of the query to avoid
    // making a fragmented partition list over time
    let rows = sqlx::query(
            "SELECT block_id, stream_id, process_id, begin_time, end_time, begin_ticks, end_ticks, nb_objects, object_offset, payload_size, insert_time as block_insert_time
             FROM blocks
             WHERE stream_id = $1
             AND begin_ticks <= $2
             ORDER BY begin_ticks;",
        )
        .bind(stream.stream_id)
        .bind(relative_end_ticks)
        .fetch_all(pool)
        .await
        .with_context(|| "listing blocks")?;

    let mut partitions = vec![];
    let mut partition_blocks = vec![];
    let mut partition_nb_objects: i64 = 0;
    let mut last_block_end_ticks: i64 = 0;
    // we could do a smarter search using object_offset
    for r in rows {
        let block = block_from_row(&r)?;
        last_block_end_ticks = block.end_ticks;
        partition_nb_objects += block.nb_objects as i64;
        partition_blocks.push(Arc::new(PartitionSourceBlock {
            block,
            stream: stream.clone(),
            process: process.clone(),
        }));

        // should we also add a threshold on the number of source blocks?
        if partition_nb_objects > NB_OBJECTS_PER_PARTITION {
            if last_block_end_ticks > relative_begin_ticks {
                partitions.push(PartitionSourceDataBlocks {
                    blocks: partition_blocks,
                    block_ids_hash: partition_nb_objects.to_le_bytes().to_vec(),
                });
            }
            partition_blocks = vec![];
            partition_nb_objects = 0;
        }
    }
    if partition_nb_objects != 0 && last_block_end_ticks > relative_begin_ticks {
        partitions.push(PartitionSourceDataBlocks {
            blocks: partition_blocks,
            block_ids_hash: partition_nb_objects.to_le_bytes().to_vec(),
        });
    }
    Ok(partitions)
}

/// is_jit_partition_up_to_date compares a partition spec with the partitions that exist to know if it should be recreated
pub async fn is_jit_partition_up_to_date(
    pool: &sqlx::PgPool,
    view_meta: ViewMetadata,
    convert_ticks: &ConvertTicks,
    spec: &PartitionSourceDataBlocks,
) -> Result<bool> {
    let (min_event_time, max_event_time) =
        get_event_time_range(convert_ticks, spec).with_context(|| "get_event_time_range")?;
    let desc = format!(
        "[{}, {}] {} {}",
        min_event_time.to_rfc3339(),
        max_event_time.to_rfc3339(),
        &*view_meta.view_set_name,
        &*view_meta.view_instance_id,
    );

    let rows = sqlx::query(
        "SELECT file_schema_hash, source_data_hash
         FROM lakehouse_partitions
         WHERE view_set_name = $1
         AND view_instance_id = $2
         AND min_event_time < $3
         AND max_event_time > $4
         AND file_metadata IS NOT NULL
         ;",
    )
    .bind(&*view_meta.view_set_name)
    .bind(&*view_meta.view_instance_id)
    .bind(max_event_time)
    .bind(min_event_time)
    .fetch_all(pool)
    .await
    .with_context(|| "fetching matching partitions")?;
    if rows.len() != 1 {
        info!("{desc}: found {} partitions", rows.len());
        return Ok(false);
    }
    let r = &rows[0];
    let part_file_schema: Vec<u8> = r.try_get("file_schema_hash")?;
    if part_file_schema != view_meta.file_schema_hash {
        // this is dangerous because we could be creating a new partition smaller than the old one, which is not supported.
        // let's make sure there is no old data loitering
        warn!("{desc}: found matching partition with different file schema");
        return Ok(false);
    }
    let part_source_data: Vec<u8> = r.try_get("source_data_hash")?;
    if hash_to_object_count(&part_source_data)? < hash_to_object_count(&spec.block_ids_hash)? {
        info!("{desc}: existing partition lacks source data: creating a new partition");
        return Ok(false);
    }
    info!("{desc}: partition up to date");
    Ok(true)
}

/// get_event_time_range returns the time range covered by a partition spec
fn get_event_time_range(
    convert_ticks: &ConvertTicks,
    spec: &PartitionSourceDataBlocks,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    if spec.blocks.is_empty() {
        anyhow::bail!("empty partition should not exist");
    }
    // blocks need to be sorted by (event & insert) time
    let min_rel_ticks = spec.blocks[0].block.begin_ticks;
    let max_rel_ticks = spec.blocks[spec.blocks.len() - 1].block.end_ticks;
    Ok((
        convert_ticks.delta_ticks_to_time(min_rel_ticks),
        convert_ticks.delta_ticks_to_time(max_rel_ticks),
    ))
}

pub async fn write_partition_from_blocks(
    lake: Arc<DataLakeConnection>,
    view_metadata: ViewMetadata,
    schema: Arc<Schema>,
    source_data: PartitionSourceDataBlocks,
    block_processor: Arc<dyn BlockProcessor>,
) -> Result<()> {
    if source_data.blocks.is_empty() {
        anyhow::bail!("empty partition spec");
    }
    // blocks need to be sorted by (event & insert) time
    let min_insert_time = source_data.blocks[0].block.insert_time;
    let max_insert_time = source_data.blocks[source_data.blocks.len() - 1]
        .block
        .insert_time;
    let block_spec = BlockPartitionSpec {
        view_metadata,
        schema,
        begin_insert: min_insert_time,
        end_insert: max_insert_time,
        source_data,
        block_processor,
    };
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    block_spec
        .write(lake, null_response_writer)
        .await
        .with_context(|| "block_spec.write")?;
    Ok(())
}
