#[derive(Clone, PartialEq)]
pub struct Block {
    pub block_id: String,
    pub stream_id: String,
    pub begin_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub begin_ticks: i64,
    pub end_ticks: i64,
    pub payload: Option<BlockPayload>,
    pub nb_objects: i32,
}

#[derive(Clone, PartialEq)]
pub struct BlockPayload {
    pub dependencies: Vec<u8>,
    pub objects: Vec<u8>,
}

#[derive(Clone, PartialEq)]
pub struct BlockMetadata {
    pub block_id: String,
    pub stream_id: String,
    pub begin_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub begin_ticks: i64,
    pub end_ticks: i64,
    pub nb_objects: i32,
    pub payload_size: i64,
}

impl From<crate::api::components::Block> for Block {
    fn from(block: crate::api::components::Block) -> Self {
        Block {
            block_id: block.block_id,
            stream_id: block.stream_id,
            begin_time: block.begin_time,
            end_time: block.end_time,
            begin_ticks: block.begin_ticks,
            end_ticks: block.end_ticks,
            payload: block.payload.map(Into::into),
            nb_objects: block.nb_objects,
        }
    }
}

impl From<Block> for crate::api::components::Block {
    fn from(block: Block) -> Self {
        crate::api::components::Block {
            block_id: block.block_id,
            stream_id: block.stream_id,
            begin_time: block.begin_time,
            end_time: block.end_time,
            begin_ticks: block.begin_ticks,
            end_ticks: block.end_ticks,
            payload: block.payload.map(Into::into),
            nb_objects: block.nb_objects,
        }
    }
}

impl From<crate::api::components::BlockPayload> for BlockPayload {
    fn from(payload: crate::api::components::BlockPayload) -> Self {
        BlockPayload {
            dependencies: payload.dependencies.into(),
            objects: payload.objects.into(),
        }
    }
}

impl From<BlockPayload> for crate::api::components::BlockPayload {
    fn from(payload: BlockPayload) -> Self {
        crate::api::components::BlockPayload {
            dependencies: payload.dependencies.into(),
            objects: payload.objects.into(),
        }
    }
}

impl From<crate::api::components::BlockMetadata> for BlockMetadata {
    fn from(metadata: crate::api::components::BlockMetadata) -> Self {
        BlockMetadata {
            block_id: metadata.block_id,
            stream_id: metadata.stream_id,
            begin_time: metadata.begin_time,
            end_time: metadata.end_time,
            begin_ticks: metadata.begin_ticks,
            end_ticks: metadata.end_ticks,
            nb_objects: metadata.nb_objects,
            payload_size: metadata.payload_size,
        }
    }
}

impl From<BlockMetadata> for crate::api::components::BlockMetadata {
    fn from(metadata: BlockMetadata) -> Self {
        crate::api::components::BlockMetadata {
            block_id: metadata.block_id,
            stream_id: metadata.stream_id,
            begin_time: metadata.begin_time,
            end_time: metadata.end_time,
            begin_ticks: metadata.begin_ticks,
            end_ticks: metadata.end_ticks,
            nb_objects: metadata.nb_objects,
            payload_size: metadata.payload_size,
        }
    }
}
