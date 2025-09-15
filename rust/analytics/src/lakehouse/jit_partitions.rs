use super::{
    block_partition_spec::{BlockPartitionSpec, BlockProcessor},
    blocks_view::BlocksView,
    partition_cache::{LivePartitionProvider, QueryPartitionProvider},
    partition_source_data::{PartitionSourceBlock, SourceDataBlocksInMemory},
    view::{View, ViewMetadata},
};
use crate::{
    dfext::typed_column::get_single_row_primitive_value,
    lakehouse::{partition_cache::PartitionCache, view::PartitionSpec},
    metadata::block_from_batch_row,
    time::TimeRange,
};
use crate::{
    lakehouse::{partition_source_data::hash_to_object_count, query::query_partitions},
    response_writer::ResponseWriter,
};
use anyhow::{Context, Result};
use chrono::DurationRound;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::{Schema, TimestampNanosecondType},
    execution::runtime_env::RuntimeEnv,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_tracing::prelude::*;
use micromegas_tracing::process_info::ProcessInfo;
use sqlx::Row;
use std::sync::Arc;

/// Configuration for Just-In-Time (JIT) partition generation.
pub struct JitPartitionConfig {
    pub max_nb_objects: i64,
    pub max_insert_time_slice: TimeDelta,
}

impl Default for JitPartitionConfig {
    fn default() -> Self {
        JitPartitionConfig {
            max_nb_objects: 20 * 1024 * 1024,
            max_insert_time_slice: TimeDelta::hours(1),
        }
    }
}

async fn get_insert_time_range(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    blocks_view: &BlocksView,
    query_time_range: &TimeRange,
    stream: Arc<StreamInfo>,
) -> Result<Option<TimeRange>> {
    // we would need a PartitionCache built from event time range and then filtered for insert time range
    let part_provider = LivePartitionProvider::new(lake.db_pool.clone());
    let partitions = part_provider
        .fetch(
            &blocks_view.get_view_set_name(),
            &blocks_view.get_view_instance_id(),
            Some(*query_time_range),
            blocks_view.get_file_schema_hash(),
        )
        .await?;
    let stream_id = &stream.stream_id;
    let begin_range_iso = query_time_range.begin.to_rfc3339();
    let end_range_iso = query_time_range.end.to_rfc3339();
    let sql = format!(
        r#"SELECT MIN(insert_time) as min_insert_time, MAX(insert_time) as max_insert_time
        FROM source
        WHERE stream_id = '{stream_id}'
        AND begin_time <= '{end_range_iso}'
        AND end_time >= '{begin_range_iso}';"#
    );
    let rbs = query_partitions(
        runtime,
        lake,
        blocks_view.get_file_schema(),
        Arc::new(partitions),
        &sql,
    )
    .await?
    .collect()
    .await?;
    if rbs.is_empty() {
        return Ok(None);
    }
    if rbs[0].num_rows() == 0 {
        return Ok(None);
    }
    let min_insert_time = get_single_row_primitive_value::<TimestampNanosecondType>(&rbs, 0)?;
    let max_insert_time = get_single_row_primitive_value::<TimestampNanosecondType>(&rbs, 1)?;
    Ok(Some(TimeRange::new(
        DateTime::from_timestamp_nanos(min_insert_time),
        DateTime::from_timestamp_nanos(max_insert_time),
    )))
}

