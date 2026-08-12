//! Format → decoder registry for `parse_block`: turns a block payload of any
//! registered `streams.format` into generic `(type_name, jsonb)` objects, one
//! per row, mirroring the existing `BlockProcessorMap` pattern
//! (`block_partition_spec.rs`).

use crate::lakehouse::otel::block_decoders::{
    OtelLogsBlockDecoder, OtelMetricsBlockDecoder, OtelTracesBlockDecoder,
};
use crate::lakehouse::parse_block_table_function::transit_value_to_jsonb;
use crate::metadata::StreamMetadata;
use crate::payload::parse_block;
use anyhow::Result;
use micromegas_ingestion::web_ingestion_service::{
    FORMAT_OTLP_LOGS, FORMAT_OTLP_METRICS, FORMAT_OTLP_TRACES, FORMAT_TRANSIT,
};
use micromegas_telemetry::block_wire_format::BlockPayload;
use micromegas_tracing::prelude::*;
use micromegas_transit::value::Value as TransitValue;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

/// Receives objects decoded from a block payload, in payload order.
pub trait ObjectVisitor {
    /// `value` is the JSONB encoding of one object. Returns false to stop iteration.
    fn visit(&mut self, type_name: &str, value: &[u8]) -> Result<bool>;

    /// Consumes an ordinal without emitting a row (a payload entry the decoder
    /// recognizes but cannot represent). Keeps `object_index` aligned with the
    /// block's true ordinals. Returns false to stop iteration.
    fn skip(&mut self) -> Result<bool>;
}

/// Decodes one block payload of a given `streams.format` into generic objects.
pub trait BlockObjectDecoder: Send + Sync + Debug {
    fn decode(
        &self,
        stream: &StreamMetadata,
        payload: &BlockPayload,
        visitor: &mut dyn ObjectVisitor,
    ) -> Result<()>;
}

/// Map from `streams.format` to the decoder that turns that wire format into
/// generic `(type_name, jsonb)` objects for `parse_block`.
pub type BlockObjectDecoderMap = HashMap<&'static str, Arc<dyn BlockObjectDecoder>>;

/// Decodes `micromegas-transit` block payloads: the native CBOR/transit wire
/// format used by every in-tree SDK. A straight lift of the per-object callback
/// `parse_block_table_function.rs` used to drive directly.
#[derive(Debug)]
pub struct TransitBlockDecoder;

impl BlockObjectDecoder for TransitBlockDecoder {
    fn decode(
        &self,
        stream: &StreamMetadata,
        payload: &BlockPayload,
        visitor: &mut dyn ObjectVisitor,
    ) -> Result<()> {
        parse_block(stream, payload, |value| {
            if let TransitValue::Object(obj) = value {
                let jsonb_val = transit_value_to_jsonb(value);
                let mut buf = Vec::new();
                jsonb_val.write_to_vec(&mut buf);
                visitor.visit(obj.type_name, &buf)
            } else {
                // Defensive branch: no in-tree writer emits a non-Object top-level
                // value (see Testing Strategy in the design plan). `skip()` keeps
                // `object_index` aligned with the block's true ordinals.
                warn!("parse_block: skipping non-Object value in transit block");
                visitor.skip()
            }
        })?;
        Ok(())
    }
}

/// Registry covering every `streams.format` shipped in-tree.
pub fn default_block_object_decoders() -> Arc<BlockObjectDecoderMap> {
    let mut m: BlockObjectDecoderMap = HashMap::new();
    m.insert(
        FORMAT_TRANSIT,
        Arc::new(TransitBlockDecoder) as Arc<dyn BlockObjectDecoder>,
    );
    m.insert(
        FORMAT_OTLP_LOGS,
        Arc::new(OtelLogsBlockDecoder) as Arc<dyn BlockObjectDecoder>,
    );
    m.insert(
        FORMAT_OTLP_METRICS,
        Arc::new(OtelMetricsBlockDecoder) as Arc<dyn BlockObjectDecoder>,
    );
    m.insert(
        FORMAT_OTLP_TRACES,
        Arc::new(OtelTracesBlockDecoder) as Arc<dyn BlockObjectDecoder>,
    );
    Arc::new(m)
}
