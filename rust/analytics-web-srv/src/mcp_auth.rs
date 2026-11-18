//! Bearer-token validation for the `/mcp` route.
//!
//! The web app's session validator is OIDC-only and cookie-based; the MCP route accepts the same
//! credentials as FlightSQL (OIDC tokens and analytics API keys) so that a token the agent holds
//! works on both hops.

use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use micromegas::auth::axum::{AuthError, authenticate};
use micromegas::auth::types::AuthProvider;
use std::sync::Arc;

const CHALLENGE: &str = "Bearer realm=\"micromegas\"";

fn unauthorized() -> Response {
    let mut response = (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static(CHALLENGE),
    );
    response
}

/// Rejects requests without a valid bearer token (401 with `WWW-Authenticate`, or 503 when a
/// credential store is unreachable) and puts the resulting `AuthContext` in the request
/// extensions for the MCP tools.
pub async fn mcp_bearer_middleware(
    State(provider): State<Arc<dyn AuthProvider>>,
    mut req: Request,
    next: Next,
) -> Response {
    let has_bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Bearer ") && v.len() > "Bearer ".len());
    if !has_bearer {
        return unauthorized();
    }
    match authenticate(provider.as_ref(), &mut req).await {
        Ok(()) => next.run(req).await,
        Err(AuthError::InvalidToken) => unauthorized(),
        Err(AuthError::Unavailable) => AuthError::Unavailable.into_response(),
    }
}
