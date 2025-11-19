//! Tower service layer for async authentication with tonic/gRPC.
//!
//! This module provides a tower service wrapper that integrates authentication
//! into tonic gRPC services. It extracts request parts from gRPC metadata,
//! validates them using an AuthProvider, and injects the AuthContext into request extensions.

use crate::types::{AuthProvider, GrpcRequestParts, RequestParts};
use futures::future::BoxFuture;
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use tonic::Status;
use tower::Service;

/// Async authentication service wrapper for tonic/gRPC.
///
/// This service wraps another tower service and adds authentication:
/// 1. Extracts request parts from gRPC metadata
/// 2. Validates request using the configured AuthProvider
/// 3. Injects AuthContext into request extensions
/// 4. Logs authentication success/failure
///
/// If no auth_provider is configured, requests pass through without authentication.
///
/// # Example
///
/// ```rust,no_run
/// use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
/// use micromegas_auth::tower::AuthService;
/// use std::sync::Arc;
///
/// # async fn example<S>(inner_service: S) -> anyhow::Result<()>
/// # where
/// #     S: tower::Service<http::Request<tonic::body::Body>> + Clone + Send + 'static,
/// #     S::Response: 'static,
/// #     S::Future: Send + 'static,
/// #     S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
/// # {
/// // Create auth provider
/// let keyring = parse_key_ring(r#"[{"name": "test", "key": "secret"}]"#)?;
/// let auth_provider = Arc::new(ApiKeyAuthProvider::new(keyring));
///
/// // Wrap your service with authentication
/// let auth_service = AuthService {
///     inner: inner_service,
///     auth_provider: Some(auth_provider as Arc<dyn micromegas_auth::types::AuthProvider>),
/// };
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct AuthService<S> {
    /// The inner service to wrap
    pub inner: S,
    /// Optional authentication provider (None = no auth required)
    pub auth_provider: Option<Arc<dyn AuthProvider>>,
}

impl<S> Service<http::Request<tonic::body::Body>> for AuthService<S>
where
    S: Service<http::Request<tonic::body::Body>> + Clone + Send + 'static,
    S::Response: 'static,
    S::Future: Send + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Response = S::Response;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: http::Request<tonic::body::Body>) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let auth_provider = self.auth_provider.clone();

        Box::pin(async move {
            if let Some(provider) = auth_provider {
                let (mut parts, body) = req.into_parts();

                // Extract request parts for validation
                let request_parts = GrpcRequestParts {
                    metadata: tonic::metadata::MetadataMap::from_headers(parts.headers.clone()),
                };

                // Validate request
                match provider
                    .validate_request(&request_parts as &dyn RequestParts)
                    .await
                {
                    Ok(auth_ctx) => {
                        info!(
                            "authenticated: subject={} email={:?} issuer={} admin={}",
                            auth_ctx.subject, auth_ctx.email, auth_ctx.issuer, auth_ctx.is_admin
                        );

                        // SECURITY: Remove any client-provided auth headers to prevent spoofing
                        // These headers are only set by the authentication layer
                        parts.headers.remove("x-auth-subject");
                        parts.headers.remove("x-auth-email");
                        parts.headers.remove("x-auth-issuer");
                        parts.headers.remove("x-allow-delegation");

                        // Inject auth context into gRPC metadata headers
                        parts.headers.insert(
                            "x-auth-subject",
                            http::HeaderValue::from_str(&auth_ctx.subject)
                                .expect("valid user id header"),
                        );
                        if let Some(email) = &auth_ctx.email {
                            parts.headers.insert(
                                "x-auth-email",
                                http::HeaderValue::from_str(email).expect("valid email header"),
                            );
                        }
                        parts.headers.insert(
                            "x-auth-issuer",
                            http::HeaderValue::from_str(&auth_ctx.issuer)
                                .expect("valid issuer header"),
                        );
                        parts.headers.insert(
                            "x-allow-delegation",
                            http::HeaderValue::from_str(&auth_ctx.allow_delegation.to_string())
                                .expect("valid allow_delegation header"),
                        );

                        parts.extensions.insert(auth_ctx);
                        let req = http::Request::from_parts(parts, body);
                        inner.call(req).await.map_err(Into::into)
                    }
                    Err(e) => {
                        warn!("authentication failed: {e}");
                        Err(Box::new(Status::unauthenticated("invalid token"))
                            as Box<dyn std::error::Error + Send + Sync>)
                    }
                }
            } else {
                inner.call(req).await.map_err(Into::into)
            }
        })
    }
}
