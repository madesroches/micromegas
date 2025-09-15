use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use datafusion::arrow::array::{
    Array, Int32Array, Int64Array, ListArray, RecordBatch, TimestampNanosecondArray,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::{
    property::Property, stream_info::StreamInfo, types::block::BlockMetadata,
};
use micromegas_tracing::{prelude::*, process_info::ProcessInfo};
use micromegas_transit::{UserDefinedType, uuid_utils::parse_optional_uuid};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    arrow_properties::read_property_list,
    dfext::{string_column_accessor::string_column_by_name, typed_column::typed_column_by_name},
    lakehouse::{
        partition_cache::LivePartitionProvider, query::make_session_context,
        view_factory::ViewFactory,
    },
    time::TimeRange,
};
use datafusion::execution::runtime_env::RuntimeEnv;

/// Creates a `StreamInfo` from a database row.
pub fn stream_from_row(row: &sqlx::postgres::PgRow) -> Result<StreamInfo> {
    let dependencies_metadata_buffer: Vec<u8> = row.try_get("dependencies_metadata")?;
    let dependencies_metadata: Vec<UserDefinedType> =
        ciborium::from_reader(&dependencies_metadata_buffer[..])
            .with_context(|| "decoding dependencies metadata")?;
    let objects_metadata_buffer: Vec<u8> = row.try_get("objects_metadata")?;
    let objects_metadata: Vec<UserDefinedType> =
        ciborium::from_reader(&objects_metadata_buffer[..])
            .with_context(|| "decoding objects metadata")?;
    let tags: Vec<String> = row.try_get("tags")?;
    let properties: Vec<Property> = row.try_get("properties")?;
    Ok(StreamInfo {
        stream_id: row.try_get("stream_id")?,
        process_id: row.try_get("process_id")?,
        dependencies_metadata,
        objects_metadata,
        tags,
        properties: micromegas_telemetry::property::into_hashmap(properties),
    })
}

/// Finds a stream by its ID.
#[span_fn]
pub async fn find_stream(
    pool: &sqlx::Pool<sqlx::Postgres>,
    stream_id: sqlx::types::Uuid,
) -> Result<StreamInfo> {
    let row = sqlx::query(
        "SELECT stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties
         FROM streams
         WHERE stream_id = $1
         ;",
    )
    .bind(stream_id)
    .fetch_one(pool)
    .await
    .with_context(|| "select from streams")?;
    stream_from_row(&row)
}

/// Lists all streams for a given process that are tagged with a specific tag.
pub async fn list_process_streams_tagged(
    pool: &sqlx::Pool<sqlx::Postgres>,
    process_id: sqlx::types::Uuid,
    tag: &str,
) -> Result<Vec<StreamInfo>> {
    let stream_rows = sqlx::query(
        "SELECT stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties
         FROM streams
         WHERE process_id = $1
         AND array_position(tags, $2) is not NULL
         ;",
    )
    .bind(process_id)
    .bind(tag)
    .fetch_all(pool)
    .await
    .with_context(|| "fetching streams")?;
    let mut streams = vec![];
    for row in stream_rows {
        let stream = stream_from_row(&row).with_context(|| "stream_from_row")?;
        streams.push(stream);
    }
    Ok(streams)
}

/// Creates a `ProcessInfo` from a database row.
#[span_fn]
pub fn process_from_row(row: &sqlx::postgres::PgRow) -> Result<ProcessInfo> {
    let properties: Vec<Property> = row.try_get("process_properties")?;
    Ok(ProcessInfo {
        process_id: row.try_get("process_id")?,
        exe: row.try_get("exe")?,
        username: row.try_get("username")?,
        realname: row.try_get("realname")?,
        computer: row.try_get("computer")?,
        distro: row.try_get("distro")?,
        cpu_brand: row.try_get("cpu_brand")?,
        tsc_frequency: row.try_get("tsc_frequency")?,
        start_time: row.try_get("start_time")?,
        start_ticks: row.try_get("start_ticks")?,
        parent_process_id: row.try_get("parent_process_id")?,
        properties: micromegas_telemetry::property::into_hashmap(properties),
    })
}

