use micromegas_transit::UserDefinedType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

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

impl StreamInfo {
    // only makes sense if the stream is associated with a thread
    pub fn get_thread_name(&self) -> String {
        const THREAD_NAME_KEY: &str = "thread-name";
        const THREAD_ID_KEY: &str = "thread-id";
        let opt_name = self
            .properties
            .get(&THREAD_NAME_KEY.to_owned())
            .or_else(|| self.properties.get(&THREAD_ID_KEY.to_owned()));
        if let Some(name) = opt_name {
            name.to_owned()
        } else {
            format!("{}", &self.stream_id)
        }
    }
}
