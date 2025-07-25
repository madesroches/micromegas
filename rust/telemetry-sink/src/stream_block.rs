use anyhow::Result;
use micromegas_telemetry::{block_wire_format, compression::compress, wire_format::encode_cbor};
use micromegas_tracing::{
    event::{EventBlock, ExtractDeps, TracingBlock},
    logs::LogBlock,
    metrics::MetricsBlock,
    prelude::*,
    spans::ThreadBlock,
};
use micromegas_transit::HeterogeneousQueue;

pub trait StreamBlock {
    /// Encodes the stream block into a binary format.
    ///
    /// This function serializes the block data, compresses it, and then encodes it
    /// into the wire format for transmission.
    ///
    /// # Arguments
    ///
    /// * `process_info` - Information about the current process, used for time calibration.
    fn encode_bin(&self, process_info: &ProcessInfo) -> Result<Vec<u8>>;
}

fn encode_block<Q>(block: &EventBlock<Q>, process_info: &ProcessInfo) -> Result<Vec<u8>>
where
    Q: HeterogeneousQueue + ExtractDeps,
    <Q as ExtractDeps>::DepsQueue: HeterogeneousQueue,
{
    let block_id = uuid::Uuid::new_v4();
    trace!("encoding block_id={block_id}");
    let end = block.end.as_ref().unwrap();

    let payload = block_wire_format::BlockPayload {
        dependencies: compress(block.events.extract().as_bytes())?,
        objects: compress(block.events.as_bytes())?,
    };

    let block = block_wire_format::Block {
        block_id,
        stream_id: block.stream_id,
        process_id: block.process_id,
        begin_time: block
            .begin
            .time
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
        begin_ticks: block.begin.ticks - process_info.start_ticks,
        end_time: end
            .time
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
        end_ticks: end.ticks - process_info.start_ticks,
        payload,
        nb_objects: block.nb_objects() as i32,
        object_offset: block.object_offset() as i64,
    };
    encode_cbor(&block)
}

impl StreamBlock for LogBlock {
    fn encode_bin(&self, process_info: &ProcessInfo) -> Result<Vec<u8>> {
        encode_block(self, process_info)
    }
}

impl StreamBlock for MetricsBlock {
    fn encode_bin(&self, process_info: &ProcessInfo) -> Result<Vec<u8>> {
        encode_block(self, process_info)
    }
}

impl StreamBlock for ThreadBlock {
    fn encode_bin(&self, process_info: &ProcessInfo) -> Result<Vec<u8>> {
        encode_block(self, process_info)
    }
}
