use anyhow::Result;
use micromegas_telemetry::{block_wire_format, compression::compress, wire_format::encode_cbor};
use micromegas_tracing::{
    event::{EventBlock, ExtractDeps, TracingBlock},
    logs::LogBlock,
    metrics::MetricsBlock,
    spans::ThreadBlock,
};
use micromegas_transit::HeterogeneousQueue;

pub trait StreamBlock {
    fn encode_bin(&self) -> Result<Vec<u8>>;
}

fn encode_block<Q>(block: &EventBlock<Q>) -> Result<Vec<u8>>
where
    Q: HeterogeneousQueue + ExtractDeps,
    <Q as ExtractDeps>::DepsQueue: HeterogeneousQueue,
{
    let block_id = uuid::Uuid::new_v4().to_string();
    let end = block.end.as_ref().unwrap();

    let payload = block_wire_format::BlockPayload {
        dependencies: compress(block.events.extract().as_bytes())?,
        objects: compress(block.events.as_bytes())?,
    };

    let block = block_wire_format::Block {
        stream_id: block.stream_id.clone(),
        block_id,
        begin_time: block
            .begin
            .time
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
        begin_ticks: block.begin.ticks,
        end_time: end
            .time
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
        end_ticks: end.ticks,
        payload,
        nb_objects: block.nb_objects() as i32,
    };
    encode_cbor(&block)
}

impl StreamBlock for LogBlock {
    fn encode_bin(&self) -> Result<Vec<u8>> {
        encode_block(self)
    }
}

impl StreamBlock for MetricsBlock {
    fn encode_bin(&self) -> Result<Vec<u8>> {
        encode_block(self)
    }
}

impl StreamBlock for ThreadBlock {
    fn encode_bin(&self) -> Result<Vec<u8>> {
        encode_block(self)
    }
}
