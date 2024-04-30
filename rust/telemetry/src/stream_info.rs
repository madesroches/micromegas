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

impl StreamInfo {
    // only makes sense if the stream is associated with a thread
    pub fn get_thread_name(&self) -> String {
        const THREAD_NAME_KEY: &str = "thread-name";
        const THREAD_ID_KEY: &str = "thread-id";
        self.properties
            .get(&THREAD_NAME_KEY.to_owned())
            .unwrap_or_else(|| {
                self.properties
                    .get(&THREAD_ID_KEY.to_owned())
                    .unwrap_or(&self.stream_id)
            })
            .clone()
    }
}