/// Generates a segment of JIT partitions.
pub async fn generate_stream_jit_partitions_segment(
    config: &JitPartitionConfig,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    blocks_view: &BlocksView,
    insert_time_range: &TimeRange,
    stream: Arc<StreamInfo>,
    process: Arc<ProcessInfo>,
) -> Result<Vec<SourceDataBlocksInMemory>> {
    let cache = PartitionCache::fetch_overlapping_insert_range_for_view(
        &lake.db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        *insert_time_range,
    )
    .await?;
    let partitions = cache.partitions;

    let stream_id = &stream.stream_id;
    let begin_range_iso = insert_time_range.begin.to_rfc3339();
    let end_range_iso = insert_time_range.end.to_rfc3339();
    let sql = format!(
        r#"SELECT block_id, stream_id, process_id, begin_time, end_time, begin_ticks, end_ticks, nb_objects, object_offset, payload_size, insert_time
             FROM source
             WHERE stream_id = '{stream_id}'
             AND insert_time >= '{begin_range_iso}'
             AND insert_time < '{end_range_iso}'
             ORDER BY insert_time, block_id;"#
    );

    let rbs = query_partitions(
        runtime,
        lake,
        blocks_view.get_file_schema(),
        Arc::new(partitions),
        &sql,
    )
    .await?
    .collect()
    .await?;

    let mut partitions = vec![];
    let mut partition_blocks = vec![];
    let mut partition_nb_objects: i64 = 0;
    for rb in rbs {
        for ir in 0..rb.num_rows() {
            let block = block_from_batch_row(&rb, ir).with_context(|| "block_from_batch_row")?;
            let block_nb_objects = block.nb_objects as i64;

            // Check if adding this block would exceed the limit
            if partition_nb_objects + block_nb_objects > config.max_nb_objects
                && !partition_blocks.is_empty()
            {
                // Push current partition without this block
                partitions.push(SourceDataBlocksInMemory {
                    blocks: partition_blocks,
                    block_ids_hash: partition_nb_objects.to_le_bytes().to_vec(),
                });
                // Start new partition with this block
                partition_blocks = vec![Arc::new(PartitionSourceBlock {
                    block,
                    stream: stream.clone(),
                    process: process.clone(),
                })];
                partition_nb_objects = block_nb_objects;
            } else {
                // Add block to current partition
                partition_nb_objects += block_nb_objects;
                partition_blocks.push(Arc::new(PartitionSourceBlock {
                    block,
                    stream: stream.clone(),
                    process: process.clone(),
                }));
            }
        }
    }
    if partition_nb_objects != 0 {
        partitions.push(SourceDataBlocksInMemory {
            blocks: partition_blocks,
            block_ids_hash: partition_nb_objects.to_le_bytes().to_vec(),
        });
    }

    Ok(partitions)
}

/// generate_stream_jit_partitions lists the partitiions that are needed to cover a time span
/// these partitions may not exist or they could be out of date
/// Generates JIT partitions for a given time range.
pub async fn generate_stream_jit_partitions(
    config: &JitPartitionConfig,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    blocks_view: &BlocksView,
    query_time_range: &TimeRange,
    stream: Arc<StreamInfo>,
    process: Arc<ProcessInfo>,
) -> Result<Vec<SourceDataBlocksInMemory>> {
    let insert_time_range = get_insert_time_range(
        runtime.clone(),
        lake.clone(),
        blocks_view,
        query_time_range,
        stream.clone(),
    )
    .await?;
    if insert_time_range.is_none() {
        return Ok(vec![]);
    }
    let insert_time_range = insert_time_range.with_context(|| "missing insert_time_range")?;
    let insert_time_range = TimeRange::new(
        insert_time_range
            .begin
            .duration_trunc(config.max_insert_time_slice)?,
        insert_time_range
            .end
            .duration_trunc(config.max_insert_time_slice)?
            + config.max_insert_time_slice,
    );
    let mut begin_segment = insert_time_range.begin;
    let mut end_segment = begin_segment + config.max_insert_time_slice;
    let mut partitions = vec![];
    while end_segment <= insert_time_range.end {
        let insert_time_range = TimeRange::new(begin_segment, end_segment);
        let mut segment_partitions = generate_stream_jit_partitions_segment(
            config,
            runtime.clone(),
            lake.clone(),
            blocks_view,
            &insert_time_range,
            stream.clone(),
            process.clone(),
        )
        .await?;
        partitions.append(&mut segment_partitions);
        begin_segment = end_segment;
        end_segment = begin_segment + config.max_insert_time_slice;
    }
    Ok(partitions)
}

