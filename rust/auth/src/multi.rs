//! Multi-provider authentication that tries multiple auth methods in sequence.

use crate::api_key::ApiKeyAuthProvider;
use crate::oidc::OidcAuthProvider;
use crate::types::{AuthContext, AuthProvider};
use async_trait::async_trait;
use std::sync::Arc;

/// Multi-provider authentication that tries API key first, then OIDC.
///
/// This provider allows supporting multiple authentication methods simultaneously.
/// It tries providers in order until one succeeds:
/// 1. API key (fast O(1) HashMap lookup)
/// 2. OIDC (slower JWT validation with JWKS)
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
/// // Create multi-provider
/// let multi = MultiAuthProvider {
///     api_key_provider: Some(api_key_provider),
///     oidc_provider: Some(oidc_provider),
/// };
/// # Ok(())
/// # }
/// ```
pub struct MultiAuthProvider {
    /// Optional API key provider (tried first)
    pub api_key_provider: Option<Arc<ApiKeyAuthProvider>>,
    /// Optional OIDC provider (tried second)
    pub oidc_provider: Option<Arc<OidcAuthProvider>>,
}

#[async_trait]
impl AuthProvider for MultiAuthProvider {
    async fn validate_token(&self, token: &str) -> anyhow::Result<AuthContext> {
        // Try API key authentication first (fast path)
        if let Some(api_key_provider) = &self.api_key_provider
            && let Ok(auth_ctx) = api_key_provider.validate_token(token).await
        {
            return Ok(auth_ctx);
        }

        // Try OIDC authentication (slower, involves JWT validation)
        if let Some(oidc_provider) = &self.oidc_provider
            && let Ok(auth_ctx) = oidc_provider.validate_token(token).await
        {
            return Ok(auth_ctx);
        }

        anyhow::bail!("authentication failed with all providers")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_key::parse_key_ring;

    #[tokio::test]
    async fn test_multi_provider_api_key() {
        let keyring = parse_key_ring(r#"[{"name": "test", "key": "secret"}]"#).unwrap();
        let api_key_provider = Arc::new(ApiKeyAuthProvider::new(keyring));

        let multi = MultiAuthProvider {
            api_key_provider: Some(api_key_provider),
            oidc_provider: None,
        };

        let result = multi.validate_token("secret").await;
        assert!(result.is_ok());
        let auth_ctx = result.unwrap();
        assert_eq!(auth_ctx.subject, "test");
    }

    #[tokio::test]
    async fn test_multi_provider_no_providers() {
        let multi = MultiAuthProvider {
            api_key_provider: None,
            oidc_provider: None,
        };

        let result = multi.validate_token("any-token").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_multi_provider_invalid_token() {
        let keyring = parse_key_ring(r#"[{"name": "test", "key": "secret"}]"#).unwrap();
        let api_key_provider = Arc::new(ApiKeyAuthProvider::new(keyring));

        let multi = MultiAuthProvider {
            api_key_provider: Some(api_key_provider),
            oidc_provider: None,
        };

        let result = multi.validate_token("wrong-token").await;
        assert!(result.is_err());
    }
}
