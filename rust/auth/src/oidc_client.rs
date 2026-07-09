//! Canonical login-flow OIDC client construction.
//!
//! This module owns discovery of an OIDC provider's metadata and
//! construction of the `openidconnect` client used for the authorization
//! code (login) flow. It is the single place where this happens; consumers
//! (e.g. `analytics-web-srv`) should discover a provider once, cache the
//! resulting [`DiscoveredProvider`], and call [`DiscoveredProvider::build_client`]
//! whenever they need a configured client.
//!
//! This is deliberately separate from `crate::oidc`'s internal
//! `discover_async` call used to fetch JWKS for JWT validation: the two serve
//! different lazy caches for different purposes (login-flow client vs. JWKS
//! validation).

use crate::oidc::create_http_client;
use anyhow::{Result, anyhow};
use openidconnect::core::{CoreClient, CoreProviderMetadata};
use openidconnect::{ClientId, IssuerUrl, RedirectUrl};
use std::sync::Arc;

/// Fully-parameterized openidconnect client with endpoints set from
/// discovered provider metadata (public client, PKCE — no client secret).
pub type ConfiguredCoreClient = openidconnect::Client<
    openidconnect::EmptyAdditionalClaims,
    openidconnect::core::CoreAuthDisplay,
    openidconnect::core::CoreGenderClaim,
    openidconnect::core::CoreJweContentEncryptionAlgorithm,
    openidconnect::core::CoreJsonWebKey,
    openidconnect::core::CoreAuthPrompt,
    openidconnect::StandardErrorResponse<openidconnect::core::CoreErrorResponseType>,
    openidconnect::core::CoreTokenResponse,
    openidconnect::core::CoreTokenIntrospectionResponse,
    openidconnect::core::CoreRevocableToken,
    openidconnect::core::CoreRevocationErrorResponse,
    openidconnect::EndpointSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointMaybeSet,
    openidconnect::EndpointMaybeSet,
>;

/// A discovered OIDC provider, ready to build login-flow clients from.
///
/// This is the single canonical home for provider discovery + client
/// construction. `analytics-web-srv` caches one of these in a `OnceCell`
/// and calls `build_client()` for the authorization-code flow.
pub struct DiscoveredProvider {
    pub metadata: Arc<CoreProviderMetadata>,
    pub client_id: ClientId,
    pub redirect_uri: RedirectUrl,
}

impl DiscoveredProvider {
    /// Discover provider metadata for `issuer` and remember the client id /
    /// redirect uri needed to build clients. Uses the SSRF-hardened HTTP client.
    pub async fn discover(issuer: &str, client_id: &str, redirect_uri: &str) -> Result<Self> {
        let issuer_url =
            IssuerUrl::new(issuer.to_string()).map_err(|e| anyhow!("Invalid issuer URL: {e:?}"))?;
        let http_client = create_http_client()?;
        let metadata = CoreProviderMetadata::discover_async(issuer_url, &http_client)
            .await
            .map_err(|e| anyhow!("Failed to discover OIDC provider: {e:?}"))?;
        let redirect_uri = RedirectUrl::new(redirect_uri.to_string())
            .map_err(|e| anyhow!("Invalid redirect URI: {e:?}"))?;
        Ok(Self {
            metadata: Arc::new(metadata),
            client_id: ClientId::new(client_id.to_string()),
            redirect_uri,
        })
    }

    /// Build a configured login-flow client (public client, PKCE) from the
    /// discovered metadata.
    pub fn build_client(&self) -> ConfiguredCoreClient {
        CoreClient::from_provider_metadata(
            (*self.metadata).clone(),
            self.client_id.clone(),
            None, // public client with PKCE — no secret
        )
        .set_redirect_uri(self.redirect_uri.clone())
    }
}
