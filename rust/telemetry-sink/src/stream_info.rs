use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_tracing::event::{EventStream, ExtractDeps, TracingBlock};
use micromegas_transit::HeterogeneousQueue;
use micromegas_transit::UserDefinedType;
use std::collections::HashMap;

fn extract_secondary_udts(
    secondary_types: &mut HashMap<String, UserDefinedType>,
    udt: &UserDefinedType,
) {
    for secondary in &udt.secondary_udts {
        secondary_types.insert(secondary.name.clone(), secondary.clone());
        extract_secondary_udts(secondary_types, secondary);
    }
}

fn flatten_metadata(udts: Vec<UserDefinedType>) -> Vec<UserDefinedType> {
    let mut secondary_types = HashMap::new();
    let mut result = vec![];
    for udt in udts {
        extract_secondary_udts(&mut secondary_types, &udt);
        result.push(udt);
    }
    for (_k, v) in secondary_types {
        result.push(v);
    }
    result
}

pub fn make_stream_info<Block>(stream: &EventStream<Block>) -> StreamInfo
where
    Block: TracingBlock,
    <Block as TracingBlock>::Queue: micromegas_transit::HeterogeneousQueue,
    <<Block as TracingBlock>::Queue as ExtractDeps>::DepsQueue:
        micromegas_transit::HeterogeneousQueue,
{
    let dependencies_meta = flatten_metadata(
        <<Block as TracingBlock>::Queue as ExtractDeps>::DepsQueue::reflect_contained(),
    );
    let obj_meta = flatten_metadata(<Block as TracingBlock>::Queue::reflect_contained());
    StreamInfo {
        process_id: stream.process_id().to_owned(),
        stream_id: stream.stream_id().to_owned(),
        dependencies_metadata: dependencies_meta,
        objects_metadata: obj_meta,
        tags: stream.tags().to_owned(),
        properties: stream.properties().clone(),
    }
}
