// block wire format
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockPayload {
    pub dependencies: Vec<u8>,
    pub objects: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    #[serde(
        deserialize_with = "micromegas_transit::uuid_utils::uuid_from_string",
        serialize_with = "micromegas_transit::uuid_utils::uuid_to_string"
    )]
    pub block_id: uuid::Uuid,
    #[serde(
        deserialize_with = "micromegas_transit::uuid_utils::uuid_from_string",
        serialize_with = "micromegas_transit::uuid_utils::uuid_to_string"
    )]
    pub stream_id: uuid::Uuid,
    #[serde(
        deserialize_with = "micromegas_transit::uuid_utils::uuid_from_string",
        serialize_with = "micromegas_transit::uuid_utils::uuid_to_string"
    )]
    pub process_id: uuid::Uuid,
    /// we send both RFC3339 times and ticks to be able to calibrate the tick
    pub begin_time: String,
    pub begin_ticks: i64,
    pub end_time: String,
    pub end_ticks: i64,
    pub payload: BlockPayload,
    pub object_offset: i64,
    pub nb_objects: i32,
}
