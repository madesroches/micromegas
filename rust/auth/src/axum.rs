//! Axum middleware for HTTP authentication
//!
//! Provides authentication middleware for Axum HTTP services that:
//! 1. Extracts request parts (headers, method, URI)
//! 2. Validates using configured AuthProvider
//! 3. Injects AuthContext into request extensions
//! 4. Returns 401 Unauthorized on auth failures

use crate::types::{AuthProvider, HttpRequestParts, RequestParts};
use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// Axum middleware for request-based authentication
///
/// This middleware extracts request parts (headers, method, URI),
/// validates them using the provided AuthProvider, and injects the resulting
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
    // Extract request parts for authentication
    let parts = HttpRequestParts {
        headers: req.headers().clone(),
        method: req.method().clone(),
        uri: req.uri().clone(),
    };

    // Validate request using auth provider
    let auth_ctx = auth_provider
        .validate_request(&parts as &dyn RequestParts)
        .await
        .map_err(|e| {
            warn!("[auth_failure] {e}");
            AuthError::InvalidToken
        })?;

    // Log successful authentication (trace level to avoid noise on every request)
    trace!(
        "[auth_success] subject={} email={:?} issuer={} admin={}",
        auth_ctx.subject, auth_ctx.email, auth_ctx.issuer, auth_ctx.is_admin
    );

    // SECURITY: Remove any client-provided auth headers to prevent spoofing
    // These headers should only be trusted when set by the authentication layer
    // The AuthContext in request extensions is the authoritative source
    req.headers_mut().remove("x-auth-subject");
    req.headers_mut().remove("x-auth-email");
    req.headers_mut().remove("x-auth-issuer");
    req.headers_mut().remove("x-allow-delegation");

    // Inject auth context into request extensions for downstream handlers
    req.extensions_mut().insert(auth_ctx);

    // Continue to next middleware/handler
    Ok(next.run(req).await)
}

/// Authentication errors for HTTP responses
#[derive(Debug)]
pub enum AuthError {
    /// Token validation failed
    InvalidToken,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthError::InvalidToken => (StatusCode::UNAUTHORIZED, "Invalid token"),
        };

        (status, message).into_response()
    }
}
