use micromegas_transit::uuid_utils;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    #[serde(
        deserialize_with = "uuid_utils::uuid_from_string",
        serialize_with = "uuid_utils::uuid_to_string"
    )]
    pub process_id: uuid::Uuid,
    pub exe: String,
    pub username: String,
    pub realname: String,
    pub computer: String,
    pub distro: String,
    pub cpu_brand: String,
    pub tsc_frequency: i64,
    /// RFC 3339
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub start_ticks: i64,
    #[serde(
        deserialize_with = "uuid_utils::opt_uuid_from_string",
        serialize_with = "uuid_utils::opt_uuid_to_string"
    )]
    pub parent_process_id: Option<uuid::Uuid>,
    pub properties: HashMap<String, String>,
}
