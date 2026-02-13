//! Process metadata
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

mod uuid_serde {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn uuid_to_string<S>(id: &uuid::Uuid, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&id.to_string())
    }

    pub fn uuid_from_string<'de, D>(deserializer: D) -> Result<uuid::Uuid, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        uuid::Uuid::try_parse(&s).map_err(serde::de::Error::custom)
    }

    pub fn opt_uuid_to_string<S>(id: &Option<uuid::Uuid>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match id {
            Some(id) => serializer.serialize_some(&id.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn opt_uuid_from_string<'de, D>(deserializer: D) -> Result<Option<uuid::Uuid>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: Option<String> = Option::deserialize(deserializer)?;
        match s {
            Some(s) => uuid::Uuid::try_parse(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    #[serde(
        deserialize_with = "uuid_serde::uuid_from_string",
        serialize_with = "uuid_serde::uuid_to_string"
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
        deserialize_with = "uuid_serde::opt_uuid_from_string",
        serialize_with = "uuid_serde::opt_uuid_to_string"
    )]
    pub parent_process_id: Option<uuid::Uuid>,
    pub properties: HashMap<String, String>,
}
