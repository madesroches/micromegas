use anyhow::Result;
use micromegas::tracing::debug;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
pub struct KeyRingEntry {
    pub name: String,
    pub key: String,
}

pub type KeyRing = HashMap<String, String>; // key -> name

pub fn parse_key_ring(json: &str) -> Result<KeyRing> {
    let entries: Vec<KeyRingEntry> = serde_json::from_str(json)?;
    let mut ring = KeyRing::new();
    for entry in entries {
        ring.insert(entry.key, entry.name);
    }
    debug!("loaded keys: {:?}", ring.values());
    Ok(ring)
}
