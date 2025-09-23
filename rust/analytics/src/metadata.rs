use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use datafusion::arrow::array::{Int32Array, Int64Array, RecordBatch, TimestampNanosecondArray};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::{
    property::Property, stream_info::StreamInfo, types::block::BlockMetadata,
};
use micromegas_tracing::prelude::*;
use micromegas_transit::{UserDefinedType, uuid_utils::parse_optional_uuid};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    arrow_properties::serialize_properties_to_jsonb,
    dfext::{string_column_accessor::string_column_by_name, typed_column::typed_column_by_name},
    lakehouse::{
        partition_cache::LivePartitionProvider, query::make_session_context,
        view_factory::ViewFactory,
    },
    properties::properties_column_accessor::properties_column_by_name,
    time::TimeRange,
};
use datafusion::execution::runtime_env::RuntimeEnv;
/// Type alias for shared, pre-serialized JSONB data.
/// This represents JSONB properties that have been serialized once and can be reused.
pub type SharedJsonbSerialized = Arc<Vec<u8>>;

/// Analytics-optimized process metadata.
///
/// This struct is designed for analytics use cases where process properties need to be
/// efficiently serialized to JSONB format multiple times. Unlike `ProcessInfo`, which
/// uses `HashMap<String, String>` for properties, this struct stores pre-serialized
/// JSONB data to eliminate redundant serialization overhead.
#[derive(Debug, Clone)]
pub struct ProcessMetadata {
    // Core fields (same as ProcessInfo)
    pub process_id: uuid::Uuid,
    pub exe: String,
    pub username: String,
    pub realname: String,
    pub computer: String,
    pub distro: String,
    pub cpu_brand: String,
    pub tsc_frequency: i64,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub start_ticks: i64,
    pub parent_process_id: Option<uuid::Uuid>,
    pub properties: SharedJsonbSerialized,
}

/// Analytics-optimized stream metadata.
///
/// This struct is designed for analytics use cases where stream properties need to be
/// efficiently serialized to JSONB format multiple times. Unlike `StreamInfo`, which
/// uses `HashMap<String, String>` for properties, this struct stores pre-serialized
/// JSONB data to eliminate redundant serialization overhead.
#[derive(Debug, Clone)]
pub struct StreamMetadata {
    // Core fields (same as StreamInfo)
    pub process_id: Uuid,
    pub stream_id: Uuid,
    pub dependencies_metadata: Vec<UserDefinedType>,
    pub objects_metadata: Vec<UserDefinedType>,
    pub tags: Vec<String>,
    pub properties: SharedJsonbSerialized,
}

impl StreamMetadata {
    /// Creates StreamMetadata from StreamInfo by converting properties to JSONB format.
    pub fn from_stream_info(stream_info: &StreamInfo) -> Result<Self> {
        let properties = serialize_properties_to_jsonb(&stream_info.properties)
            .with_context(|| "serializing stream properties to JSONB")?;
        Ok(Self {
            process_id: stream_info.process_id,
            stream_id: stream_info.stream_id,
            dependencies_metadata: stream_info.dependencies_metadata.clone(),
            objects_metadata: stream_info.objects_metadata.clone(),
            tags: stream_info.tags.clone(),
            properties: Arc::new(properties),
        })
    }
}

/// Returns the thread name associated with the stream, if available.
/// This function is only meaningful for streams associated with CPU threads.
pub fn get_thread_name_from_stream_metadata(stream: &StreamMetadata) -> Result<String> {
    use jsonb::RawJsonb;

    const THREAD_NAME_KEY: &str = "thread-name";
    const THREAD_ID_KEY: &str = "thread-id";

    if stream.properties.is_empty() {
        return Ok(format!("{}", &stream.stream_id));
    }

    let jsonb = RawJsonb::new(&stream.properties);

    // Try to get thread-name first
    if let Ok(Some(thread_name_value)) = jsonb.get_by_name(THREAD_NAME_KEY, false)
        && let Ok(Some(thread_name)) = thread_name_value.as_raw().as_str()
    {
        return Ok(thread_name.to_string());
    }

    // Fall back to thread-id
    if let Ok(Some(thread_id_value)) = jsonb.get_by_name(THREAD_ID_KEY, false)
        && let Ok(Some(thread_id)) = thread_id_value.as_raw().as_str()
    {
        return Ok(thread_id.to_string());
    }

    // If neither property exists, use stream_id
    Ok(format!("{}", &stream.stream_id))
}

