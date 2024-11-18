use anyhow::{Context, Result};
use micromegas_telemetry::{
    blob_storage::BlobStorage, compression::decompress, stream_info::StreamInfo,
};
use micromegas_tracing::prelude::*;
use micromegas_transit::{parse_object_buffer, read_dependencies, value::Value};
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
