//! Tower service layer for async authentication with tonic/gRPC.
//!
//! This module provides a tower service wrapper that integrates authentication
//! into tonic gRPC services. It extracts Bearer tokens from requests, validates
//! them using an AuthProvider, and injects the AuthContext into request extensions.

use crate::types::AuthProvider;
use futures::future::BoxFuture;
use http::header::AUTHORIZATION;
use std::sync::Arc;
use tonic::Status;
use tower::Service;

/// Async authentication service wrapper for tonic/gRPC.
///
/// This service wraps another tower service and adds authentication:
/// 1. Extracts Bearer token from Authorization header
/// 2. Validates token using the configured AuthProvider
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

                // Extract and validate token
                let authorization = parts
                    .headers
                    .get(AUTHORIZATION)
                    .and_then(|h| h.to_str().ok())
                    .and_then(|h| h.strip_prefix("Bearer "))
                    .ok_or_else(|| {
                        log::trace!("missing or invalid authorization header");
                        Box::new(Status::unauthenticated("missing authorization header"))
                            as Box<dyn std::error::Error + Send + Sync>
                    });

                match authorization {
                    Ok(token) => match provider.validate_token(token).await {
                        Ok(auth_ctx) => {
                            log::info!(
                                "authenticated: subject={} email={:?} issuer={} admin={}",
                                auth_ctx.subject,
                                auth_ctx.email,
                                auth_ctx.issuer,
                                auth_ctx.is_admin
                            );
                            parts.extensions.insert(auth_ctx);
                            let req = http::Request::from_parts(parts, body);
                            inner.call(req).await.map_err(Into::into)
                        }
                        Err(e) => {
                            log::warn!("authentication failed: {e}");
                            Err(Box::new(Status::unauthenticated("invalid token"))
                                as Box<dyn std::error::Error + Send + Sync>)
                        }
                    },
                    Err(e) => Err(e),
                }
            } else {
                inner.call(req).await.map_err(Into::into)
            }
        })
    }
}
