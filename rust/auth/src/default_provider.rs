//! Default authentication provider initialization for Micromegas services.
//!
//! This module provides the standard way to initialize authentication with both
//! API key and OIDC providers from environment variables.

use crate::api_key::{ApiKeyAuthProvider, parse_key_ring};
use crate::multi::MultiAuthProvider;
use crate::oidc::{OidcAuthProvider, OidcConfig};
use crate::types::AuthProvider;
use micromegas_tracing::info;
use std::sync::Arc;

/// Initializes the default authentication provider with API key and OIDC from environment.
///
/// Reads configuration from:
/// - `MICROMEGAS_API_KEYS`: JSON array of API keys
/// - `MICROMEGAS_OIDC_CONFIG`: OIDC configuration JSON
/// - `MICROMEGAS_ADMINS`: JSON array of admin user emails/subjects
///
/// Returns `Ok(Some(...))` if at least one provider is configured.
/// Returns `Ok(None)` if no providers are configured (auth disabled).
/// Returns `Err` on configuration errors.
///
/// # Example
///
/// ```rust,no_run
/// use micromegas_auth::default_provider::provider;
///
/// # async fn example() -> anyhow::Result<()> {
/// let auth_provider = provider().await?;
/// if let Some(provider) = auth_provider {
///     println!("Authentication enabled");
/// } else {
///     println!("No authentication configured");
/// }
/// # Ok(())
/// # }
/// ```
pub async fn provider() -> anyhow::Result<Option<Arc<dyn AuthProvider>>> {
    provider_with_prefix("").await
}

/// Initializes auth providers using env vars scoped to a prefix.
///
/// For prefix `"MICROMEGAS_INGESTION"`:
/// - API keys: tries `MICROMEGAS_INGESTION_API_KEYS`, falls back to `MICROMEGAS_API_KEYS`
/// - OIDC:     tries `MICROMEGAS_INGESTION_OIDC_CONFIG`, falls back to `MICROMEGAS_OIDC_CONFIG`
/// - Admins:   tries `MICROMEGAS_INGESTION_ADMINS`, falls back to `MICROMEGAS_ADMINS`
///
/// With an empty prefix the behaviour is identical to [`provider`].
pub async fn provider_with_prefix(prefix: &str) -> anyhow::Result<Option<Arc<dyn AuthProvider>>> {
    // Resolve API keys var with fallback
    let api_keys_json = if prefix.is_empty() {
        std::env::var("MICROMEGAS_API_KEYS").ok()
    } else {
        std::env::var(format!("{prefix}_API_KEYS"))
            .or_else(|_| std::env::var("MICROMEGAS_API_KEYS"))
            .ok()
    };

    // Resolve OIDC config var with fallback
    let oidc_config_var: String = if prefix.is_empty() {
        "MICROMEGAS_OIDC_CONFIG".to_string()
    } else if std::env::var(format!("{prefix}_OIDC_CONFIG")).is_ok() {
        format!("{prefix}_OIDC_CONFIG")
    } else {
        "MICROMEGAS_OIDC_CONFIG".to_string()
    };

    // Resolve admin users var with fallback
    let admin_var: String = if prefix.is_empty() {
        "MICROMEGAS_ADMINS".to_string()
    } else {
        let prefixed = format!("{prefix}_ADMINS");
        if std::env::var(&prefixed).is_ok() {
            prefixed
        } else {
            "MICROMEGAS_ADMINS".to_string()
        }
    };

    // Initialize API key provider if configured
    let api_key_provider = if let Some(keys_json) = api_keys_json {
        let keyring = parse_key_ring(&keys_json)?;
        info!("API key authentication enabled");
        Some(Arc::new(ApiKeyAuthProvider::new(keyring)) as Arc<dyn AuthProvider>)
    } else {
        info!("API key auth not configured");
        None
    };

    // Initialize OIDC provider if configured
    let oidc_provider = match OidcConfig::from_env_var(&oidc_config_var) {
        Ok(config) => {
            info!("Initializing OIDC authentication");
            Some(Arc::new(OidcAuthProvider::new(config, &admin_var).await?) as Arc<dyn AuthProvider>)
        }
        Err(e) => {
            info!("OIDC not configured ({e}) - OIDC auth disabled");
            None
        }
    };

    // Build multi-provider from available providers
    let mut multi = MultiAuthProvider::new();
    if let Some(p) = api_key_provider {
        multi = multi.with_provider(p);
    }
    if let Some(p) = oidc_provider {
        multi = multi.with_provider(p);
    }

    // Return None if no providers configured
    if multi.is_empty() {
        return Ok(None);
    }

    Ok(Some(Arc::new(multi) as Arc<dyn AuthProvider>))
}
