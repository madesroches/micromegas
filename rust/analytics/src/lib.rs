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
pub mod span_table;
pub mod sql_arrow_bridge;
pub mod thread_block_processor;
pub mod time;
pub mod query_log_entries;
pub mod log_entries_table;

use anyhow::{Context, Result};
use metadata::{map_row_block, process_from_row};
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry::compression::decompress;
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_telemetry::types::block::BlockMetadata;
use micromegas_tracing::prelude::*;
use micromegas_transit::{parse_object_buffer, read_dependencies, UserDefinedType, Value};
use sqlx::Row;
use std::sync::Arc;

#[span_fn]
pub async fn processes_by_name_substring(
    connection: &mut sqlx::PgConnection,
    filter: &str,
) -> Result<Vec<ProcessInfo>> {
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
) -> Result<ProcessInfo> {
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
) -> Result<Vec<ProcessInfo>> {
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
) -> Result<Vec<ProcessInfo>> {
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
    parent_process_id: &sqlx::types::Uuid,
) -> Result<Vec<ProcessInfo>> {
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
    process_id: &sqlx::types::Uuid,
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
        let stream_id: sqlx::types::Uuid = r.get("stream_id");
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
        let stream_id: sqlx::types::Uuid = r.get("stream_id");
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
    process_id: &sqlx::types::Uuid,
) -> Result<Vec<StreamInfo>> {
    find_process_streams_tagged(connection, process_id, "log").await
}

#[span_fn]
pub async fn find_process_thread_streams(
    connection: &mut sqlx::PgConnection,
    process_id: &sqlx::types::Uuid,
) -> Result<Vec<StreamInfo>> {
    find_process_streams_tagged(connection, process_id, "cpu").await
}

#[span_fn]
pub async fn find_process_metrics_streams(
    connection: &mut sqlx::PgConnection,
    process_id: &sqlx::types::Uuid,
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
    stream_id: &sqlx::types::Uuid,
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
    process_id: sqlx::types::Uuid,
    stream_id: sqlx::types::Uuid,
    block_id: sqlx::types::Uuid,
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
pub async fn for_each_process_metric<ProcessMetric: FnMut(Arc<micromegas_transit::Object>)>(
    connection: &mut sqlx::PgConnection,
    blob_storage: Arc<BlobStorage>,
    process_id: &sqlx::types::Uuid,
    mut process_metric: ProcessMetric,
) -> Result<()> {
    for stream in find_process_metrics_streams(connection, process_id).await? {
        for block in find_stream_blocks(connection, &stream.stream_id).await? {
            let payload = fetch_block_payload(
                blob_storage.clone(),
                stream.process_id,
                stream.stream_id,
                block.block_id,
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
    root: &ProcessInfo,
    rec_level: u16,
    fun: F,
) -> Result<()>
where
    F: Fn(&ProcessInfo, u16) + std::marker::Send + Clone,
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
    pub use crate::find_process_log_streams;
    pub use crate::find_process_metrics_streams;
    pub use crate::find_process_streams;
    pub use crate::find_process_thread_streams;
    pub use crate::find_stream_blocks;
    pub use crate::for_each_process_in_tree;
    pub use crate::for_each_process_metric;
    pub use crate::list_recent_processes;
    pub use crate::parse_block;
    pub use crate::processes_by_name_substring;
    pub use crate::search_processes;
    pub use crate::time::get_process_tick_length_ms;
    pub use crate::time::get_tsc_frequency_inverse_ms;
}
