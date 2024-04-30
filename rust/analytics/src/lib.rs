//! analytics : provides read access to the telemetry data lake

// crate-specific lint exceptions:
#![allow(clippy::missing_errors_doc)]

pub mod analytics_service;
pub mod arrow_utils;
pub mod call_tree;
pub mod log_entry;
pub mod metadata;
pub mod query_spans;
pub mod scope;
pub mod sql_arrow_bridge;
pub mod thread_block_processor;
pub mod time;

use crate::log_entry::LogEntry;
use anyhow::{Context, Result};
use metadata::{map_row_block, process_from_row};
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry::compression::decompress;
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_telemetry::types::block::BlockMetadata;
use micromegas_telemetry::types::process::Process;
use micromegas_tracing::prelude::*;
use micromegas_transit::{parse_object_buffer, read_dependencies, UserDefinedType, Value};
use sqlx::Row;
use std::sync::Arc;
use time::ConvertTicks;

#[span_fn]
pub async fn processes_by_name_substring(
    connection: &mut sqlx::PgConnection,
    filter: &str,
) -> Result<Vec<Process>> {
    let mut processes = Vec::new();
    let rows = sqlx::query(
        "SELECT process_id, exe, username, realname, computer, distro, cpu_brand, tsc_frequency, start_time, start_ticks, parent_process_id
         FROM processes
         WHERE exe LIKE ?
         ORDER BY start_time DESC
         LIMIT 100;",
    )
    .bind( format!("%{}%", filter) )
    .fetch_all(connection)
    .await?;
    for r in rows {
        processes.push(process_from_row(&r)?);
    }
    Ok(processes)
}

#[span_fn]
pub async fn find_block_process(
    connection: &mut sqlx::PgConnection,
    block_id: &str,
) -> Result<Process> {
    let row = sqlx::query(
        "SELECT processes.process_id AS process_id, exe, username, realname, computer, distro, cpu_brand, tsc_frequency, start_time, start_ticks, parent_process_id
         FROM processes, streams, blocks
         WHERE blocks.block_id = ?
         AND blocks.stream_id = streams.stream_id
         AND processes.process_id = streams.process_id;"
    )
    .bind(block_id)
    .fetch_one(connection)
    .await?;
    process_from_row(&row)
}

#[span_fn]
pub async fn list_recent_processes(
    connection: &mut sqlx::PgConnection,
    parent_process_id: Option<&str>,
) -> Result<Vec<Process>> {
    let mut processes = Vec::new();
    // like ?!
    let rows = sqlx::query(
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
         WHERE parent_process_id like ?
         ORDER BY start_time DESC
         LIMIT 100;",
    )
    .bind(match parent_process_id {
        Some(str) => str.to_string(),
        None => "''".to_string(),
    })
    .fetch_all(connection)
    .await?;
    for r in rows {
        processes.push(process_from_row(&r)?);
    }
    Ok(processes)
}

#[span_fn]
pub async fn search_processes(
    connection: &mut sqlx::PgConnection,
    keyword: &str,
) -> Result<Vec<Process>> {
    let mut processes = Vec::new();
    let rows = sqlx::query(
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
         WHERE exe LIKE ?
         OR username LIKE ?
         OR computer LIKE ?
         ORDER BY start_time DESC
         LIMIT 100;",
    )
    .bind(format!("%{}%", keyword))
    .bind(format!("%{}%", keyword))
    .bind(format!("%{}%", keyword))
    .fetch_all(connection)
    .await?;
    for r in rows {
        processes.push(process_from_row(&r)?);
    }
    Ok(processes)
}

#[span_fn]
pub async fn fetch_child_processes(
    connection: &mut sqlx::PgConnection,
    parent_process_id: &str,
) -> Result<Vec<Process>> {
    let mut processes = Vec::new();
    let rows = sqlx::query(
        "SELECT process_id, exe, username, realname, computer, distro, cpu_brand, tsc_frequency, start_time, start_ticks, parent_process_id
         FROM processes
         WHERE parent_process_id = ?
         ORDER BY start_time DESC
         ;",
    )
    .bind(parent_process_id)
    .fetch_all(connection)
    .await?;
    for r in rows {
        processes.push(process_from_row(&r)?);
    }
    Ok(processes)
}

