use anyhow::{Context, Result};
use micromegas_ingestion::sql_property;
use micromegas_telemetry::{stream_info::StreamInfo, types::process::Process};
use micromegas_tracing::prelude::*;
use micromegas_transit::UserDefinedType;
use sqlx::Row;

#[span_fn]
pub async fn find_stream(
    connection: &mut sqlx::PgConnection,
    stream_id: &str,
) -> Result<StreamInfo> {
    let row = sqlx::query(
        "SELECT process_id, dependencies_metadata, objects_metadata, tags, properties
         FROM streams
         WHERE stream_id = $1
         ;",
    )
    .bind(stream_id)
    .fetch_one(connection)
    .await
    .with_context(|| "select from streams")?;
    let dependencies_metadata_buffer: Vec<u8> = row.try_get("dependencies_metadata")?;
    let dependencies_metadata: Vec<UserDefinedType> =
        ciborium::from_reader(&dependencies_metadata_buffer[..])
            .with_context(|| "decoding dependencies metadata")?;
    let objects_metadata_buffer: Vec<u8> = row.try_get("objects_metadata")?;
    let objects_metadata: Vec<UserDefinedType> =
        ciborium::from_reader(&objects_metadata_buffer[..])
            .with_context(|| "decoding objects metadata")?;
    let tags: Vec<String> = row.try_get("tags")?;
    let properties: Vec<sql_property::Property> = row.try_get("properties")?;
    Ok(StreamInfo {
        stream_id: String::from(stream_id),
        process_id: row.try_get("process_id")?,
        dependencies_metadata,
        objects_metadata,
        tags,
        properties: sql_property::into_hashmap(properties),
    })
}

#[span_fn]
pub fn process_from_row(row: &sqlx::postgres::PgRow) -> Result<Process> {
    Ok(Process {
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
    })
}

#[span_fn]
pub async fn find_process(
    connection: &mut sqlx::PgConnection,
    process_id: &str,
) -> Result<Process> {
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
                parent_process_id
         FROM processes
         WHERE process_id = $1;",
    )
    .bind(process_id)
    .fetch_one(connection)
    .await
    .with_context(|| "select from processes")?;
    process_from_row(&row)
}
