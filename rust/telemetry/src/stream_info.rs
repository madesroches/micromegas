use micromegas_transit::UserDefinedType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct StreamInfo {
    pub process_id: String,
    pub stream_id: String,
    pub dependencies_metadata: Vec<UserDefinedType>,
    pub objects_metadata: Vec<UserDefinedType>,
    pub tags: Vec<String>,
    pub properties: HashMap<String, String>,
}