#[span_fn]
pub async fn find_process_streams_tagged(
    connection: &mut sqlx::PgConnection,
    process_id: &str,
    tag: &str,
) -> Result<Vec<StreamInfo>> {
    let rows = sqlx::query(
        "SELECT stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties
         FROM streams
         WHERE tags LIKE ?
         AND process_id = ?
         ;",
    )
    .bind(format!("%{}%", tag))
    .bind(process_id)
    .fetch_all(connection)
    .await
    .with_context(|| "fetch_all in find_process_streams_tagged")?;
    let mut res = Vec::new();
    for r in rows {
        let stream_id: String = r.get("stream_id");
        let dependencies_metadata_buffer: Vec<u8> = r.get("dependencies_metadata");
        let dependencies_metadata: Vec<UserDefinedType> =
            ciborium::from_reader(&dependencies_metadata_buffer[..])
                .with_context(|| "decoding dependencies metadata")?;
        let objects_metadata_buffer: Vec<u8> = r.get("objects_metadata");
        let objects_metadata: Vec<UserDefinedType> =
            ciborium::from_reader(&objects_metadata_buffer[..])
                .with_context(|| "decoding objects metadata")?;
        let tags: Vec<String> = r.get("tags");
        let properties_str: String = r.get("properties");
        let properties: std::collections::HashMap<String, String> =
            serde_json::from_str(&properties_str).unwrap();
        res.push(StreamInfo {
            stream_id,
            process_id: r.get("process_id"),
            dependencies_metadata,
            objects_metadata,
            tags,
            properties,
        });
    }
    Ok(res)
}

#[span_fn]
pub async fn find_process_streams(
    connection: &mut sqlx::PgConnection,
    process_id: &str,
) -> Result<Vec<StreamInfo>> {
    let rows = sqlx::query(
        "SELECT stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties
         FROM streams
         WHERE process_id = ?
         ;",
    )
    .bind(process_id)
    .fetch_all(connection)
    .await
    .with_context(|| "fetch_all in find_process_streams")?;
    let mut res = Vec::new();
    for r in rows {
        let stream_id: String = r.get("stream_id");
        let dependencies_metadata_buffer: Vec<u8> = r.get("dependencies_metadata");
        let dependencies_metadata: Vec<UserDefinedType> =
            ciborium::from_reader(&dependencies_metadata_buffer[..])
                .with_context(|| "decoding dependencies metadata")?;
        let objects_metadata_buffer: Vec<u8> = r.get("objects_metadata");
        let objects_metadata: Vec<UserDefinedType> =
            ciborium::from_reader(&objects_metadata_buffer[..])
                .with_context(|| "decoding objects metadata")?;
        let tags: Vec<String> = r.get("tags");
        let properties_str: String = r.get("properties");
        let properties: std::collections::HashMap<String, String> =
            serde_json::from_str(&properties_str).unwrap();
        res.push(StreamInfo {
            stream_id,
            process_id: r.get("process_id"),
            dependencies_metadata,
            objects_metadata,
            tags,
            properties,
        });
    }
    Ok(res)
}

#[span_fn]
pub async fn find_process_blocks(
    connection: &mut sqlx::PgConnection,
    process_id: &str,
    tag: &str,
) -> Result<Vec<BlockMetadata>> {
    let rows = sqlx::query(
        "SELECT B.*
        FROM streams S
        LEFT JOIN blocks B
        ON S.stream_id = B.stream_id
        WHERE S.process_id = ?  
        AND S.tags like ?
        AND B.block_id IS NOT NULL",
    )
    .bind(process_id)
    .bind(format!("%{}%", tag))
    .fetch_all(connection)
    .await
    .with_context(|| "find_process_blocks")?;
    let mut blocks = Vec::new();
    for r in rows {
        blocks.push(map_row_block(&r)?);
    }
    Ok(blocks)
}

#[span_fn]
pub async fn find_process_log_streams(
    connection: &mut sqlx::PgConnection,
    process_id: &str,
) -> Result<Vec<StreamInfo>> {
    find_process_streams_tagged(connection, process_id, "log").await
}

#[span_fn]
pub async fn find_process_thread_streams(
    connection: &mut sqlx::PgConnection,
    process_id: &str,
) -> Result<Vec<StreamInfo>> {
    find_process_streams_tagged(connection, process_id, "cpu").await
}

