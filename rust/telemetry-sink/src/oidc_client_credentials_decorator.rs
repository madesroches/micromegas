//! OIDC Client Credentials request decorator for service authentication
//!
//! Implements OAuth 2.0 client credentials flow for service-to-service authentication.
//! Fetches access tokens from OIDC provider and caches them until expiration.

use crate::request_decorator::{RequestDecorator, RequestDecoratorError, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// OIDC token response from client credentials flow
#[derive(Debug, Clone, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: u64, // seconds, defaults to 0 if not present
}

/// Cached token with expiration
#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    expires_at: u64, // Unix timestamp
}

/// Request decorator that uses OIDC client credentials flow
///
/// Fetches access tokens from OIDC provider using client_id + client_secret,
/// caches tokens until expiration, and adds them as Bearer tokens.
pub struct OidcClientCredentialsDecorator {
    token_endpoint: String,
    client_id: String,
    client_secret: String,
    audience: Option<String>,
    buffer_seconds: u64, // Token expiration buffer in seconds
    client: reqwest::Client,
    cached_token: Arc<Mutex<Option<CachedToken>>>,
}

impl OidcClientCredentialsDecorator {
    /// Create from environment variables
    ///
    /// Reads:
    /// - `MICROMEGAS_OIDC_TOKEN_ENDPOINT` - Token endpoint URL
    /// - `MICROMEGAS_OIDC_CLIENT_ID` - Client ID
    /// - `MICROMEGAS_OIDC_CLIENT_SECRET` - Client secret
    /// - `MICROMEGAS_OIDC_AUDIENCE` - Audience (optional, required for Auth0/Azure AD)
    /// - `MICROMEGAS_OIDC_TOKEN_BUFFER_SECONDS` - Token expiration buffer in seconds (optional, default: 180)
    pub fn from_env() -> Result<Self> {
        let token_endpoint = std::env::var("MICROMEGAS_OIDC_TOKEN_ENDPOINT").map_err(|_| {
            RequestDecoratorError::Permanent("MICROMEGAS_OIDC_TOKEN_ENDPOINT not set".to_string())
        })?;

        let client_id = std::env::var("MICROMEGAS_OIDC_CLIENT_ID").map_err(|_| {
            RequestDecoratorError::Permanent("MICROMEGAS_OIDC_CLIENT_ID not set".to_string())
        })?;

        let client_secret = std::env::var("MICROMEGAS_OIDC_CLIENT_SECRET").map_err(|_| {
            RequestDecoratorError::Permanent("MICROMEGAS_OIDC_CLIENT_SECRET not set".to_string())
        })?;

        let audience = std::env::var("MICROMEGAS_OIDC_AUDIENCE").ok();

        let buffer_seconds = std::env::var("MICROMEGAS_OIDC_TOKEN_BUFFER_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(180); // Default: 3 minutes

        Ok(Self::new(
            token_endpoint,
            client_id,
            client_secret,
            audience,
            buffer_seconds,
        ))
    }

    /// Create with explicit credentials
    pub fn new(
        token_endpoint: String,
        client_id: String,
        client_secret: String,
        audience: Option<String>,
        buffer_seconds: u64,
    ) -> Self {
        Self {
            token_endpoint,
            client_id,
            client_secret,
            audience,
            buffer_seconds,
            client: reqwest::Client::new(),
            cached_token: Arc::new(Mutex::new(None)),
        }
    }

    /// Fetch fresh token from OIDC provider
    async fn fetch_token(&self) -> Result<CachedToken> {
        let mut params = vec![
            ("grant_type", "client_credentials"),
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
        ];

        // Add audience if provided (required for Auth0/Azure AD)
        let audience_str;
        if let Some(ref audience) = self.audience {
            audience_str = audience.clone();
            params.push(("audience", audience_str.as_str()));
        }

        let response = self
            .client
            .post(&self.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| {
                RequestDecoratorError::Transient(format!("Failed to fetch token: {}", e))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(RequestDecoratorError::Permanent(format!(
                "Token request failed with status {}: {}",
                status, body
            )));
        }

        let token_response: TokenResponse = response.json().await.map_err(|e| {
            RequestDecoratorError::Permanent(format!("Failed to parse token response: {}", e))
        })?;

        // Calculate expiration time (with buffer)
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_secs();

        // Apply buffer to avoid using tokens near expiration
        let expires_in = token_response
            .expires_in
            .saturating_sub(self.buffer_seconds);
        let expires_at = now + expires_in;

        Ok(CachedToken {
            access_token: token_response.access_token,
            expires_at,
        })
    }

    /// Get valid token (from cache or fetch new)
    async fn get_token(&self) -> Result<String> {
        // Check cache first
        {
            let cached = self.cached_token.lock().await;
            if let Some(token) = &*cached {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("time")
                    .as_secs();
                if token.expires_at > now {
                    // Token still valid
                    return Ok(token.access_token.clone());
                }
            }
        }

        // Token expired or not cached - fetch new one
        let new_token = self.fetch_token().await?;
        let access_token = new_token.access_token.clone();

        // Update cache
        {
            let mut cached = self.cached_token.lock().await;
            *cached = Some(new_token);
        }

        Ok(access_token)
    }
}

#[async_trait]
impl RequestDecorator for OidcClientCredentialsDecorator {
    async fn decorate(&self, request: &mut reqwest::Request) -> Result<()> {
        let token = self.get_token().await?;
        let auth_value = format!("Bearer {}", token);

        request.headers_mut().insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&auth_value).map_err(|e| {
                RequestDecoratorError::Permanent(format!("Invalid token format: {}", e))
            })?,
        );

        Ok(())
    }
}