/// Finds a process by its ID.
#[span_fn]
pub async fn find_process(
    pool: &sqlx::Pool<sqlx::Postgres>,
    process_id: &sqlx::types::Uuid,
) -> Result<ProcessInfo> {
    let row = sqlx::query(
        "SELECT process_id,
                exe,
                username,
                realname,
                computer,
                distro,
                cpu_brand,
                tsc_frequency,
                start_time,
                start_ticks,
                parent_process_id,
                properties as process_properties
         FROM processes
         WHERE process_id = $1;",
    )
    .bind(process_id)
    .fetch_one(pool)
    .await
    .with_context(|| "select from processes")?;
    process_from_row(&row)
}

/// Finds a process and its latest timing information using DataFusion.
/// Returns (ProcessInfo, last_block_end_ticks, last_block_end_time)
#[span_fn]
pub async fn find_process_with_latest_timing(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
    process_id: &Uuid,
    query_range: Option<TimeRange>,
) -> Result<(ProcessInfo, i64, DateTime<Utc>)> {
    let partition_provider = Arc::new(LivePartitionProvider::new(lake.db_pool.clone()));

    let ctx = make_session_context(
        runtime,
        lake.clone(),
        partition_provider,
        query_range,
        view_factory,
    )
    .await
    .with_context(|| "creating DataFusion session context")?;

    let sql = format!(
        "SELECT process_id, exe, username, realname, computer, distro, cpu_brand,
                tsc_frequency, start_time, start_ticks, parent_process_id, properties,
                last_block_end_ticks, last_block_end_time
         FROM processes
         WHERE process_id = '{}'",
        process_id
    );

    let df = instrument_named!(ctx.sql(&sql), "datafusion_sql_query")
        .await
        .with_context(|| "executing DataFusion query for process with latest timing")?;

    let batches = instrument_named!(df.collect(), "datafusion_collect")
        .await
        .with_context(|| "collecting DataFusion query results")?;

    if batches.is_empty() || batches[0].num_rows() == 0 {
        anyhow::bail!("Process not found: {}", process_id);
    }

    let batch = &batches[0];

    // Extract process fields
    let process_id_column = string_column_by_name(batch, "process_id")?;
    let exe_column = string_column_by_name(batch, "exe")?;
    let username_column = string_column_by_name(batch, "username")?;
    let realname_column = string_column_by_name(batch, "realname")?;
    let computer_column = string_column_by_name(batch, "computer")?;
    let distro_column = string_column_by_name(batch, "distro")?;
    let cpu_brand_column = string_column_by_name(batch, "cpu_brand")?;
    let tsc_frequency_column: &Int64Array = typed_column_by_name(batch, "tsc_frequency")?;
    let start_time_column: &TimestampNanosecondArray = typed_column_by_name(batch, "start_time")?;
    let start_ticks_column: &Int64Array = typed_column_by_name(batch, "start_ticks")?;
    let last_block_end_ticks_column: &Int64Array =
        typed_column_by_name(batch, "last_block_end_ticks")?;
    let last_block_end_time_column: &TimestampNanosecondArray =
        typed_column_by_name(batch, "last_block_end_time")?;
    let parent_process_id_column = string_column_by_name(batch, "parent_process_id")?;
    let properties_column: &ListArray = typed_column_by_name(batch, "properties")?;

    let parent_process_id = if parent_process_id_column.is_null(0) {
        None
    } else {
        parse_optional_uuid(parent_process_id_column.value(0))?
    };

    let properties = if properties_column.is_null(0) {
        Default::default()
    } else {
        let properties_list = read_property_list(properties_column.value(0))?;
        micromegas_telemetry::property::into_hashmap(properties_list)
    };

    let process_info = ProcessInfo {
        process_id: parse_optional_uuid(process_id_column.value(0))?
            .ok_or_else(|| anyhow::anyhow!("process_id cannot be empty"))?,
        exe: exe_column.value(0).to_string(),
        username: username_column.value(0).to_string(),
        realname: realname_column.value(0).to_string(),
        computer: computer_column.value(0).to_string(),
        distro: distro_column.value(0).to_string(),
        cpu_brand: cpu_brand_column.value(0).to_string(),
        tsc_frequency: tsc_frequency_column.value(0),
        start_time: DateTime::from_timestamp_nanos(start_time_column.value(0)),
        start_ticks: start_ticks_column.value(0),
        parent_process_id,
        properties,
    };

    let last_block_end_ticks = last_block_end_ticks_column.value(0);
    let last_block_end_time = DateTime::from_timestamp_nanos(last_block_end_time_column.value(0));

    Ok((process_info, last_block_end_ticks, last_block_end_time))
}
/// Creates a `BlockMetadata` from a database row.
#[span_fn]
pub fn block_from_row(row: &sqlx::postgres::PgRow) -> Result<BlockMetadata> {
    Ok(BlockMetadata {
        block_id: row.try_get("block_id")?,
        stream_id: row.try_get("stream_id")?,
        process_id: row.try_get("process_id")?,
        begin_time: row.try_get("begin_time")?,
        end_time: row.try_get("end_time")?,
        begin_ticks: row.try_get("begin_ticks")?,
        end_ticks: row.try_get("end_ticks")?,
        nb_objects: row.try_get("nb_objects")?,
        object_offset: row.try_get("object_offset")?,
        payload_size: row.try_get("payload_size")?,
        insert_time: row.try_get("insert_time")?,
    })
}

