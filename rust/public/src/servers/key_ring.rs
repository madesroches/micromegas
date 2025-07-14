use anyhow::Result;
use micromegas_tracing::prelude::*;
use serde::Deserialize;
use std::{collections::HashMap, fmt::Display};

/// Represents a key in the keyring.
#[derive(Hash, Eq, PartialEq)]
pub struct Key {
    pub value: String,
}

impl Key {
    /// Creates a new `Key` from a string value.
    pub fn new(value: String) -> Self {
        Self { value }
    }
}

impl From<String> for Key {
    fn from(value: String) -> Self {
        Self { value }
    }
}

impl Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<sensitive key>")
    }
}

/// Deserializes a string into a `Key`.
///
/// This function is used by `serde` to deserialize the key from a JSON string.
pub fn key_from_string<'de, D>(deserializer: D) -> Result<Key, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    Ok(Key::new(s))
}

/// Represents an entry in the keyring, mapping a key to a name.
#[derive(Deserialize)]
pub struct KeyRingEntry {
    pub name: String,
    #[serde(deserialize_with = "key_from_string")]
    pub key: Key,
}

/// A map from `Key` to `String` (name).
pub type KeyRing = HashMap<Key, String>; // key -> name

/// Parses a JSON string into a `KeyRing`.
///
/// The JSON string is expected to be an array of objects, each with a `name` and `key` field.
pub fn parse_key_ring(json: &str) -> Result<KeyRing> {
    let entries: Vec<KeyRingEntry> = serde_json::from_str(json)?;
    let mut ring = KeyRing::new();
    for entry in entries {
        ring.insert(entry.key, entry.name);
    }
    info!("loaded keys: {:?}", ring.values());
    Ok(ring)
}
