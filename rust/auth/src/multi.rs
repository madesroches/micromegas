//! Multi-provider authentication that tries multiple auth methods in sequence.

use crate::types::{AuthContext, AuthProvider};
use async_trait::async_trait;
use std::sync::Arc;

/// Multi-provider authentication that tries providers in order until one succeeds.
///
/// This provider allows supporting multiple authentication methods simultaneously
/// and enables adding custom enterprise authentication providers. Providers are
/// tried in the order they were added via `with_provider()`.
///
/// Provider order matters for authentication precedence - the first successful
/// match wins. Typically, you want faster providers (like API key) before slower
/// ones (like OIDC JWT validation).
///
/// # Example
///
/// ```rust,no_run
/// use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
/// use micromegas_auth::oidc::{OidcAuthProvider, OidcConfig, OidcIssuer};
/// use micromegas_auth::multi::MultiAuthProvider;
/// use std::sync::Arc;
///
/// # async fn example() -> anyhow::Result<()> {
/// // Set up API key provider
/// let keyring = parse_key_ring(r#"[{"name": "test", "key": "secret"}]"#)?;
/// let api_key_provider = Arc::new(ApiKeyAuthProvider::new(keyring));
///
/// // Set up OIDC provider
/// let oidc_config = OidcConfig {
///     issuers: vec![OidcIssuer {
///         issuer: "https://accounts.google.com".to_string(),
///         audience: "your-app.apps.googleusercontent.com".to_string(),
///     }],
///     jwks_refresh_interval_secs: 3600,
///     token_cache_size: 1000,
///     token_cache_ttl_secs: 300,
/// };
/// let oidc_provider = Arc::new(OidcAuthProvider::new(oidc_config).await?);
///
/// // Create multi-provider with builder pattern
/// let multi = MultiAuthProvider::new()
///     .with_provider(api_key_provider)
///     .with_provider(oidc_provider);
/// // .with_provider(Arc::new(MyEnterpriseAuthProvider::new())); // Custom provider!
/// # Ok(())
/// # }
/// ```
pub struct MultiAuthProvider {
    providers: Vec<Arc<dyn AuthProvider>>,
}

impl MultiAuthProvider {
    /// Creates a new empty MultiAuthProvider.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Adds a provider to the authentication chain.
    ///
    /// Providers are tried in the order they are added. Returns self for chaining.
    pub fn with_provider(mut self, provider: Arc<dyn AuthProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    /// Returns true if no providers are configured.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

#[async_trait]
impl AuthProvider for MultiAuthProvider {
    async fn validate_request(
        &self,
        parts: &dyn crate::types::RequestParts,
    ) -> anyhow::Result<AuthContext> {
        for provider in &self.providers {
            if let Ok(auth_ctx) = provider.validate_request(parts).await {
                return Ok(auth_ctx);
            }
        }
        anyhow::bail!("authentication failed with all providers")
    }
}
