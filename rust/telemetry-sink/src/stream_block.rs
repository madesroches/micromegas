use crate::{block_wire_format, compression::compress, wire_format::encode_cbor};
use anyhow::Result;
use micromegas_tracing::{
    event::{ExtractDeps, TracingBlock},
    logs::LogBlock,
    metrics::MetricsBlock,
    spans::ThreadBlock,
};

pub trait StreamBlock {
    fn encode_bin(&self) -> Result<Vec<u8>>;
}

impl StreamBlock for LogBlock {
    fn encode_bin(&self) -> Result<Vec<u8>> {
        let block_id = uuid::Uuid::new_v4().to_string();
        let end = self.end.as_ref().unwrap();

        let payload = block_wire_format::BlockPayload {
            dependencies: compress(self.events.extract().as_bytes())?,
            objects: compress(self.events.as_bytes())?,
        };

        let block = block_wire_format::Block {
            stream_id: self.stream_id.clone(),
            block_id,
            begin_time: self
                .begin
                .time
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
            begin_ticks: self.begin.ticks,
            end_time: end
                .time
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
            end_ticks: end.ticks,
            payload,
            nb_objects: self.nb_objects() as i32,
        };
        encode_cbor(&block)
    }

    // #[allow(clippy::cast_possible_wrap)]
    // fn encode(&self) -> Result<EncodedBlock> {
    //     let block_id = uuid::Uuid::new_v4().to_string();
    //     let end = self.end.as_ref().unwrap();

    //     let payload = lgn_telemetry_proto::telemetry::BlockPayload {
    //         dependencies: compress(self.events.extract().as_bytes())?,
    //         objects: compress(self.events.as_bytes())?,
    //     };

    //     Ok(EncodedBlock {
    //         stream_id: self.stream_id.clone(),
    //         block_id,
    //         begin_time: self
    //             .begin
    //             .time
    //             .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
    //         begin_ticks: self.begin.ticks,
    //         end_time: end
    //             .time
    //             .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
    //         end_ticks: end.ticks,
    //         payload: Some(payload),
    //         nb_objects: self.nb_objects() as i32,
    //     })
    // }
}

impl StreamBlock for MetricsBlock {
    fn encode_bin(&self) -> Result<Vec<u8>> {
        todo!();
        //Ok(vec![])
    }

    // #[allow(clippy::cast_possible_wrap)]
    // fn encode(&self) -> Result<EncodedBlock> {
    //     let block_id = uuid::Uuid::new_v4().to_string();
    //     let end = self.end.as_ref().unwrap();

    //     let payload = lgn_telemetry_proto::telemetry::BlockPayload {
    //         dependencies: compress(self.events.extract().as_bytes())?,
    //         objects: compress(self.events.as_bytes())?,
    //     };

    //     Ok(EncodedBlock {
    //         stream_id: self.stream_id.clone(),
    //         block_id,
    //         begin_time: self
    //             .begin
    //             .time
    //             .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
    //         begin_ticks: self.begin.ticks,
    //         end_time: end
    //             .time
    //             .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
    //         end_ticks: end.ticks,
    //         payload: Some(payload),
    //         nb_objects: self.nb_objects() as i32,
    //     })
    // }
}

impl StreamBlock for ThreadBlock {
    fn encode_bin(&self) -> Result<Vec<u8>> {
        todo!();
        //Ok(vec![])
    }

    // #[allow(clippy::cast_possible_wrap)]
    // fn encode(&self) -> Result<EncodedBlock> {
    //     let block_id = uuid::Uuid::new_v4().to_string();
    //     let end = self.end.as_ref().unwrap();

    //     let payload = lgn_telemetry_proto::telemetry::BlockPayload {
    //         dependencies: compress(self.events.extract().as_bytes())?,
    //         objects: compress(self.events.as_bytes())?,
    //     };

    //     Ok(EncodedBlock {
    //         stream_id: self.stream_id.clone(),
    //         block_id,
    //         begin_time: self
    //             .begin
    //             .time
    //             .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
    //         begin_ticks: self.begin.ticks,
    //         end_time: end
    //             .time
    //             .to_rfc3339_opts(chrono::SecondsFormat::Nanos, false),
    //         end_ticks: end.ticks,
    //         payload: Some(payload),
    //         nb_objects: self.nb_objects() as i32,
    //     })
    // }
}