/// Generates a segment of JIT partitions filtered by process.
pub async fn generate_process_jit_partitions_segment(
    config: &JitPartitionConfig,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    blocks_view: &BlocksView,
    insert_time_range: &TimeRange,
    process: Arc<ProcessInfo>,
    stream_tag: &str,
) -> Result<Vec<SourceDataBlocksInMemory>> {
    let cache = PartitionCache::fetch_overlapping_insert_range_for_view(
        &lake.db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        *insert_time_range,
    )
    .await?;
    let partitions = cache.partitions;

    let process_id = &process.process_id;
    let begin_range_iso = insert_time_range.begin.to_rfc3339();
    let end_range_iso = insert_time_range.end.to_rfc3339();
    let sql = format!(
        r#"SELECT block_id, stream_id, process_id, begin_time, end_time, begin_ticks, end_ticks, nb_objects, object_offset, payload_size, insert_time,
             "streams.dependencies_metadata", "streams.objects_metadata", "streams.tags", "streams.properties"
             FROM source
             WHERE process_id = '{process_id}'
             AND array_has( "streams.tags", '{stream_tag}' )
             AND insert_time >= '{begin_range_iso}'
             AND insert_time < '{end_range_iso}'
             ORDER BY insert_time, block_id;"#
    );

    let rbs = query_partitions(
        runtime.clone(),
        lake.clone(),
        blocks_view.get_file_schema(),
        Arc::new(partitions),
        &sql,
    )
    .await?
    .collect()
    .await?;

    let mut partitions = vec![];
    let mut partition_blocks = vec![];
    let mut partition_nb_objects: i64 = 0;

    for rb in rbs {
        for ir in 0..rb.num_rows() {
            let block = block_from_batch_row(&rb, ir).with_context(|| "block_from_batch_row")?;
            let block_nb_objects = block.nb_objects as i64;

            // Build StreamInfo from the query results
            use crate::arrow_properties::read_property_list;
            use crate::dfext::{
                string_column_accessor::string_column_by_name, typed_column::typed_column_by_name,
            };
            use datafusion::arrow::array::{BinaryArray, GenericListArray, StringArray};
            use uuid::Uuid;

            let stream_id_column = string_column_by_name(&rb, "stream_id")?;
            let stream_process_id_column = string_column_by_name(&rb, "process_id")?;
            let dependencies_metadata_column: &BinaryArray =
                typed_column_by_name(&rb, "streams.dependencies_metadata")?;
            let objects_metadata_column: &BinaryArray =
                typed_column_by_name(&rb, "streams.objects_metadata")?;
            let stream_tags_column: &GenericListArray<i32> =
                typed_column_by_name(&rb, "streams.tags")?;
            let stream_properties_column: &GenericListArray<i32> =
                typed_column_by_name(&rb, "streams.properties")?;

            let stream_id =
                Uuid::parse_str(stream_id_column.value(ir)).with_context(|| "parsing stream_id")?;
            let stream_process_id = Uuid::parse_str(stream_process_id_column.value(ir))
                .with_context(|| "parsing stream process_id")?;

            let dependencies_metadata = dependencies_metadata_column.value(ir);
            let objects_metadata = objects_metadata_column.value(ir);
            let stream_tags = stream_tags_column
                .value(ir)
                .as_any()
                .downcast_ref::<StringArray>()
                .with_context(|| "casting stream_tags")?
                .iter()
                .map(|item| String::from(item.unwrap_or_default()))
                .collect();

            let stream_properties = read_property_list(stream_properties_column.value(ir))?;

            let stream = Arc::new(StreamInfo {
                stream_id,
                process_id: stream_process_id,
                dependencies_metadata: ciborium::from_reader(dependencies_metadata)
                    .with_context(|| "decoding dependencies_metadata")?,
                objects_metadata: ciborium::from_reader(objects_metadata)
                    .with_context(|| "decoding objects_metadata")?,
                tags: stream_tags,
                properties: micromegas_telemetry::property::into_hashmap(stream_properties),
            });

            // Check if adding this block would exceed the limit
            if partition_nb_objects + block_nb_objects > config.max_nb_objects
                && !partition_blocks.is_empty()
            {
                // Push current partition without this block
                partitions.push(SourceDataBlocksInMemory {
                    blocks: partition_blocks,
                    block_ids_hash: partition_nb_objects.to_le_bytes().to_vec(),
                });
                // Start new partition with this block
                partition_blocks = vec![Arc::new(PartitionSourceBlock {
                    block,
                    stream: stream.clone(),
                    process: process.clone(),
                })];
                partition_nb_objects = block_nb_objects;
            } else {
                // Add block to current partition
                partition_nb_objects += block_nb_objects;
                partition_blocks.push(Arc::new(PartitionSourceBlock {
                    block,
                    stream: stream.clone(),
                    process: process.clone(),
                }));
            }
        }
    }
    if partition_nb_objects != 0 {
        partitions.push(SourceDataBlocksInMemory {
            blocks: partition_blocks,
            block_ids_hash: partition_nb_objects.to_le_bytes().to_vec(),
        });
    }
    Ok(partitions)
}

