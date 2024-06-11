#[derive(Debug, Clone, PartialEq)]
pub struct BlockMetadata {
    pub block_id: uuid::Uuid,
    pub stream_id: uuid::Uuid,
    pub process_id: uuid::Uuid,
    pub begin_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub begin_ticks: i64,
    pub end_ticks: i64,
    pub nb_objects: i32,
    pub payload_size: i64,
    pub object_offset: i64,
}
