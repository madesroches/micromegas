use micromegas_auth::types::{AuthProvider, GrpcRequestParts, RequestParts};
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use tonic::{Request, Status};

/// Checks the authentication of a Tonic request using an `AuthProvider`.
///
/// This function extracts request parts from the gRPC metadata
/// and validates them using the provided authentication provider.
pub async fn check_auth(
    req: Request<()>,
    auth_provider: &Arc<dyn AuthProvider>,
) -> Result<Request<()>, Status> {
    let metadata = req.metadata();

    let parts = GrpcRequestParts {
        metadata: metadata.clone(),
    };

    let auth_ctx = auth_provider
        .validate_request(&parts as &dyn RequestParts)
        .await
        .map_err(|e| {
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
