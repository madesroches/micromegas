//! Shared auth state: OIDC provider / auth-provider caches, cookie settings.

use super::config::OidcClientConfig;
use anyhow::Result;
use micromegas_auth::oidc::{OidcAuthProvider, OidcConfig};
use micromegas_auth::oidc_client::DiscoveredProvider;
use std::sync::Arc;

/// State for auth endpoints
#[derive(Clone)]
pub struct AuthState {
    /// OIDC provider info (lazy initialized) - for OAuth flow
    pub oidc_provider: Arc<tokio::sync::OnceCell<DiscoveredProvider>>,
    /// OIDC auth provider (lazy initialized) - for JWT validation
    pub auth_provider: Arc<tokio::sync::OnceCell<Arc<OidcAuthProvider>>>,
    /// OIDC client configuration
    pub config: OidcClientConfig,
    /// Cookie domain (optional)
    pub cookie_domain: Option<String>,
    /// Whether we're in production (secure cookies)
    pub secure_cookies: bool,
    /// Secret for signing OAuth state parameters (HMAC-SHA256)
    pub state_signing_secret: Vec<u8>,
    /// Base path for cookies (e.g., "/micromegas"), defaults to "/"
    pub base_path: String,
    /// Environment variable name used to load the OIDC admin list.
    ///
    /// Defaults to `"MICROMEGAS_ADMINS"` for standalone deployments.
    /// The monolith sets this to `"MICROMEGAS_ANALYTICS_ADMINS"` (with fallback
    /// already resolved) so the web role's admin list matches the FlightSQL role.
    pub admin_var_name: String,
}

impl AuthState {
    /// Returns the cookie path, using base_path or "/" if empty
    pub fn cookie_path(&self) -> String {
        if self.base_path.is_empty() {
            "/".to_string()
        } else {
            self.base_path.clone()
        }
    }

    pub async fn get_oidc_provider(&self) -> Result<&DiscoveredProvider> {
        let config = self.config.clone();
        self.oidc_provider
            .get_or_try_init(|| async move {
                // discover() already returns an anyhow error, and every caller
                // (auth_login/auth_callback/auth_refresh) wraps it with
                // "Failed to get OIDC provider" — so return it directly, no extra wrap.
                DiscoveredProvider::discover(
                    &config.issuer,
                    &config.client_id,
                    &config.redirect_uri,
                )
                .await
            })
            .await
    }

    /// Get or initialize the OIDC auth provider for JWT validation.
    ///
    /// The auth provider is lazy-initialized on first use and cached.
    /// The admin list is loaded from the var named by `self.admin_var_name`
    /// (defaults to `MICROMEGAS_ADMINS`; the monolith may set a role-scoped name).
    pub async fn get_auth_provider(&self) -> Result<&Arc<OidcAuthProvider>> {
        let admin_var = self.admin_var_name.clone();
        self.auth_provider
            .get_or_try_init(|| async move {
                let config = OidcConfig::from_env()?;
                let provider = OidcAuthProvider::new(config, &admin_var).await?;
                Ok(Arc::new(provider))
            })
            .await
    }
}
