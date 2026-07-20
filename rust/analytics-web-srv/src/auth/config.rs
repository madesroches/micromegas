//! Web-specific OIDC client configuration.

use anyhow::{Result, anyhow};
use micromegas::tracing::prelude::*;
use serde::Deserialize;

/// OIDC client configuration
#[derive(Debug, Clone, Deserialize)]
pub struct OidcClientConfig {
    /// OIDC provider issuer URL
    pub issuer: String,
    /// Client ID (public client)
    pub client_id: String,
    /// Redirect URI for callback
    pub redirect_uri: String,
}

impl OidcClientConfig {
    /// Load configuration from environment variables
    ///
    /// Required environment variables:
    /// - MICROMEGAS_OIDC_CONFIG: JSON with "issuers" array (same format as FlightSQL server)
    /// - MICROMEGAS_AUTH_REDIRECT_URI: OAuth callback URL
    ///
    /// Expected MICROMEGAS_OIDC_CONFIG format (uses micromegas-auth's OidcConfig):
    /// {
    ///   "issuers": [
    ///     {
    ///       "issuer": "https://...",
    ///       "audience": "client-id"
    ///     }
    ///   ]
    /// }
    ///
    /// Note: When multiple issuers are configured, the first issuer is used for
    /// the OAuth login flow (you can only redirect to one provider). Token
    /// validation via OidcAuthProvider will accept tokens from any configured issuer.
    pub fn from_env() -> Result<Self> {
        // Use the shared OidcConfig from micromegas-auth
        let config = micromegas::auth::oidc::OidcConfig::from_env()?;

        // Need at least one issuer
        if config.issuers.is_empty() {
            return Err(anyhow!(
                "MICROMEGAS_OIDC_CONFIG must contain at least one issuer in the 'issuers' array"
            ));
        }

        // Use the first issuer for OAuth login flow
        // (token validation via OidcAuthProvider supports all issuers)
        let issuer_config = &config.issuers[0];

        if config.issuers.len() > 1 {
            info!(
                "Multiple OIDC issuers configured ({}). Using '{}' for OAuth login flow. \
                 Token validation will accept tokens from all configured issuers.",
                config.issuers.len(),
                issuer_config.issuer
            );
        }

        let redirect_uri = std::env::var("MICROMEGAS_AUTH_REDIRECT_URI")
            .map_err(|_| anyhow!("MICROMEGAS_AUTH_REDIRECT_URI environment variable not set"))?;

        Ok(OidcClientConfig {
            issuer: issuer_config.issuer.clone(),
            client_id: issuer_config.audience.clone(),
            redirect_uri,
        })
    }
}
