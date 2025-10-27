use micromegas_auth::types::AuthProvider;
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use tonic::{Request, Status};

/// Checks the authentication of a Tonic request using an `AuthProvider`.
///
/// This function checks for a `Bearer` token in the `Authorization` header
/// and validates it using the provided authentication provider.
pub async fn check_auth(
    req: Request<()>,
    auth_provider: &Arc<dyn AuthProvider>,
) -> Result<Request<()>, Status> {
    let metadata = req.metadata();
    let authorization = metadata
        .get(http::header::AUTHORIZATION.as_str())
        .ok_or_else(|| {
            trace!("missing authorization header");
            Status::unauthenticated("missing authorization header")
        })?
        .to_str()
        .map_err(|_e| {
            warn!("error parsing authorization header");
            Status::unauthenticated("error parsing authorization header")
        })?;
    let bearer = "Bearer ";
    if !authorization.starts_with(bearer) {
        warn!("Invalid auth header");
        return Err(Status::unauthenticated("Invalid auth header"));
    }
    let token = &authorization[bearer.len()..];

    let auth_ctx = auth_provider.validate_token(token).await.map_err(|e| {
        warn!("authentication failed: {e}");
        Status::unauthenticated("invalid token")
    })?;

    info!(
        "authenticated: subject={} email={:?} issuer={} admin={}",
        auth_ctx.subject, auth_ctx.email, auth_ctx.issuer, auth_ctx.is_admin
    );

    let mut req = req;
    req.extensions_mut().insert(auth_ctx);
    Ok(req)
}
