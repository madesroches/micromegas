//! Axum middleware for HTTP authentication
//!
//! Provides authentication middleware for Axum HTTP services that:
//! 1. Extracts Bearer token from Authorization header
//! 2. Validates using configured AuthProvider
//! 3. Injects AuthContext into request extensions
//! 4. Returns 401 Unauthorized on auth failures

use crate::types::AuthProvider;
use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use http::header::AUTHORIZATION;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// Axum middleware for bearer token authentication
///
/// This middleware extracts the Bearer token from the Authorization header,
/// validates it using the provided AuthProvider, and injects the resulting
/// AuthContext into the request extensions.
///
/// # Example
///
/// ```rust,ignore
/// use axum::{Router, middleware};
/// use micromegas_auth::axum::auth_middleware;
/// use micromegas_auth::api_key::ApiKeyAuthProvider;
/// use std::sync::Arc;
///
/// let auth_provider = Arc::new(ApiKeyAuthProvider::new(keyring));
/// let app = Router::new()
///     .layer(middleware::from_fn(move |req, next| {
///         auth_middleware(auth_provider.clone(), req, next)
///     }));
/// ```
pub async fn auth_middleware(
    auth_provider: Arc<dyn AuthProvider>,
    mut req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    // Extract authorization header
    let auth_header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or(AuthError::MissingHeader)?;

    // Extract bearer token
    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(AuthError::InvalidFormat)?;

    // Validate token using auth provider
    let auth_ctx = auth_provider.validate_token(token).await.map_err(|e| {
        warn!("authentication failed: {e}");
        AuthError::InvalidToken
    })?;

    // Log successful authentication
    info!(
        "authenticated: subject={} email={:?} issuer={} admin={}",
        auth_ctx.subject, auth_ctx.email, auth_ctx.issuer, auth_ctx.is_admin
    );

    // Inject auth context into request extensions for downstream handlers
    req.extensions_mut().insert(auth_ctx);

    // Continue to next middleware/handler
    Ok(next.run(req).await)
}

/// Authentication errors for HTTP responses
#[derive(Debug)]
pub enum AuthError {
    /// Missing Authorization header
    MissingHeader,
    /// Authorization header doesn't start with "Bearer "
    InvalidFormat,
    /// Token validation failed
    InvalidToken,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthError::MissingHeader => (StatusCode::UNAUTHORIZED, "Missing authorization header"),
            AuthError::InvalidFormat => (
                StatusCode::UNAUTHORIZED,
                "Invalid authorization format, expected: Bearer <token>",
            ),
            AuthError::InvalidToken => (StatusCode::UNAUTHORIZED, "Invalid token"),
        };

        (status, message).into_response()
    }
}
