use micromegas_transit::UserDefinedType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Contains information about a telemetry stream.
///
/// This struct is sent once at the beginning of a stream and provides
/// metadata about the stream, such as the process and stream IDs,
/// dependencies, and other properties.
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamInfo {
    #[serde(
        deserialize_with = "micromegas_transit::uuid_utils::uuid_from_string",
        serialize_with = "micromegas_transit::uuid_utils::uuid_to_string"
    )]
    pub process_id: Uuid,
    #[serde(
        deserialize_with = "micromegas_transit::uuid_utils::uuid_from_string",
        serialize_with = "micromegas_transit::uuid_utils::uuid_to_string"
    )]
    pub stream_id: Uuid,
    pub dependencies_metadata: Vec<UserDefinedType>,
    pub objects_metadata: Vec<UserDefinedType>,
    pub tags: Vec<String>,
    pub properties: HashMap<String, String>,
}
