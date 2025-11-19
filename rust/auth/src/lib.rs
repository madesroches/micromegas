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
//! use micromegas_auth::types::{AuthProvider, HttpRequestParts, RequestParts};
//!
//! # async fn example() -> anyhow::Result<()> {
//! let json = r#"[{"name": "user1", "key": "secret-key-123"}]"#;
//! let keyring = parse_key_ring(json)?;
//! let provider = ApiKeyAuthProvider::new(keyring);
//!
//! // Create request parts with Bearer token
//! let mut headers = http::HeaderMap::new();
//! headers.insert(
//!     http::header::AUTHORIZATION,
//!     "Bearer secret-key-123".parse().unwrap(),
//! );
//! let parts = HttpRequestParts {
//!     headers,
//!     method: http::Method::GET,
//!     uri: "/api/endpoint".parse().unwrap(),
//! };
//!
//! let auth_ctx = provider.validate_request(&parts as &dyn RequestParts).await?;
//! println!("Authenticated: {}", auth_ctx.subject);
//! # Ok(())
//! # }
//! ```
//!
//! # Example: OIDC Authentication
//!
//! ```rust,no_run
//! use micromegas_auth::oidc::{OidcAuthProvider, OidcConfig, OidcIssuer};
//! use micromegas_auth::types::{AuthProvider, HttpRequestParts, RequestParts};
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
//!
//! // Create request parts with ID token
//! let mut headers = http::HeaderMap::new();
//! headers.insert(
//!     http::header::AUTHORIZATION,
//!     "Bearer id_token_here".parse().unwrap(),
//! );
//! let parts = HttpRequestParts {
//!     headers,
//!     method: http::Method::GET,
//!     uri: "/api/endpoint".parse().unwrap(),
//! };
//!
//! let auth_ctx = provider.validate_request(&parts as &dyn RequestParts).await?;
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

/// Default authentication provider initialization
pub mod default_provider;

/// Tower service layer for tonic/gRPC authentication
pub mod tower;

/// Axum middleware for HTTP authentication
pub mod axum;

/// URL validation utilities for authentication flows
pub mod url_validation;
