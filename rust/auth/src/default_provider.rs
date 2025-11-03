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
/// - `MICROMEGAS_OIDC_*`: OIDC configuration (see `OidcConfig::from_env`)
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
    // Initialize API key provider if configured
    let api_key_provider = match std::env::var("MICROMEGAS_API_KEYS") {
        Ok(keys_json) => {
            let keyring = parse_key_ring(&keys_json)?;
            info!("API key authentication enabled");
            Some(Arc::new(ApiKeyAuthProvider::new(keyring)) as Arc<dyn AuthProvider>)
        }
        Err(_) => {
            info!("MICROMEGAS_API_KEYS not set - API key auth disabled");
            None
        }
    };

    // Initialize OIDC provider if configured
    let oidc_provider = match OidcConfig::from_env() {
        Ok(config) => {
            info!("Initializing OIDC authentication");
            Some(Arc::new(OidcAuthProvider::new(config).await?) as Arc<dyn AuthProvider>)
        }
        Err(e) => {
            info!("OIDC not configured ({e}) - OIDC auth disabled");
            None
        }
    };

    // Build multi-provider from available providers
    let mut multi = MultiAuthProvider::new();
    if let Some(provider) = api_key_provider {
        multi = multi.with_provider(provider);
    }
    if let Some(provider) = oidc_provider {
        multi = multi.with_provider(provider);
    }

    // Return None if no providers configured
    if multi.is_empty() {
        return Ok(None);
    }

    Ok(Some(Arc::new(multi) as Arc<dyn AuthProvider>))
}