#[span_fn]
pub async fn find_process_metrics_streams(
    connection: &mut sqlx::PgConnection,
    process_id: &str,
) -> Result<Vec<StreamInfo>> {
    find_process_streams_tagged(connection, process_id, "metrics").await
}

#[span_fn]
pub async fn find_block_stream(
    connection: &mut sqlx::PgConnection,
    block_id: &str,
) -> Result<StreamInfo> {
    let row = sqlx::query(
        "SELECT streams.stream_id as stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties
         FROM streams, blocks
         WHERE streams.stream_id = blocks.stream_id
         AND   blocks.block_id = ?
         ;",
    )
    .bind(block_id)
    .fetch_one(connection)
    .await
    .with_context(|| "find_block_stream")?;
    let dependencies_metadata_buffer: Vec<u8> = row.get("dependencies_metadata");
    let dependencies_metadata: Vec<UserDefinedType> =
        ciborium::from_reader(&dependencies_metadata_buffer[..])
            .with_context(|| "decoding dependencies metadata")?;
    let objects_metadata_buffer: Vec<u8> = row.get("objects_metadata");
    let objects_metadata: Vec<UserDefinedType> =
        ciborium::from_reader(&objects_metadata_buffer[..])
            .with_context(|| "decoding objects metadata")?;
    let tags: Vec<String> = row.get("tags");
    let properties_str: String = row.get("properties");
    let properties: std::collections::HashMap<String, String> =
        serde_json::from_str(&properties_str).unwrap();
    Ok(StreamInfo {
        stream_id: row.get("stream_id"),
        process_id: row.get("process_id"),
        dependencies_metadata,
        objects_metadata,
        tags,
        properties,
    })
}

#[span_fn]
pub async fn find_block(
    connection: &mut sqlx::PgConnection,
    block_id: &str,
) -> Result<BlockMetadata> {
    let row = sqlx::query(
        "SELECT block_id, stream_id, begin_time, begin_ticks, end_time, end_ticks, nb_objects, payload_size
         FROM blocks
         WHERE block_id = ?
         ;",
    )
    .bind(block_id)
    .fetch_one(connection)
    .await
    .with_context(|| "find_block")?;
    map_row_block(&row)
}

#[span_fn]
pub async fn find_stream_blocks(
    connection: &mut sqlx::PgConnection,
    stream_id: &str,
) -> Result<Vec<BlockMetadata>> {
    let rows = sqlx::query(
        "SELECT block_id, stream_id, begin_time, begin_ticks, end_time, end_ticks, nb_objects, payload_size
         FROM blocks
         WHERE stream_id = ?
         ORDER BY begin_time;",
    )
    .bind(stream_id)
    .fetch_all(connection)
    .await
        .with_context(|| "find_stream_blocks")?;
    let mut blocks = Vec::new();
    for r in rows {
        blocks.push(map_row_block(&r)?);
    }
    Ok(blocks)
}

#[span_fn]
pub async fn fetch_block_payload(
    blob_storage: Arc<BlobStorage>,
    process_id: &str,
    stream_id: &str,
    block_id: &str,
) -> Result<micromegas_telemetry::block_wire_format::BlockPayload> {
    let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");
    let buffer: Vec<u8> = blob_storage
        .read_blob(&obj_path)
        .await
        .with_context(|| "reading block payload from blob storage")?
        .into();
    {
        span_scope!("decode");
        let payload: micromegas_telemetry::block_wire_format::BlockPayload =
            ciborium::from_reader(&buffer[..])
                .with_context(|| format!("reading payload {}", &block_id))?;
        Ok(payload)
    }
}

// parse_block calls fun for each object in the block until fun returns `false`
#[span_fn]
pub fn parse_block<F>(
    stream: &StreamInfo,
    payload: &micromegas_telemetry::block_wire_format::BlockPayload,
    fun: F,
) -> Result<()>
where
    F: FnMut(Value) -> Result<bool>,
{
    let dep_udts = &stream.dependencies_metadata;
    let dependencies = read_dependencies(
        dep_udts,
        &decompress(&payload.dependencies).with_context(|| "decompressing dependencies payload")?,
    )
    .with_context(|| "reading dependencies")?;
    let obj_udts = &stream.objects_metadata;
    parse_object_buffer(
        &dependencies,
        obj_udts,
        &decompress(&payload.objects).with_context(|| "decompressing objects payload")?,
        fun,
    )
    .with_context(|| "parsing object buffer")?;
    Ok(())
}

