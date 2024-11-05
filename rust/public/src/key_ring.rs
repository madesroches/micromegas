use anyhow::Result;
use micromegas_tracing::prelude::*;
use serde::Deserialize;
use std::{collections::HashMap, fmt::Display};

#[derive(Hash, Eq, PartialEq)]
pub struct Key {
    pub value: String,
}

impl Key {
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

pub fn key_from_string<'de, D>(deserializer: D) -> Result<Key, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    Ok(Key::new(s))
}

#[derive(Deserialize)]
pub struct KeyRingEntry {
    pub name: String,
    #[serde(deserialize_with = "key_from_string")]
    pub key: Key,
}

pub type KeyRing = HashMap<Key, String>; // key -> name

pub fn parse_key_ring(json: &str) -> Result<KeyRing> {
    let entries: Vec<KeyRingEntry> = serde_json::from_str(json)?;
    let mut ring = KeyRing::new();
    for entry in entries {
        ring.insert(entry.key.into(), entry.name);
    }
    info!("loaded keys: {:?}", ring.values());
    Ok(ring)
}
