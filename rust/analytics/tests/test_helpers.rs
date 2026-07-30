//! Shared test scaffolding, imported per-binary via `mod test_helpers;`. Not every test
//! binary uses every helper here, hence the blanket allow instead of per-binary warnings.
#![allow(dead_code)]

use micromegas_analytics::arrow_properties::serialize_properties_to_jsonb;
use micromegas_analytics::lakehouse::partition_source_data::PartitionSourceBlock;
use micromegas_analytics::metadata::{ProcessMetadata, StreamMetadata};
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry::block_wire_format::BlockPayload;
use micromegas_telemetry::types::block::BlockMetadata;
use micromegas_tracing::dispatch::make_process_info;
use std::collections::HashMap;
use std::sync::Arc;

// Helper function to convert ProcessInfo to ProcessMetadata for tests
pub fn make_process_metadata(
    process_id: uuid::Uuid,
    parent_process_id: Option<uuid::Uuid>,
    properties: HashMap<String, String>,
) -> ProcessMetadata {
    let process_info = make_process_info(process_id, parent_process_id, properties.clone());
    let properties_jsonb = serialize_properties_to_jsonb(&properties).unwrap();
    ProcessMetadata {
        process_id: process_info.process_id,
        exe: process_info.exe,
        username: process_info.username,
        realname: process_info.realname,
        computer: process_info.computer,
        distro: process_info.distro,
        cpu_brand: process_info.cpu_brand,
        tsc_frequency: process_info.tsc_frequency,
        start_time: process_info.start_time,
        start_ticks: process_info.start_ticks,
        parent_process_id: process_info.parent_process_id,
        properties: Arc::new(properties_jsonb),
    }
}

/// Builds an in-memory `BlobStorage` with no path prefix, so `blobs/{process_id}/...`
/// paths written directly onto the store are exactly what `fetch_block_payload` reads.
pub fn make_in_memory_blob_storage() -> Arc<BlobStorage> {
    Arc::new(BlobStorage::new(
        Arc::new(object_store::memory::InMemory::new()),
        object_store::path::Path::from(""),
    ))
}

/// Builds a `PartitionSourceBlock` for a single block carrying `payload_bytes` under a
/// fresh random process/stream/block id, and writes the CBOR-wrapped `BlockPayload` to
/// `blob_storage` at the path `fetch_block_payload` expects.
pub async fn make_source_block(
    blob_storage: &BlobStorage,
    payload_bytes: Vec<u8>,
    nb_objects: usize,
    format: &str,
) -> anyhow::Result<Arc<PartitionSourceBlock>> {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let block_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();

    let block_payload = BlockPayload {
        dependencies: vec![],
        objects: payload_bytes,
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&block_payload, &mut buf)?;
    let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");
    blob_storage.put(&obj_path, buf.into()).await?;

    let block = BlockMetadata {
        block_id,
        stream_id,
        process_id,
        begin_time: now,
        end_time: now,
        begin_ticks: 0,
        end_ticks: 0,
        nb_objects: nb_objects as i32,
        payload_size: block_payload.objects.len() as i64,
        object_offset: 0,
        insert_time: now,
    };
    let stream = Arc::new(StreamMetadata {
        process_id,
        stream_id,
        dependencies_metadata: vec![],
        objects_metadata: vec![],
        tags: vec![],
        properties: Arc::new(vec![]),
    });
    let process = Arc::new(make_process_metadata(process_id, None, HashMap::new()));
    Ok(Arc::new(PartitionSourceBlock {
        block,
        stream,
        process,
        format: format.to_string(),
    }))
}
