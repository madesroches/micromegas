// block wire format
use serde::{Deserialize, Serialize};

/// Payload sent by instrumented processes, containing serialized dependencies and objects.
///
/// The `dependencies` field contains the serialized data for all the
/// `UserDefinedType` that are required to deserialize the `objects`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockPayload {
    pub dependencies: Vec<u8>,
    pub objects: Vec<u8>,
}

/// Block metadata sent by instrumented processes.
///
/// A block represents a chunk of telemetry data from a single stream.
/// It contains timing information, references to the process and stream,
/// and the actual payload of telemetry events.
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