/// Creates a `BlockMetadata` from a recordbatch row.
#[span_fn]
pub fn block_from_batch_row(rb: &RecordBatch, row: usize) -> Result<BlockMetadata> {
    let block_id_column = string_column_by_name(rb, "block_id")?;
    let stream_id_column = string_column_by_name(rb, "stream_id")?;
    let process_id_column = string_column_by_name(rb, "process_id")?;
    let begin_time_column: &TimestampNanosecondArray = typed_column_by_name(rb, "begin_time")?;
    let begin_ticks_column: &Int64Array = typed_column_by_name(rb, "begin_ticks")?;
    let end_time_column: &TimestampNanosecondArray = typed_column_by_name(rb, "end_time")?;
    let end_ticks_column: &Int64Array = typed_column_by_name(rb, "end_ticks")?;
    let nb_objects_column: &Int32Array = typed_column_by_name(rb, "nb_objects")?;
    let object_offset_column: &Int64Array = typed_column_by_name(rb, "object_offset")?;
    let payload_size_column: &Int64Array = typed_column_by_name(rb, "payload_size")?;
    let insert_time_column: &TimestampNanosecondArray = typed_column_by_name(rb, "insert_time")?;
    Ok(BlockMetadata {
        block_id: Uuid::parse_str(block_id_column.value(row))?,
        stream_id: Uuid::parse_str(stream_id_column.value(row))?,
        process_id: Uuid::parse_str(process_id_column.value(row))?,
        begin_time: DateTime::from_timestamp_nanos(begin_time_column.value(row)),
        end_time: DateTime::from_timestamp_nanos(end_time_column.value(row)),
        begin_ticks: begin_ticks_column.value(row),
        end_ticks: end_ticks_column.value(row),
        nb_objects: nb_objects_column.value(row),
        object_offset: object_offset_column.value(row),
        payload_size: payload_size_column.value(row),
        insert_time: DateTime::from_timestamp_nanos(insert_time_column.value(row)),
    })
}

/// Finds all blocks for a given stream within a given time range.
#[span_fn]
pub async fn find_stream_blocks_in_range(
    connection: &mut sqlx::PgConnection,
    stream_id: sqlx::types::Uuid,
    begin_ticks: i64,
    end_ticks: i64,
) -> Result<Vec<BlockMetadata>> {
    let rows = sqlx::query(
        "SELECT block_id, stream_id, process_id, begin_time, begin_ticks, end_time, end_ticks, nb_objects, object_offset, payload_size, insert_time
         FROM blocks
         WHERE stream_id = $1
         AND begin_ticks <= $2
         AND end_ticks >= $3
         ORDER BY begin_ticks;",
    )
    .bind(stream_id)
    .bind(end_ticks)
    .bind(begin_ticks)
    .fetch_all(connection)
    .await
    .with_context(|| "find_stream_blocks")?;
    let mut blocks = Vec::new();
    for r in rows {
        blocks.push(block_from_row(&r)?);
    }
    Ok(blocks)
}
