//! analytics : provides read access to the telemetry data lake

// crate-specific lint exceptions:
#![allow(clippy::missing_errors_doc)]

pub mod analytics_service;
pub mod arrow_utils;
pub mod call_tree;
pub mod log_entries_table;
pub mod log_entry;
pub mod measure;
pub mod metadata;
pub mod metrics_table;
pub mod query_log_entries;
pub mod query_metrics;
pub mod query_spans;
pub mod query_thread_events;
pub mod scope;
pub mod span_table;
pub mod sql_arrow_bridge;
pub mod thread_block_processor;
pub mod thread_events_table;
pub mod time;

use anyhow::{Context, Result};
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry::compression::decompress;
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_tracing::prelude::*;
use micromegas_transit::{parse_object_buffer, read_dependencies, Value};
use std::sync::Arc;

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
) -> Result<bool>
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
    let continue_iterating = parse_object_buffer(
        &dependencies,
        obj_udts,
        &decompress(&payload.objects).with_context(|| "decompressing objects payload")?,
        fun,
    )
    .with_context(|| "parsing object buffer")?;
    Ok(continue_iterating)
}

pub mod prelude {
    pub use crate::fetch_block_payload;
    pub use crate::parse_block;
    pub use crate::time::get_process_tick_length_ms;
    pub use crate::time::get_tsc_frequency_inverse_ms;
}
