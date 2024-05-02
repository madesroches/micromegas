use serde::de::Error;
use serde::Deserialize;
use uuid::Uuid;

pub fn uuid_from_string<'de, D>(deserializer: D) -> Result<Uuid, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    Uuid::parse_str(&s).map_err(D::Error::custom)
}

pub fn opt_uuid_from_string<'de, D>(deserializer: D) -> Result<Option<Uuid>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    if s.is_empty() {
        Ok(None)
    } else {
        Uuid::parse_str(&s).map(Some).map_err(D::Error::custom)
    }
}

pub fn uuid_to_string<S>(value: &uuid::Uuid, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

pub fn opt_uuid_to_string<S>(value: &Option<uuid::Uuid>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if let Some(v) = value {
        serializer.serialize_str(&v.to_string())
    } else {
        serializer.serialize_str("")
    }
}
