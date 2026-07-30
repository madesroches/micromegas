use futures::future::BoxFuture;
use http::Request;
use http::header::AUTHORIZATION;
use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas_auth::tower::AuthService;
use micromegas_auth::types::{AuthContext, AuthProvider, AuthType, RequestParts};
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::Service;
use tower::ServiceExt;

// Mock service that returns OK - we'll just check if it's called
#[derive(Clone)]
struct MockService {
    should_have_auth: bool,
}

impl Service<Request<tonic::body::Body>> for MockService {
    type Response = http::Response<String>;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<tonic::body::Body>) -> Self::Future {
        let has_auth = req.extensions().get::<AuthContext>().is_some();
        let should_have = self.should_have_auth;

        Box::pin(async move {
            if should_have && !has_auth {
                return Err("Expected auth context but not found".into());
            }
            Ok(http::Response::new("OK".to_string()))
        })
    }
}

/// Mock service that captures the inbound `x-auth-is-admin` header value (if any) so tests can
/// assert on what `AuthService` re-injected, and that a client-supplied copy was stripped.
#[derive(Clone)]
struct HeaderCapturingService {
    captured: Arc<std::sync::Mutex<Option<Option<String>>>>,
}

impl Service<Request<tonic::body::Body>> for HeaderCapturingService {
    type Response = http::Response<String>;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<tonic::body::Body>) -> Self::Future {
        let value = req
            .headers()
            .get("x-auth-is-admin")
            .map(|v| v.to_str().unwrap_or_default().to_string());
        *self.captured.lock().expect("lock") = Some(value);

        Box::pin(async move { Ok(http::Response::new("OK".to_string())) })
    }
}

/// Mock auth provider that always succeeds, returning a caller-configurable `AuthContext`.
/// `ApiKeyAuthProvider` hardcodes `is_admin: false`, and `OidcAuthProvider` needs a live JWKS —
/// neither can exercise the `is_admin: true` path, so this fills that gap.
struct MockAdminAuthProvider {
    is_admin: bool,
}

#[async_trait::async_trait]
impl AuthProvider for MockAdminAuthProvider {
    async fn validate_request(&self, _parts: &dyn RequestParts) -> anyhow::Result<AuthContext> {
        Ok(AuthContext {
            subject: "mock-subject".to_string(),
            email: Some("mock@example.com".to_string()),
            issuer: "mock-issuer".to_string(),
            audience: None,
            expires_at: None,
            auth_type: AuthType::Oidc,
            is_admin: self.is_admin,
            allow_delegation: false,
        })
    }
}

#[tokio::test]
async fn test_auth_service_with_valid_token() {
    let keyring = parse_key_ring(r#"[{"name": "test", "key": "secret"}]"#).unwrap();
    let auth_provider = Arc::new(ApiKeyAuthProvider::new(keyring));

    let mut service = AuthService {
        inner: MockService {
            should_have_auth: true,
        },
        auth_provider: Some(auth_provider as Arc<dyn AuthProvider>),
    };

    let req = Request::builder()
        .header(AUTHORIZATION, "Bearer secret")
        .body(tonic::body::Body::empty())
        .unwrap();

    let result = service.ready().await.unwrap().call(req).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_auth_service_with_invalid_token() {
    let keyring = parse_key_ring(r#"[{"name": "test", "key": "secret"}]"#).unwrap();
    let auth_provider = Arc::new(ApiKeyAuthProvider::new(keyring));

    let mut service = AuthService {
        inner: MockService {
            should_have_auth: false,
        },
        auth_provider: Some(auth_provider as Arc<dyn AuthProvider>),
    };

    let req = Request::builder()
        .header(AUTHORIZATION, "Bearer wrong")
        .body(tonic::body::Body::empty())
        .unwrap();

    let result = service.ready().await.unwrap().call(req).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_auth_service_no_header() {
    let keyring = parse_key_ring(r#"[{"name": "test", "key": "secret"}]"#).unwrap();
    let auth_provider = Arc::new(ApiKeyAuthProvider::new(keyring));

    let mut service = AuthService {
        inner: MockService {
            should_have_auth: false,
        },
        auth_provider: Some(auth_provider as Arc<dyn AuthProvider>),
    };

    let req = Request::builder().body(tonic::body::Body::empty()).unwrap();

    let result = service.ready().await.unwrap().call(req).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_auth_service_no_provider() {
    let mut service = AuthService {
        inner: MockService {
            should_have_auth: false,
        },
        auth_provider: None,
    };

    let req = Request::builder().body(tonic::body::Body::empty()).unwrap();

    let result = service.ready().await.unwrap().call(req).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_auth_service_sets_is_admin_header_true() {
    let auth_provider = Arc::new(MockAdminAuthProvider { is_admin: true });
    let captured = Arc::new(std::sync::Mutex::new(None));
    let mut service = AuthService {
        inner: HeaderCapturingService {
            captured: captured.clone(),
        },
        auth_provider: Some(auth_provider as Arc<dyn AuthProvider>),
    };

    let req = Request::builder()
        .header(AUTHORIZATION, "Bearer irrelevant")
        .body(tonic::body::Body::empty())
        .unwrap();

    let result = service.ready().await.unwrap().call(req).await;
    assert!(result.is_ok());
    assert_eq!(
        captured.lock().expect("lock").clone(),
        Some(Some("true".to_string()))
    );
}

#[tokio::test]
async fn test_auth_service_sets_is_admin_header_false() {
    let auth_provider = Arc::new(MockAdminAuthProvider { is_admin: false });
    let captured = Arc::new(std::sync::Mutex::new(None));
    let mut service = AuthService {
        inner: HeaderCapturingService {
            captured: captured.clone(),
        },
        auth_provider: Some(auth_provider as Arc<dyn AuthProvider>),
    };

    let req = Request::builder()
        .header(AUTHORIZATION, "Bearer irrelevant")
        .body(tonic::body::Body::empty())
        .unwrap();

    let result = service.ready().await.unwrap().call(req).await;
    assert!(result.is_ok());
    assert_eq!(
        captured.lock().expect("lock").clone(),
        Some(Some("false".to_string()))
    );
}

#[tokio::test]
async fn test_auth_service_strips_client_supplied_is_admin_header() {
    // A non-admin AuthContext must result in the re-injected header being "false", even though
    // the client tried to smuggle in "true" — AuthService::call strips it before re-setting it
    // from the validated AuthContext.
    let auth_provider = Arc::new(MockAdminAuthProvider { is_admin: false });
    let captured = Arc::new(std::sync::Mutex::new(None));
    let mut service = AuthService {
        inner: HeaderCapturingService {
            captured: captured.clone(),
        },
        auth_provider: Some(auth_provider as Arc<dyn AuthProvider>),
    };

    let req = Request::builder()
        .header(AUTHORIZATION, "Bearer irrelevant")
        .header("x-auth-is-admin", "true")
        .body(tonic::body::Body::empty())
        .unwrap();

    let result = service.ready().await.unwrap().call(req).await;
    assert!(result.is_ok());
    assert_eq!(
        captured.lock().expect("lock").clone(),
        Some(Some("false".to_string())),
        "client-supplied x-auth-is-admin: true must not survive AuthService"
    );
}