/// generate_process_jit_partitions lists the partitions that are needed to cover a time span for a specific process
/// these partitions may not exist or they could be out of date
/// Generates JIT partitions for a given time range filtered by process.
pub async fn generate_process_jit_partitions(
    config: &JitPartitionConfig,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    blocks_view: &BlocksView,
    query_time_range: &TimeRange,
    process: Arc<ProcessInfo>,
    stream_tag: &str,
) -> Result<Vec<SourceDataBlocksInMemory>> {
    // Get insert time range for all blocks in this process
    let part_provider = LivePartitionProvider::new(lake.db_pool.clone());
    let partitions = part_provider
        .fetch(
            &blocks_view.get_view_set_name(),
            &blocks_view.get_view_instance_id(),
            Some(*query_time_range),
            blocks_view.get_file_schema_hash(),
        )
        .await?;

    let process_id = &process.process_id;
    let begin_range_iso = query_time_range.begin.to_rfc3339();
    let end_range_iso = query_time_range.end.to_rfc3339();
    let sql = format!(
        r#"SELECT MIN(insert_time) as min_insert_time, MAX(insert_time) as max_insert_time
        FROM source
        WHERE process_id = '{process_id}'
        AND array_has( "streams.tags", '{stream_tag}' )
        AND begin_time <= '{end_range_iso}'
        AND end_time >= '{begin_range_iso}';"#
    );

    let rbs = query_partitions(
        runtime.clone(),
        lake.clone(),
        blocks_view.get_file_schema(),
        Arc::new(partitions),
        &sql,
    )
    .await?
    .collect()
    .await?;

    if rbs.is_empty() || rbs[0].num_rows() == 0 {
        return Ok(vec![]);
    }

    let min_insert_time = get_single_row_primitive_value::<TimestampNanosecondType>(&rbs, 0)?;
    let max_insert_time = get_single_row_primitive_value::<TimestampNanosecondType>(&rbs, 1)?;

    if min_insert_time == 0 || max_insert_time == 0 {
        return Ok(vec![]);
    }

    let insert_time_range = TimeRange::new(
        DateTime::from_timestamp_nanos(min_insert_time)
            .duration_trunc(config.max_insert_time_slice)?,
        DateTime::from_timestamp_nanos(max_insert_time)
            .duration_trunc(config.max_insert_time_slice)?
            + config.max_insert_time_slice,
    );

    let mut begin_segment = insert_time_range.begin;
    let mut end_segment = begin_segment + config.max_insert_time_slice;
    let mut partitions = vec![];

    while end_segment <= insert_time_range.end {
        let insert_time_range = TimeRange::new(begin_segment, end_segment);
        let mut segment_partitions = generate_process_jit_partitions_segment(
            config,
            runtime.clone(),
            lake.clone(),
            blocks_view,
            &insert_time_range,
            process.clone(),
            stream_tag,
        )
        .await?;
        partitions.append(&mut segment_partitions);
        begin_segment = end_segment;
        end_segment = begin_segment + config.max_insert_time_slice;
    }
    Ok(partitions)
}

