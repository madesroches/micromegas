use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use datafusion::config::ConfigOptions;
use micromegas_tracing::prelude::*;
use std::collections::BTreeMap;
use std::fmt::{self, Debug, Display};
use std::sync::Arc;

/// Keys every session forces; an override could never take effect, so storing one is rejected.
pub const PINNED_KEYS: &[&str] = &[
    "datafusion.execution.parquet.enable_page_index",
    "datafusion.sql_parser.collect_spans",
    "datafusion.catalog.information_schema",
];

/// Validated DataFusion config overrides: key -> value as `ConfigOptions::set` takes it.
///
/// A sparse map applied on top of `SessionConfig::default()`; keys it does not list keep
/// DataFusion's defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataFusionConfigOverrides(BTreeMap<String, String>);

fn stringify_value(key: &str, value: &serde_json::Value) -> Result<String> {
    match value {
        serde_json::Value::String(s) => Ok(s.clone()),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::Bool(b) => Ok(b.to_string()),
        _ => Err(anyhow!(
            "value for '{key}' must be a string, number or boolean"
        )),
    }
}

fn validate_entry(key: &str, value: &serde_json::Value) -> Result<String> {
    let value = stringify_value(key, value)?;
    if PINNED_KEYS.contains(&key) {
        return Err(anyhow!(
            "'{key}' is set by micromegas on every session and cannot be overridden"
        ));
    }
    // A scratch ConfigOptions lets DataFusion judge both the key and the value.
    ConfigOptions::new()
        .set(key, &value)
        .with_context(|| format!("invalid DataFusion option '{key}'"))?;
    Ok(value)
}

fn as_object(value: &serde_json::Value) -> Result<&serde_json::Map<String, serde_json::Value>> {
    value
        .as_object()
        .ok_or_else(|| anyhow!("DataFusion config must be a JSON object of option -> value"))
}

impl DataFusionConfigOverrides {
    /// Strict: any bad entry is an error. Used by the PUT handler.
    pub fn parse_strict(value: &serde_json::Value) -> Result<Self> {
        let mut map = BTreeMap::new();
        for (key, v) in as_object(value)? {
            map.insert(key.clone(), validate_entry(key, v)?);
        }
        Ok(Self(map))
    }

    /// Lenient: a non-object is an error, but an entry DataFusion rejects is skipped with a
    /// warning. A stored entry can become invalid when DataFusion is upgraded, and that must not
    /// take the whole settings load down.
    pub fn parse_lenient(value: &serde_json::Value) -> Result<Self> {
        let mut map = BTreeMap::new();
        for (key, v) in as_object(value)? {
            match validate_entry(key, v) {
                Ok(v) => {
                    map.insert(key.clone(), v);
                }
                Err(e) => warn!("ignoring stored DataFusion config entry '{key}': {e:#}"),
            }
        }
        Ok(Self(map))
    }

    /// Sets every override on `options`.
    pub fn apply(&self, options: &mut ConfigOptions) -> Result<()> {
        for (key, value) in &self.0 {
            options
                .set(key, value)
                .with_context(|| format!("applying DataFusion option '{key}'"))?;
        }
        Ok(())
    }

    /// True when no option is overridden.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Comma-separated `key=value` list.
impl Display for DataFusionConfigOverrides {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (k, v)) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{k}={v}")?;
        }
        Ok(())
    }
}

/// Resolves the overrides applied to every session a `LakehouseContext` builds.
#[async_trait]
pub trait DataFusionConfigSource: Send + Sync + Debug {
    /// The current overrides.
    async fn overrides(&self) -> Result<Arc<DataFusionConfigOverrides>>;
}

/// The source of a `LakehouseContext` nobody configured: no overrides.
#[derive(Debug)]
pub struct NoDataFusionOverrides;

#[async_trait]
impl DataFusionConfigSource for NoDataFusionOverrides {
    async fn overrides(&self) -> Result<Arc<DataFusionConfigOverrides>> {
        Ok(Arc::new(DataFusionConfigOverrides::default()))
    }
}
