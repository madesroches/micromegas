use crate::metadata::StreamMetadata;
use crate::payload::{fetch_block_payload, parse_block};
use anyhow::{Context, Result};
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry::block_wire_format::BlockPayload;
use micromegas_tracing::prelude::*;
use micromegas_transit::value::{Object, Value};
use std::sync::Arc;

/// A trait for processing network tracing event blocks.
///
/// Implementors receive one callback per decoded net event. Returning `Ok(true)`
/// continues iteration; returning `Ok(false)` stops parsing the current block.
pub trait NetBlockProcessor {
    fn on_connection_begin(
        &mut self,
        event_id: i64,
        time: i64,
        connection_name: Arc<String>,
        is_outgoing: bool,
    ) -> Result<bool>;

    fn on_connection_end(&mut self, event_id: i64, time: i64, bit_size: i64) -> Result<bool>;

    fn on_object_begin(
        &mut self,
        event_id: i64,
        time: i64,
        object_name: Arc<String>,
    ) -> Result<bool>;

    fn on_object_end(&mut self, event_id: i64, time: i64, bit_size: i64) -> Result<bool>;

    fn on_property(
        &mut self,
        event_id: i64,
        time: i64,
        property_name: Arc<String>,
        bit_size: i64,
    ) -> Result<bool>;

    fn on_rpc_begin(
        &mut self,
        event_id: i64,
        time: i64,
        function_name: Arc<String>,
    ) -> Result<bool>;

    fn on_rpc_end(&mut self, event_id: i64, time: i64, bit_size: i64) -> Result<bool>;
}

fn read_time(obj: &Object) -> Result<i64> {
    obj.get::<i64>("time")
}

fn read_bit_size(obj: &Object) -> Result<i64> {
    Ok(obj.get::<u32>("bit_size")? as i64)
}

/// Parses a net event block payload and calls the appropriate processor callback for each event.
#[span_fn]
pub fn parse_net_block_payload<Proc: NetBlockProcessor>(
    object_offset: i64,
    payload: &BlockPayload,
    stream: &StreamMetadata,
    processor: &mut Proc,
) -> Result<bool> {
    let mut event_id = object_offset;
    parse_block(stream, payload, |val| {
        let res = if let Value::Object(obj) = val {
            match obj.type_name.as_str() {
                "NetConnectionBeginEvent" => {
                    let time = read_time(&obj).with_context(|| "NetConnectionBeginEvent.time")?;
                    let connection_name = obj
                        .get::<Arc<String>>("connection_name")
                        .with_context(|| "NetConnectionBeginEvent.connection_name")?;
                    let is_outgoing = obj
                        .get::<u8>("is_outgoing")
                        .with_context(|| "NetConnectionBeginEvent.is_outgoing")?
                        != 0;
                    processor.on_connection_begin(event_id, time, connection_name, is_outgoing)
                }
                "NetConnectionEndEvent" => {
                    let time = read_time(&obj).with_context(|| "NetConnectionEndEvent.time")?;
                    let bit_size =
                        read_bit_size(&obj).with_context(|| "NetConnectionEndEvent.bit_size")?;
                    processor.on_connection_end(event_id, time, bit_size)
                }
                "NetObjectBeginEvent" => {
                    let time = read_time(&obj).with_context(|| "NetObjectBeginEvent.time")?;
                    let object_name = obj
                        .get::<Arc<String>>("object_name")
                        .with_context(|| "NetObjectBeginEvent.object_name")?;
                    processor.on_object_begin(event_id, time, object_name)
                }
                "NetObjectEndEvent" => {
                    let time = read_time(&obj).with_context(|| "NetObjectEndEvent.time")?;
                    let bit_size =
                        read_bit_size(&obj).with_context(|| "NetObjectEndEvent.bit_size")?;
                    processor.on_object_end(event_id, time, bit_size)
                }
                "NetPropertyEvent" => {
                    let time = read_time(&obj).with_context(|| "NetPropertyEvent.time")?;
                    let property_name = obj
                        .get::<Arc<String>>("property_name")
                        .with_context(|| "NetPropertyEvent.property_name")?;
                    let bit_size =
                        read_bit_size(&obj).with_context(|| "NetPropertyEvent.bit_size")?;
                    processor.on_property(event_id, time, property_name, bit_size)
                }
                "NetRPCBeginEvent" => {
                    let time = read_time(&obj).with_context(|| "NetRPCBeginEvent.time")?;
                    let function_name = obj
                        .get::<Arc<String>>("function_name")
                        .with_context(|| "NetRPCBeginEvent.function_name")?;
                    processor.on_rpc_begin(event_id, time, function_name)
                }
                "NetRPCEndEvent" => {
                    let time = read_time(&obj).with_context(|| "NetRPCEndEvent.time")?;
                    let bit_size =
                        read_bit_size(&obj).with_context(|| "NetRPCEndEvent.bit_size")?;
                    processor.on_rpc_end(event_id, time, bit_size)
                }
                event_type => {
                    warn!("unknown event type in net block: {}", event_type);
                    Ok(true)
                }
            }
        } else {
            Ok(true)
        };
        event_id += 1;
        res
    })
}

/// Fetches and parses a net event block.
#[span_fn]
pub async fn parse_net_block<Proc: NetBlockProcessor>(
    blob_storage: Arc<BlobStorage>,
    stream: &StreamMetadata,
    block_id: sqlx::types::Uuid,
    object_offset: i64,
    processor: &mut Proc,
) -> Result<bool> {
    let payload =
        fetch_block_payload(blob_storage, stream.process_id, stream.stream_id, block_id).await?;
    parse_net_block_payload(object_offset, &payload, stream, processor)
}
