//! CLI arguments and configuration for the Redis exporter.
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use redis::{ConnectionAddr, ConnectionInfo, IntoConnectionInfo};

/// Cumulative metric-collection presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum MetricsPreset {
    /// One INFO call per tick: memory, clients, throughput, keyspace
    /// efficiency, replication, persistence, per-db key counts.
    Core,
    /// Core + SLOWLOG LEN.
    Extended,
    /// Extended + per-command stats (INFO COMMANDSTATS) + LATENCY LATEST.
    Full,
}

#[derive(Parser, Debug)]
#[clap(name = "Micromegas Redis exporter")]
#[clap(
    about = "Samples a Redis server and sends its metrics to a Micromegas stack",
    version,
    author
)]
pub struct Cli {
    /// Redis connection URL (redis:// or rediss://)
    #[clap(
        long,
        default_value = "redis://127.0.0.1:6379",
        env = "MICROMEGAS_REDIS_EXPORTER_REDIS_URL"
    )]
    pub redis_url: String,

    /// Metric collection preset
    #[clap(
        long,
        value_enum,
        default_value = "full",
        env = "MICROMEGAS_REDIS_EXPORTER_METRICS"
    )]
    pub metrics: MetricsPreset,

    /// Seconds between samples
    #[clap(
        long,
        default_value = "1",
        env = "MICROMEGAS_REDIS_EXPORTER_SAMPLE_INTERVAL_SECONDS"
    )]
    pub sample_interval_seconds: u64,

    /// Name identifying this Redis instance in emitted metrics
    /// (default: host:port derived from the URL)
    #[clap(long, env = "MICROMEGAS_REDIS_EXPORTER_TARGET_NAME")]
    pub target_name: Option<String>,

    /// Extra key=value property attached to every metric (repeatable;
    /// comma-separated pairs in the env var)
    #[clap(
        long = "property",
        env = "MICROMEGAS_REDIS_EXPORTER_PROPERTIES",
        value_delimiter = ','
    )]
    pub properties: Vec<String>,

    /// Optional listen address for the /health and /ready probe endpoints
    /// (e.g. 0.0.0.0:8081); off when absent
    #[clap(long, env = "MICROMEGAS_REDIS_EXPORTER_HEALTH_LISTEN_ADDR")]
    pub health_listen_addr: Option<SocketAddr>,
}

/// Validated runtime configuration.
#[derive(Debug)]
pub struct Config {
    pub connection_info: ConnectionInfo,
    pub target_name: String,
    pub preset: MetricsPreset,
    pub sample_interval: Duration,
    pub properties: Vec<(String, String)>,
    pub health_listen_addr: Option<SocketAddr>,
}

impl Cli {
    /// `redis_password` (from `MICROMEGAS_REDIS_EXPORTER_REDIS_PASSWORD`)
    /// overrides any password embedded in the URL; passed as a parameter so
    /// tests never touch process env.
    pub fn into_config(self, redis_password: Option<String>) -> Result<Config> {
        if self.sample_interval_seconds == 0 {
            bail!("--sample-interval-seconds must be at least 1");
        }
        let mut connection_info = self
            .redis_url
            .as_str()
            .into_connection_info()
            .with_context(|| format!("parsing --redis-url {:?}", self.redis_url))?;
        if let Some(password) = redis_password.filter(|p| !p.is_empty()) {
            let redis_settings = connection_info
                .redis_settings()
                .clone()
                .set_password(password);
            connection_info = connection_info.set_redis_settings(redis_settings);
        }
        let target_name = match self.target_name {
            Some(name) => name,
            None => derive_target_name(&connection_info),
        };
        Ok(Config {
            target_name,
            preset: self.metrics,
            sample_interval: Duration::from_secs(self.sample_interval_seconds),
            properties: parse_properties(&self.properties)?,
            health_listen_addr: self.health_listen_addr,
            connection_info,
        })
    }
}

/// Default instance identity: host:port (or socket path), never credentials.
pub fn derive_target_name(info: &ConnectionInfo) -> String {
    match info.addr() {
        ConnectionAddr::Tcp(host, port) => format!("{host}:{port}"),
        ConnectionAddr::TcpTls { host, port, .. } => format!("{host}:{port}"),
        ConnectionAddr::Unix(path) => path.display().to_string(),
        _ => "unknown".to_string(),
    }
}

/// Property keys reserved for tags the exporter attaches itself; `property_get`
/// is case-insensitive in SQL, so the check below is too.
const RESERVED_PROPERTY_KEYS: &[&str] = &["instance", "command", "db", "event"];

/// Parses `key=value` pairs; reserved keys (see [`RESERVED_PROPERTY_KEYS`])
/// are rejected case-insensitively to avoid colliding with tags the exporter
/// attaches itself. Empty entries are dropped first: clap's `value_delimiter`
/// turns an empty `MICROMEGAS_REDIS_EXPORTER_PROPERTIES=""` env var into one
/// empty item, which k8s templates commonly produce for an unset optional var.
pub fn parse_properties(raw: &[String]) -> Result<Vec<(String, String)>> {
    raw.iter()
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (key, value) = entry
                .split_once('=')
                .with_context(|| format!("invalid --property {entry:?}: expected key=value"))?;
            if key.is_empty() {
                bail!("invalid --property {entry:?}: empty key");
            }
            if RESERVED_PROPERTY_KEYS
                .iter()
                .any(|reserved| key.eq_ignore_ascii_case(reserved))
            {
                bail!(
                    "property key {key:?} is reserved (reserved keys: {})",
                    RESERVED_PROPERTY_KEYS.join(", ")
                );
            }
            Ok((key.to_string(), value.to_string()))
        })
        .collect()
}
