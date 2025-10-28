//! Authentication providers for Micromegas
//!
//! This crate provides authentication and authorization for Micromegas services.
//! It supports multiple authentication methods:
//!
//! - **API Keys**: Simple bearer token authentication
//! - **OIDC**: OpenID Connect authentication with automatic JWKS caching
//!
//! # Example: API Key Authentication
//!
//! ```rust
//! use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
//! use micromegas_auth::types::AuthProvider;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let json = r#"[{"name": "user1", "key": "secret-key-123"}]"#;
//! let keyring = parse_key_ring(json)?;
//! let provider = ApiKeyAuthProvider::new(keyring);
//!
//! let auth_ctx = provider.validate_token("secret-key-123").await?;
//! println!("Authenticated: {}", auth_ctx.subject);
//! # Ok(())
//! # }
//! ```
//!
//! # Example: OIDC Authentication
//!
//! ```rust,no_run
//! use micromegas_auth::oidc::{OidcAuthProvider, OidcConfig, OidcIssuer};
//! use micromegas_auth::types::AuthProvider;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let config = OidcConfig {
//!     issuers: vec![OidcIssuer {
//!         issuer: "https://accounts.google.com".to_string(),
//!         audience: "your-client-id.apps.googleusercontent.com".to_string(),
//!     }],
//!     jwks_refresh_interval_secs: 3600,
//!     token_cache_size: 1000,
//!     token_cache_ttl_secs: 300,
//! };
//!
//! let provider = OidcAuthProvider::new(config).await?;
//! let auth_ctx = provider.validate_token("id_token_here").await?;
//! println!("Authenticated: {}", auth_ctx.subject);
//! # Ok(())
//! # }
//! ```

/// Core authentication types and traits
pub mod types;

/// API key authentication
pub mod api_key;

/// OIDC authentication with JWKS caching
pub mod oidc;

/// Multi-provider authentication (API key + OIDC)
pub mod multi;

/// Tower service layer for tonic/gRPC authentication
pub mod tower;

/// Axum middleware for HTTP authentication
pub mod axum;

/// Test utilities for generating test tokens
#[cfg(test)]
pub mod test_utils;
