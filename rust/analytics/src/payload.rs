use anyhow::{Context, Result};
use bumpalo::Bump;
use micromegas_telemetry::{blob_storage::BlobStorage, compression::decompress};
use micromegas_tracing::{parsing::make_custom_readers, prelude::*};
use micromegas_transit::{CustomReaderMap, parse_object_buffer, read_dependencies, value::Value};
use std::sync::Arc;

use crate::metadata::StreamMetadata;

thread_local! {
    /// The custom-reader map is identical for every block, so build it once per
    /// worker thread instead of rebuilding a `HashMap` of `Arc<dyn Fn>` on every
    /// `parse_block` call.
    static CUSTOM_READERS: CustomReaderMap = make_custom_readers();
}

/// Fetches the payload of a block from blob storage.
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

/// Parses a block of telemetry data, calling a function for each object in the block.
///
/// Each parsed `Value` borrows from a per-block bump arena (and the decompressed
/// buffers) that live only for the duration of this call. The higher-ranked
/// `FnMut(Value<'_>)` bound forbids the callback from retaining a `Value` beyond
/// its invocation — anything that must outlive the block (e.g. an Arrow append)
/// must copy out inside the callback.
// parse_block calls fun for each object in the block until fun returns `false`
#[span_fn]
pub fn parse_block<F>(
    stream: &StreamMetadata,
    payload: &micromegas_telemetry::block_wire_format::BlockPayload,
    mut fun: F,
) -> Result<bool>
where
    F: for<'a> FnMut(Value<'a>) -> Result<bool>,
{
    let dep_udts = &stream.dependencies_metadata;
    let obj_udts = &stream.objects_metadata;
    // Bind the decompressed buffers and the arena to locals so every parsed
    // Value borrows from storage that outlives the parse below.
    let deps_buf =
        decompress(&payload.dependencies).with_context(|| "decompressing dependencies payload")?;
    let objs_buf = decompress(&payload.objects).with_context(|| "decompressing objects payload")?;
    let bump = Bump::new();
    CUSTOM_READERS.with(|custom_readers| {
        let dependencies = read_dependencies(&bump, custom_readers, dep_udts, &deps_buf)
            .with_context(|| "reading dependencies")?;
        let continue_iterating = parse_object_buffer(
            &bump,
            custom_readers,
            &dependencies,
            obj_udts,
            &objs_buf,
            &mut fun,
        )
        .with_context(|| "parsing object buffer")?;
        Ok(continue_iterating)
    })
}