#[span_fn]
pub fn log_entry_from_value(convert_ticks: &ConvertTicks, val: &Value) -> Result<Option<LogEntry>> {
    if let Value::Object(obj) = val {
        match obj.type_name.as_str() {
            "LogStaticStrEvent" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| "reading time from LogStaticStrEvent")?;
                let desc = obj
                    .get::<Arc<micromegas_transit::Object>>("desc")
                    .with_context(|| "reading desc from LogStaticStrEvent")?;
                let level = Level::from_value(
                    desc.get::<u32>("level")
                        .with_context(|| "reading level from LogStaticStrEvent")?,
                )
                .with_context(|| "converting level to Level enum")?;
                let target = desc
                    .get::<Arc<String>>("target")
                    .with_context(|| "reading target from LogStaticStrEvent")?;
                let msg = desc
                    .get::<Arc<String>>("fmt_str")
                    .with_context(|| "reading fmt_str from LogStaticStrEvent")?;
                Ok(Some(LogEntry {
                    time_ms: convert_ticks.get_time(ticks),
                    level: (level as i32) - 1,
                    target: target.as_str().to_string(),
                    msg: msg.as_str().to_string(),
                }))
            }
            "LogStringEvent" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| "reading time from LogStringEvent")?;
                let desc = obj
                    .get::<Arc<micromegas_transit::Object>>("desc")
                    .with_context(|| "reading desc from LogStringEvent")?;
                let level = Level::from_value(
                    desc.get::<u32>("level")
                        .with_context(|| "reading level from LogStringEvent")?,
                )
                .with_context(|| "converting level to Level enum")?;
                let target = desc
                    .get::<Arc<String>>("target")
                    .with_context(|| "reading target from LogStringEvent")?;
                let msg = obj
                    .get::<Arc<String>>("msg")
                    .with_context(|| "reading msg from LogStringEvent")?;
                Ok(Some(LogEntry {
                    time_ms: convert_ticks.get_time(ticks),
                    level: (level as i32) - 1,
                    target: target.as_str().to_string(),
                    msg: msg.as_str().to_string(),
                }))
            }
            "LogStaticStrInteropEvent" | "LogStringInteropEventV2" | "LogStringInteropEventV3" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| format!("reading time from {}", obj.type_name.as_str()))?;
                let level =
                    Level::from_value(obj.get::<u32>("level").with_context(|| {
                        format!("reading level from {}", obj.type_name.as_str())
                    })?)
                    .with_context(|| "converting level to Level enum")?;
                let target = obj
                    .get::<Arc<String>>("target")
                    .with_context(|| format!("reading target from {}", obj.type_name.as_str()))?;
                let msg = obj
                    .get::<Arc<String>>("msg")
                    .with_context(|| format!("reading msg from {}", obj.type_name.as_str()))?;
                Ok(Some(LogEntry {
                    time_ms: convert_ticks.get_time(ticks),
                    level: (level as i32) - 1,
                    target: target.as_str().to_string(),
                    msg: msg.as_str().to_string(),
                }))
            }
            _ => {
                warn!("unknown log event {:?}", obj);
                Ok(None)
            }
        }
    } else {
        Ok(None)
    }
}

// find_process_log_entry calls pred(time_ticks,entry_str) with each log entry
// until pred returns Some(x)
pub async fn find_process_log_entry<Res, Predicate: FnMut(LogEntry) -> Option<Res>>(
    connection: &mut sqlx::PgConnection,
    blob_storage: Arc<BlobStorage>,
    process: &Process,
    mut pred: Predicate,
) -> Result<Option<Res>> {
    let mut found_entry = None;
    let convert_ticks = ConvertTicks::new(process);
    for stream in find_process_log_streams(connection, &process.process_id).await? {
        for b in find_stream_blocks(connection, &stream.stream_id).await? {
            let payload = fetch_block_payload(
                blob_storage.clone(),
                &stream.process_id,
                &stream.stream_id,
                &b.block_id,
            )
            .await?;
            parse_block(&stream, &payload, |val| {
                if let Some(log_entry) = log_entry_from_value(&convert_ticks, &val)
                    .with_context(|| "log_entry_from_value")?
                {
                    if let Some(x) = pred(log_entry) {
                        found_entry = Some(x);
                        return Ok(false); //do not continue
                    }
                }
                Ok(true) //continue
            })?;
            if found_entry.is_some() {
                return Ok(found_entry);
            }
        }
    }
    Ok(found_entry)
}

