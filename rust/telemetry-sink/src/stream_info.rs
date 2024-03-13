use lgn_telemetry_proto::telemetry::{
    ContainerMetadata, Stream as StreamProto, UdtMember as UdtMemberProto,
    UserDefinedType as UserDefinedTypeProto,
};
use micromegas_tracing::event::{EventStream, ExtractDeps, TracingBlock};
use micromegas_transit::HeterogeneousQueue;
use micromegas_transit::UserDefinedType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub fn get_stream_info_proto<Block>(stream: &EventStream<Block>) -> StreamProto
where
    Block: TracingBlock,
    <Block as TracingBlock>::Queue: micromegas_transit::HeterogeneousQueue,
    <<Block as TracingBlock>::Queue as ExtractDeps>::DepsQueue: micromegas_transit::HeterogeneousQueue,
{
    let dependencies_meta =
        make_queue_metadata_proto::<<<Block as TracingBlock>::Queue as ExtractDeps>::DepsQueue>();
    let obj_meta = make_queue_metadata_proto::<Block::Queue>();
    StreamProto {
        process_id: stream.process_id().to_owned(),
        stream_id: stream.stream_id().to_owned(),
        dependencies_metadata: Some(dependencies_meta),
        objects_metadata: Some(obj_meta),
        tags: stream.tags().to_owned(),
        properties: stream.properties().clone(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StreamInfo {
    pub process_id: String,
    pub stream_id: String,
    pub dependencies_metadata: Vec<UserDefinedType>,
    pub objects_metadata: Vec<UserDefinedType>,
    pub tags: Vec<String>,
    pub properties: HashMap<String, String>,
}

pub fn get_stream_info<Block>(stream: &EventStream<Block>) -> StreamInfo
where
    Block: TracingBlock,
    <Block as TracingBlock>::Queue: micromegas_transit::HeterogeneousQueue,
    <<Block as TracingBlock>::Queue as ExtractDeps>::DepsQueue: micromegas_transit::HeterogeneousQueue,
{
    //todo: we should extract secondary udts
    let dependencies_meta =
        <<Block as TracingBlock>::Queue as ExtractDeps>::DepsQueue::reflect_contained();
    let obj_meta = <Block as TracingBlock>::Queue::reflect_contained();
    StreamInfo {
        process_id: stream.process_id().to_owned(),
        stream_id: stream.stream_id().to_owned(),
        dependencies_metadata: dependencies_meta,
        objects_metadata: obj_meta,
        tags: stream.tags().to_owned(),
        properties: stream.properties().clone(),
    }
}

fn proto_from_udt(
    secondary_types: &mut HashMap<String, UserDefinedTypeProto>,
    udt: &UserDefinedType,
) -> UserDefinedTypeProto {
    for secondary in &udt.secondary_udts {
        let sec_proto = proto_from_udt(secondary_types, secondary);
        secondary_types.insert(sec_proto.name.clone(), sec_proto);
    }
    UserDefinedTypeProto {
        name: udt.name.clone(),
        size: udt.size as u32,
        members: udt
            .members
            .iter()
            .map(|member| UdtMemberProto {
                name: member.name.clone(),
                type_name: member.type_name.clone(),
                offset: member.offset as u32,
                size: member.size as u32,
                is_reference: member.is_reference,
            })
            .collect(),
        is_reference: udt.is_reference,
    }
}

fn make_queue_metadata_proto<Queue: micromegas_transit::HeterogeneousQueue>() -> ContainerMetadata {
    let udts = Queue::reflect_contained();
    let mut secondary_types = HashMap::new();
    let mut types: Vec<UserDefinedTypeProto> = udts
        .iter()
        .map(|udt| proto_from_udt(&mut secondary_types, udt))
        .collect();
    for (_k, v) in secondary_types {
        types.push(v);
    }
    ContainerMetadata { types }
}
