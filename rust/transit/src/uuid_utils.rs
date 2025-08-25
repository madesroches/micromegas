use serde::Deserialize;
use serde::de::Error;
use uuid::Uuid;

/// Deserializes a UUID from a string using serde
pub fn uuid_from_string<'de, D>(deserializer: D) -> Result<Uuid, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    Uuid::parse_str(&s).map_err(D::Error::custom)
}

/// Deserializes an optional UUID from a string using serde, returning None for empty strings
pub fn opt_uuid_from_string<'de, D>(deserializer: D) -> Result<Option<Uuid>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt_s: Option<String> = Deserialize::deserialize(deserializer)?;
    if let Some(s) = opt_s {
        if s.is_empty() {
            Ok(None)
        } else {
            Uuid::parse_str(&s).map(Some).map_err(D::Error::custom)
        }
    } else {
        Ok(None)
    }
}

/// Serializes a UUID to a string using serde
pub fn uuid_to_string<S>(value: &uuid::Uuid, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

/// Serializes an optional UUID to a string using serde, using empty string for None
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

/// Parse an optional UUID from a string, returning None if the string is empty
pub fn parse_optional_uuid(s: &str) -> Result<Option<Uuid>, uuid::Error> {
    if s.is_empty() {
        Ok(None)
    } else {
        Uuid::parse_str(s).map(Some)
    }
}
