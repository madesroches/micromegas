use micromegas_tracing::prelude::*;
use std::path::PathBuf;

/// Compiled-in ceilings: configuration can lower these but never exceed them.
pub const ABSOLUTE_MAX_ROWS: usize = 10_000;
pub const ABSOLUTE_MAX_BYTES: usize = 1024 * 1024;

pub const DEFAULT_ROWS: usize = 1_000;
pub const DEFAULT_MAX_BYTES: usize = 128 * 1024;
pub const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

const DEFAULT_ALLOWED_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1"];

/// Settings of the MCP route, read once at startup.
#[derive(Debug, Clone)]
pub struct McpConfig {
    pub enabled: bool,
    /// Full path of the route, including any base path (e.g. `/micromegas/mcp`).
    pub path: String,
    /// Fixed FlightSQL target; when unset the host decides (e.g. the default data source).
    pub flightsql_url: Option<String>,
    /// Row count used when a call gives no `limit`.
    pub default_rows: usize,
    /// Largest `limit` a call may ask for.
    pub max_rows: usize,
    /// Byte cap on a rendered result.
    pub max_bytes: usize,
    pub skills_dir: Option<PathBuf>,
    pub allowed_hosts: Vec<String>,
    /// Empty means no Origin validation, as the Streamable HTTP spec requires of non-browser servers.
    pub allowed_origins: Vec<String>,
    pub max_request_body_bytes: usize,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: "/mcp".to_string(),
            flightsql_url: None,
            default_rows: DEFAULT_ROWS,
            max_rows: ABSOLUTE_MAX_ROWS,
            max_bytes: DEFAULT_MAX_BYTES,
            skills_dir: None,
            allowed_hosts: DEFAULT_ALLOWED_HOSTS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            allowed_origins: vec![],
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
        }
    }
}

fn parse_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn parse_bool(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "true" | "1")
}

impl McpConfig {
    /// Reads `MICROMEGAS_MCP_*` from the process environment.
    pub fn from_env(base_path: &str) -> Self {
        Self::from_lookup(base_path, |key| std::env::var(key).ok())
    }

    /// Same as [`Self::from_env`] with an injectable variable source.
    pub fn from_lookup(base_path: &str, lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mut config = Self {
            path: format!("{}/mcp", base_path.trim_end_matches('/')),
            ..Self::default()
        };
        config.enabled = lookup("MICROMEGAS_MCP_ENABLED")
            .map(|v| parse_bool(&v))
            .unwrap_or(false);
        config.flightsql_url = lookup("MICROMEGAS_MCP_FLIGHTSQL_URL").filter(|v| !v.is_empty());
        let number = |key: &str| {
            let raw = lookup(key)?;
            match raw.trim().parse::<usize>() {
                Ok(n) => Some(n),
                Err(_) => {
                    warn!("Invalid {key} value '{raw}', using default");
                    None
                }
            }
        };
        if let Some(n) = number("MICROMEGAS_MCP_MAX_ROWS") {
            config.max_rows = n;
        }
        if let Some(n) = number("MICROMEGAS_MCP_DEFAULT_ROWS") {
            config.default_rows = n;
        }
        if let Some(n) = number("MICROMEGAS_MCP_MAX_BYTES") {
            config.max_bytes = n;
        }
        config.skills_dir = lookup("MICROMEGAS_MCP_SKILLS_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        if let Some(hosts) = lookup("MICROMEGAS_MCP_ALLOWED_HOSTS") {
            config.allowed_hosts = parse_list(&hosts);
        }
        if let Some(origins) = lookup("MICROMEGAS_MCP_ALLOWED_ORIGINS") {
            config.allowed_origins = parse_list(&origins);
        }
        config.clamp()
    }

    /// Applies the compiled-in ceilings and keeps the defaults inside them.
    pub fn clamp(mut self) -> Self {
        self.max_rows = self.max_rows.clamp(1, ABSOLUTE_MAX_ROWS);
        self.default_rows = self.default_rows.clamp(1, self.max_rows);
        self.max_bytes = self.max_bytes.clamp(1, ABSOLUTE_MAX_BYTES);
        self
    }

    /// Resolves a caller-supplied `limit` to the number of rows to return.
    pub fn effective_limit(&self, requested: Option<usize>) -> usize {
        requested
            .unwrap_or(self.default_rows)
            .clamp(1, self.max_rows)
    }
}
