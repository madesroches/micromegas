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

#[derive(Debug, Clone, PartialEq)]
pub struct BlockMetadata {
    pub block_id: String,
    pub stream_id: String,
    pub process_id: String,
    pub begin_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub begin_ticks: i64,
    pub end_ticks: i64,
    pub nb_objects: i32,
    pub payload_size: i64,
}
