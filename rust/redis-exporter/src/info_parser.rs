//! Parsing of the text response of Redis `INFO` (including the
//! Commandstats and Keyspace sections). Unknown or malformed fields are
//! skipped, never fatal: a Redis version that adds or renames fields must
//! not crash the exporter.
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyspaceEntry {
    pub db: u32,
    pub keys: u64,
    pub expires: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandStat {
    pub name: String,
    pub calls: u64,
    pub usec: u64,
    pub usec_per_call: f64,
}

/// Field map of one INFO response. INFO field names are globally unique,
/// so section headers are ignored.
#[derive(Debug, Default)]
pub struct ParsedInfo {
    fields: HashMap<String, String>,
}

impl ParsedInfo {
    pub fn parse(raw: &str) -> Self {
        let mut fields = HashMap::new();
        for line in raw.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                fields.insert(key.to_string(), value.to_string());
            }
        }
        Self { fields }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get_str(key)?.parse().ok()
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.get_str(key)?.parse().ok()
    }

    /// Entries of the Keyspace section (`db0:keys=5,expires=1,avg_ttl=0`),
    /// sorted by db index.
    pub fn keyspace(&self) -> Vec<KeyspaceEntry> {
        let mut entries: Vec<KeyspaceEntry> = self
            .fields
            .iter()
            .filter_map(|(key, value)| {
                let db: u32 = key.strip_prefix("db")?.parse().ok()?;
                let kv = parse_kv_pairs(value);
                Some(KeyspaceEntry {
                    db,
                    keys: kv.get("keys")?.parse().ok()?,
                    expires: kv.get("expires")?.parse().ok()?,
                })
            })
            .collect();
        entries.sort_by_key(|e| e.db);
        entries
    }

    /// Entries of the Commandstats section
    /// (`cmdstat_get:calls=1,usec=2,usec_per_call=2.0,...`), sorted by name.
    pub fn command_stats(&self) -> Vec<CommandStat> {
        let mut stats: Vec<CommandStat> = self
            .fields
            .iter()
            .filter_map(|(key, value)| {
                let name = key.strip_prefix("cmdstat_")?;
                let kv = parse_kv_pairs(value);
                Some(CommandStat {
                    name: name.to_string(),
                    calls: kv.get("calls")?.parse().ok()?,
                    usec: kv.get("usec")?.parse().ok()?,
                    usec_per_call: kv.get("usec_per_call")?.parse().ok()?,
                })
            })
            .collect();
        stats.sort_by(|a, b| a.name.cmp(&b.name));
        stats
    }
}

fn parse_kv_pairs(value: &str) -> HashMap<&str, &str> {
    value
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .collect()
}
