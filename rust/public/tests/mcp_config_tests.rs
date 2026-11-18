use micromegas::servers::mcp::config::{
    ABSOLUTE_MAX_BYTES, ABSOLUTE_MAX_ROWS, DEFAULT_MAX_BYTES, DEFAULT_ROWS, McpConfig,
};
use std::collections::HashMap;

fn config(vars: &[(&str, &str)]) -> McpConfig {
    let map: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    McpConfig::from_lookup("/base", |k| map.get(k).cloned())
}

#[test]
fn defaults() {
    let c = config(&[]);
    assert!(!c.enabled);
    assert_eq!(c.path, "/base/mcp");
    assert_eq!(c.default_rows, DEFAULT_ROWS);
    assert_eq!(c.max_bytes, DEFAULT_MAX_BYTES);
    assert!(c.allowed_origins.is_empty());
}

#[test]
fn enabled_flag_and_lists() {
    let c = config(&[
        ("MICROMEGAS_MCP_ENABLED", "true"),
        ("MICROMEGAS_MCP_ALLOWED_HOSTS", "a.example, b.example:8080"),
        ("MICROMEGAS_MCP_ALLOWED_ORIGINS", "https://x.example"),
    ]);
    assert!(c.enabled);
    assert_eq!(c.allowed_hosts, vec!["a.example", "b.example:8080"]);
    assert_eq!(c.allowed_origins, vec!["https://x.example"]);
}

#[test]
fn hard_maxima_clamp_larger_values() {
    let c = config(&[
        ("MICROMEGAS_MCP_MAX_ROWS", "999999999"),
        ("MICROMEGAS_MCP_DEFAULT_ROWS", "999999999"),
        ("MICROMEGAS_MCP_MAX_BYTES", "999999999"),
    ]);
    assert_eq!(c.max_rows, ABSOLUTE_MAX_ROWS);
    assert_eq!(c.default_rows, ABSOLUTE_MAX_ROWS);
    assert_eq!(c.max_bytes, ABSOLUTE_MAX_BYTES);
}

#[test]
fn limit_defaults_and_clamps() {
    let c = config(&[("MICROMEGAS_MCP_MAX_ROWS", "500")]);
    assert_eq!(c.default_rows, 500);
    assert_eq!(c.effective_limit(None), 500);
    assert_eq!(c.effective_limit(Some(20)), 20);
    assert_eq!(c.effective_limit(Some(100_000)), 500);
    assert_eq!(c.effective_limit(Some(0)), 1);
}