/// is_jit_partition_up_to_date compares a partition spec with the partitions that exist to know if it should be recreated
/// Checks if a JIT partition is up to date.
pub async fn is_jit_partition_up_to_date(
    pool: &sqlx::PgPool,
    view_meta: ViewMetadata,
    spec: &SourceDataBlocksInMemory,
) -> Result<bool> {
    let (min_insert_time, max_insert_time) =
        get_part_insert_time_range(spec).with_context(|| "get_event_time_range")?;
    let desc = format!(
        "[{}, {}] {} {}",
        min_insert_time.to_rfc3339(),
        max_insert_time.to_rfc3339(),
        &*view_meta.view_set_name,
        &*view_meta.view_instance_id,
    );

    // CRITICAL: Use inclusive inequalities (<=, >=) to prevent race conditions.
    // With exclusive inequalities (<, >), identical time ranges never match, causing
    // partitions to be unnecessarily recreated on every query, leading to non-deterministic
    // results. See: https://github.com/madesroches/micromegas/issues/488
    //
    // ADDITIONAL FIX: For identical timestamps (min_insert_time == max_insert_time),
    // we need exact equality matching to handle single-timestamp partitions correctly.
    let rows = if min_insert_time == max_insert_time {
        // For identical timestamps, look for exact matches
        sqlx::query(
            "SELECT file_schema_hash, source_data_hash
             FROM lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND begin_insert_time = $3
             AND end_insert_time = $3
             ;",
        )
        .bind(&*view_meta.view_set_name)
        .bind(&*view_meta.view_instance_id)
        .bind(min_insert_time)
    } else {
        // For time ranges, use inclusive inequalities
        sqlx::query(
            "SELECT file_schema_hash, source_data_hash
             FROM lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND begin_insert_time <= $3
             AND end_insert_time >= $4
             ;",
        )
        .bind(&*view_meta.view_set_name)
        .bind(&*view_meta.view_instance_id)
        .bind(max_insert_time)
        .bind(min_insert_time)
    }
    .fetch_all(pool)
    .await
    .with_context(|| "fetching matching partitions")?;
    if rows.len() != 1 {
        debug!("{desc}: found {} partitions (expected 1)", rows.len());
        for (i, row) in rows.iter().enumerate() {
            let part_file_schema: Vec<u8> = row.try_get("file_schema_hash")?;
            let part_source_data: Vec<u8> = row.try_get("source_data_hash")?;
            debug!(
                "{desc}: partition {}: file_schema_hash={:?}, source_data_hash={:?}",
                i, part_file_schema, part_source_data
            );
        }
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
    let existing_count = hash_to_object_count(&part_source_data)?;
    let required_count = hash_to_object_count(&spec.block_ids_hash)?;
    if existing_count < required_count {
        info!("{desc}: existing partition lacks source data: creating a new partition");
        return Ok(false);
    }
    info!("{desc}: partition up to date");
    Ok(true)
}

/// get_event_time_range returns the time range covered by a partition spec
/// Returns the event time range covered by a partition spec.
fn get_part_insert_time_range(
    spec: &SourceDataBlocksInMemory,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    if spec.blocks.is_empty() {
        anyhow::bail!("empty partition should not exist");
    }
    // blocks need to be sorted by (event & insert) time
    let min_insert_time = spec.blocks[0].block.insert_time;
    let max_insert_time = spec.blocks[spec.blocks.len() - 1].block.insert_time;
    Ok((min_insert_time, max_insert_time))
}

/// Writes a partition from a set of blocks.
pub async fn write_partition_from_blocks(
    lake: Arc<DataLakeConnection>,
    view_metadata: ViewMetadata,
    schema: Arc<Schema>,
    source_data: SourceDataBlocksInMemory,
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
        insert_range: TimeRange::new(min_insert_time, max_insert_time),
        source_data: Arc::new(source_data),
        block_processor,
    };
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    block_spec
        .write(lake, null_response_writer)
        .await
        .with_context(|| "block_spec.write")?;
    Ok(())
}
