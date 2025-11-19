//! Configuration for HTTP gateway header forwarding

use anyhow::{Context, Result};
use serde::Deserialize;

/// Configuration for forwarding HTTP headers to FlightSQL backend
#[derive(Debug, Clone, Deserialize)]
pub struct HeaderForwardingConfig {
    /// Exact header names to forward (case-insensitive)
    pub allowed_headers: Vec<String>,

    /// Header prefixes to forward (e.g., "X-Custom-")
    pub allowed_prefixes: Vec<String>,

    /// Headers to explicitly block (overrides allows)
    pub blocked_headers: Vec<String>,
}

impl Default for HeaderForwardingConfig {
    fn default() -> Self {
        Self {
            // Default safe headers to forward
            allowed_headers: vec![
                "Authorization".to_string(),
                "User-Agent".to_string(),
                "X-Client-Type".to_string(),
                "X-Correlation-ID".to_string(),
                "X-Request-ID".to_string(),
                "X-User-Email".to_string(),
                "X-User-ID".to_string(),
                "X-User-Name".to_string(),
            ],
            allowed_prefixes: vec![],
            blocked_headers: vec![
                "Cookie".to_string(),
                "Set-Cookie".to_string(),
                // SECURITY: Gateway always sets this from actual connection
                "X-Client-IP".to_string(),
            ],
        }
    }
}

impl HeaderForwardingConfig {
    /// Load configuration from environment variable or use defaults
    pub fn from_env() -> Result<Self> {
        if let Ok(config_json) = std::env::var("MICROMEGAS_GATEWAY_HEADERS") {
            serde_json::from_str(&config_json)
                .context("Failed to parse MICROMEGAS_GATEWAY_HEADERS")
        } else {
            Ok(Self::default())
        }
    }

    /// Check if a header should be forwarded based on configuration
    pub fn should_forward(&self, header_name: &str) -> bool {
        let name_lower = header_name.to_lowercase();

        // Check blocked list first
        if self
            .blocked_headers
            .iter()
            .any(|h| h.to_lowercase() == name_lower)
        {
            return false;
        }

        // Check exact matches
        if self
            .allowed_headers
            .iter()
            .any(|h| h.to_lowercase() == name_lower)
        {
            return true;
        }

        // Check prefixes
        self.allowed_prefixes
            .iter()
            .any(|prefix| name_lower.starts_with(&prefix.to_lowercase()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HeaderForwardingConfig::default();

        // Should forward allowed headers
        assert!(config.should_forward("Authorization"));
        assert!(config.should_forward("authorization")); // case-insensitive
        assert!(config.should_forward("X-Request-ID"));
        assert!(config.should_forward("X-User-ID"));

        // Should block blocked headers
        assert!(!config.should_forward("Cookie"));
        assert!(!config.should_forward("Set-Cookie"));
        assert!(!config.should_forward("X-Client-IP"));

        // Should not forward unlisted headers
        assert!(!config.should_forward("X-Custom-Header"));
    }

    #[test]
    fn test_prefix_matching() {
        let config = HeaderForwardingConfig {
            allowed_headers: vec![],
            allowed_prefixes: vec!["X-Custom-".to_string(), "X-Tenant-".to_string()],
            blocked_headers: vec![],
        };

        assert!(config.should_forward("X-Custom-Auth"));
        assert!(config.should_forward("x-custom-auth")); // case-insensitive
        assert!(config.should_forward("X-Tenant-ID"));
        assert!(!config.should_forward("X-Other-Header"));
    }

    #[test]
    fn test_blocked_overrides_allowed() {
        let config = HeaderForwardingConfig {
            allowed_headers: vec!["X-Special".to_string()],
            allowed_prefixes: vec!["X-".to_string()],
            blocked_headers: vec!["X-Special".to_string()],
        };

        // Blocked should override allowed
        assert!(!config.should_forward("X-Special"));

        // Other X- headers should still work
        assert!(config.should_forward("X-Other"));
    }

    #[test]
    fn test_case_insensitive() {
        let config = HeaderForwardingConfig::default();

        assert!(config.should_forward("authorization"));
        assert!(config.should_forward("AUTHORIZATION"));
        assert!(config.should_forward("Authorization"));

        assert!(!config.should_forward("cookie"));
        assert!(!config.should_forward("COOKIE"));
        assert!(!config.should_forward("Cookie"));
    }
}
