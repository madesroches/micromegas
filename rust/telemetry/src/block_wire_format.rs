// block wire format
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockPayload {
    pub dependencies: Vec<u8>,
    pub objects: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub block_id: String,
    pub stream_id: String,
    pub process_id: String,
    /// we send both RFC3339 times and ticks to be able to calibrate the tick
    pub begin_time: String,
    pub begin_ticks: i64,
    pub end_time: String,
    pub end_ticks: i64,
    pub payload: BlockPayload,
    pub object_offset: i64,
    pub nb_objects: i32,
}