// for_each_log_entry_in_block calls fun(time_ticks,entry_str) with each log
// entry until fun returns false mad
#[span_fn]
pub async fn for_each_log_entry_in_block<Predicate: FnMut(LogEntry) -> bool>(
    blob_storage: Arc<BlobStorage>,
    convert_ticks: &ConvertTicks,
    stream: &StreamInfo,
    block: &BlockMetadata,
    mut fun: Predicate,
) -> Result<()> {
    let payload = fetch_block_payload(
        blob_storage,
        &stream.process_id,
        &stream.stream_id,
        &block.block_id,
    )
    .await?;
    parse_block(stream, &payload, |val| {
        if let Some(log_entry) =
            log_entry_from_value(convert_ticks, &val).with_context(|| "log_entry_from_value")?
        {
            if !fun(log_entry) {
                return Ok(false); //do not continue
            }
        }
        Ok(true) //continue
    })
    .with_context(|| "error in parse_block")?;
    Ok(())
}

#[span_fn]
pub async fn for_each_process_log_entry<ProcessLogEntry: FnMut(LogEntry)>(
    connection: &mut sqlx::PgConnection,
    blob_storage: Arc<BlobStorage>,
    process: &Process,
    mut process_log_entry: ProcessLogEntry,
) -> Result<()> {
    find_process_log_entry(connection, blob_storage, process, |log_entry| {
        process_log_entry(log_entry);
        let nothing: Option<()> = None;
        nothing //continue searching
    })
    .await?;
    Ok(())
}

#[span_fn]
pub async fn for_each_process_metric<ProcessMetric: FnMut(Arc<micromegas_transit::Object>)>(
    connection: &mut sqlx::PgConnection,
    blob_storage: Arc<BlobStorage>,
    process_id: &str,
    mut process_metric: ProcessMetric,
) -> Result<()> {
    for stream in find_process_metrics_streams(connection, process_id).await? {
        for block in find_stream_blocks(connection, &stream.stream_id).await? {
            let payload = fetch_block_payload(
                blob_storage.clone(),
                &stream.process_id,
                &stream.stream_id,
                &block.block_id,
            )
            .await?;
            parse_block(&stream, &payload, |val| {
                if let Value::Object(obj) = val {
                    process_metric(obj);
                }
                Ok(true) //continue
            })?;
        }
    }
    Ok(())
}

#[async_recursion::async_recursion]
#[span_fn]
pub async fn for_each_process_in_tree<F>(
    pool: &sqlx::PgPool,
    root: &Process,
    rec_level: u16,
    fun: F,
) -> Result<()>
where
    F: Fn(&Process, u16) + std::marker::Send + Clone,
{
    fun(root, rec_level);
    let mut connection = pool.acquire().await?;
    for child_info in fetch_child_processes(&mut connection, &root.process_id)
        .await
        .unwrap()
    {
        let fun_clone = fun.clone();
        for_each_process_in_tree(pool, &child_info, rec_level + 1, fun_clone).await?;
    }
    Ok(())
}

pub mod prelude {
    pub use crate::fetch_block_payload;
    pub use crate::fetch_child_processes;
    pub use crate::find_block;
    pub use crate::find_block_process;
    pub use crate::find_block_stream;
    pub use crate::find_process_blocks;
    pub use crate::find_process_log_entry;
    pub use crate::find_process_log_streams;
    pub use crate::find_process_metrics_streams;
    pub use crate::find_process_streams;
    pub use crate::find_process_thread_streams;
    pub use crate::find_stream_blocks;
    pub use crate::for_each_log_entry_in_block;
    pub use crate::for_each_process_in_tree;
    pub use crate::for_each_process_log_entry;
    pub use crate::for_each_process_metric;
    pub use crate::list_recent_processes;
    pub use crate::parse_block;
    pub use crate::processes_by_name_substring;
    pub use crate::search_processes;
    pub use crate::time::get_process_tick_length_ms;
    pub use crate::time::get_tsc_frequency_inverse_ms;
}
