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

/// Shared prefixed-env-var resolution (`{prefix}_{suffix}` with `MICROMEGAS_{suffix}` fallback)
pub mod env;

/// API key authentication
pub mod api_key;

/// DB-backed API key authentication
pub mod db_api_key;

/// OIDC authentication with JWKS caching
pub mod oidc;

/// Canonical login-flow OIDC client construction (discovery + client building)
pub mod oidc_client;

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

/// OAuth state parameter signing and verification
pub mod oauth_state;

/// User attribution validation (prevents impersonation attacks)
pub mod user_attribution;

/// Authorization seam: `MintPolicy`, `ReadPolicy`, and their audience-based implementations
pub mod policy;

/// Generic whole-table snapshot cache shared by the audience-grant and group stores
pub mod db_snapshot;

/// DB-backed audience grant store
pub mod db_audience_grants;

/// Local group membership: `GroupGraph`, the DB-backed store, and closure resolution
pub mod groups;

/// Wraps an `AuthProvider` to resolve transitive local-group membership per request
pub mod membership;
