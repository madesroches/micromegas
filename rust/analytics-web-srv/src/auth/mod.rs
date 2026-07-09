//! Authentication endpoints for analytics-web-srv
//!
//! Implements OIDC authorization code flow with PKCE:
//! - /auth/login - Initiate OIDC login
//! - /auth/callback - Handle OIDC callback
//! - /auth/refresh - Refresh tokens
//! - /auth/logout - Clear session
//! - /auth/me - Get current user info
//!
//! Security: All JWT tokens are fully validated (signature + claims) at this tier
//! using the micromegas-auth crate with JWKS caching. Invalid tokens are rejected
//! before forwarding requests to FlightSQL.
//!
//! This module is split by concern:
//! - [`config`] — web-specific OIDC client configuration.
//! - [`state`] — shared `AuthState` and its lazily-initialized caches.
//! - [`cookies`] — cookie helpers.
//! - [`claims`] — validated-user / JWT claim types and extraction.
//! - [`handlers`] — the Axum handlers, middleware, and extractors.
//!
//! Login-flow OIDC client construction (provider discovery + client
//! building) lives in the `micromegas_auth::oidc_client` crate module, not
//! here — this crate only consumes it.

mod claims;
mod config;
mod cookies;
mod handlers;
mod state;

pub use claims::{UserInfo, ValidatedUser};
pub use config::OidcClientConfig;
pub use cookies::{clear_cookie, create_cookie};
pub use handlers::{
    AdminRequired, AdminUser, AuthApiError, AuthToken, auth_callback, auth_login, auth_logout,
    auth_me, auth_refresh, cookie_auth_middleware, require_admin,
};
pub use state::AuthState;