/// Creates a `StreamMetadata` from a database row with pre-serialized JSONB properties.
#[span_fn]
pub fn stream_metadata_from_row(row: &sqlx::postgres::PgRow) -> Result<StreamMetadata> {
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
    let properties_map = micromegas_telemetry::property::into_hashmap(properties);
    let serialized_properties = serialize_properties_to_jsonb(&properties_map)
        .with_context(|| "serializing stream properties to JSONB")?;

    Ok(StreamMetadata {
        stream_id: row.try_get("stream_id")?,
        process_id: row.try_get("process_id")?,
        dependencies_metadata,
        objects_metadata,
        tags,
        properties: Arc::new(serialized_properties),
    })
}

/// Finds a stream by its ID and returns it as StreamMetadata.
#[span_fn]
pub async fn find_stream(
    pool: &sqlx::Pool<sqlx::Postgres>,
    stream_id: sqlx::types::Uuid,
) -> Result<StreamMetadata> {
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
    stream_metadata_from_row(&row)
}

/// Creates a `ProcessMetadata` from a database row with pre-serialized JSONB properties.
#[span_fn]
pub fn process_metadata_from_row(row: &sqlx::postgres::PgRow) -> Result<ProcessMetadata> {
    let properties: Vec<Property> = row.try_get("process_properties")?;
    let properties_map = micromegas_telemetry::property::into_hashmap(properties);
    let serialized_properties = serialize_properties_to_jsonb(&properties_map)
        .with_context(|| "serializing process properties to JSONB")?;

    Ok(ProcessMetadata {
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
        properties: Arc::new(serialized_properties),
    })
}

/// Finds a process by its ID and returns it as ProcessMetadata with pre-serialized JSONB properties.
#[span_fn]
pub async fn find_process(
    pool: &sqlx::Pool<sqlx::Postgres>,
    process_id: &sqlx::types::Uuid,
) -> Result<ProcessMetadata> {
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
    process_metadata_from_row(&row)
}

/// Finds a process and its latest timing information using DataFusion (optimized version).
/// Returns (ProcessMetadata, last_block_end_ticks, last_block_end_time)
#[span_fn]
pub async fn find_process_with_latest_timing(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
    process_id: &Uuid,
    query_range: Option<TimeRange>,
) -> Result<(ProcessMetadata, i64, DateTime<Utc>)> {
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

    let df = ctx
        .sql(&sql)
        .await
        .with_context(|| "executing SQL query for process with timing")?;

    let results = df
        .collect()
        .await
        .with_context(|| "collecting results from DataFusion")?;

    if results.is_empty() || results[0].num_rows() == 0 {
        anyhow::bail!("Process not found");
    }

    let batch = &results[0];

    // Extract all the required columns
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

    let parent_process_id = if parent_process_id_column.is_null(0) {
        None
    } else {
        parse_optional_uuid(parent_process_id_column.value(0))?
    };

    // Handle properties column using PropertiesColumnAccessor
    let properties_accessor = properties_column_by_name(batch, "properties")
        .with_context(|| "accessing properties column")?;

    // Get JSONB bytes directly from the properties column
    let properties_jsonb = Arc::new(
        properties_accessor
            .jsonb_value(0)
            .with_context(|| "extracting JSONB from properties column")?,
    );

    let process_metadata = ProcessMetadata {
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
        properties: properties_jsonb,
    };

    let last_block_end_ticks = last_block_end_ticks_column.value(0);
    let last_block_end_time = DateTime::from_timestamp_nanos(last_block_end_time_column.value(0));

    Ok((process_metadata, last_block_end_ticks, last_block_end_time))
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
