//! Shared auth state: OIDC provider / auth-provider caches, cookie settings.

use super::config::OidcClientConfig;
use anyhow::Result;
use micromegas::auth::groups::DbGroupsSource;
use micromegas::auth::membership::MembershipProvider;
use micromegas::auth::oidc::{OidcAuthProvider, OidcConfig};
use micromegas::auth::oidc_client::DiscoveredProvider;
use micromegas::auth::types::AuthProvider;
use std::sync::Arc;

/// State for auth endpoints
#[derive(Clone)]
pub struct AuthState {
    /// OIDC provider info (lazy initialized) - for OAuth flow
    pub oidc_provider: Arc<tokio::sync::OnceCell<DiscoveredProvider>>,
    /// Auth provider (lazy initialized) - for JWT validation, wrapped in a
    /// [`MembershipProvider`] over `groups` so admin-ness and group grants resolve from the
    /// `admins` group rather than an env var.
    pub auth_provider: Arc<tokio::sync::OnceCell<Arc<dyn AuthProvider>>>,
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
    /// The local-group snapshot store `get_auth_provider`'s lazy init wraps `OidcAuthProvider`
    /// in a `MembershipProvider` over. Mirrors `MembershipProvider.groups` -- `AuthState` needs
    /// its own copy since the provider itself is built lazily, on first use, not at
    /// `AuthState` construction time.
    pub groups: Arc<DbGroupsSource>,
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

    /// Get or initialize the auth provider for JWT validation, wrapped in a
    /// [`MembershipProvider`] so `AuthContext.memberships`/`is_admin()` resolve from the
    /// `admins` group rather than an env var.
    ///
    /// The auth provider is lazy-initialized on first use and cached.
    pub async fn get_auth_provider(&self) -> Result<&Arc<dyn AuthProvider>> {
        let groups = self.groups.clone();
        self.auth_provider
            .get_or_try_init(|| async move {
                let config = OidcConfig::from_env()?;
                let provider = OidcAuthProvider::new(config).await?;
                Ok(Arc::new(MembershipProvider::new(
                    Arc::new(provider) as Arc<dyn AuthProvider>,
                    groups,
                )) as Arc<dyn AuthProvider>)
            })
            .await
    }
}
