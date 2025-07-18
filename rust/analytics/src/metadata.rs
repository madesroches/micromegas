use anyhow::{Context, Result};
use micromegas_telemetry::{
    property::Property, stream_info::StreamInfo, types::block::BlockMetadata,
};
use micromegas_tracing::prelude::*;
use micromegas_transit::UserDefinedType;
use sqlx::Row;

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
        insert_time: row.try_get("block_insert_time")?,
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
        "SELECT block_id, stream_id, process_id, begin_time, begin_ticks, end_time, end_ticks, nb_objects, object_offset, payload_size, insert_time as block_insert_time
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
