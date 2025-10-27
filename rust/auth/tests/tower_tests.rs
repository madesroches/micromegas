use futures::future::BoxFuture;
use http::Request;
use http::header::AUTHORIZATION;
use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas_auth::tower::AuthService;
use micromegas_auth::types::{AuthContext, AuthProvider};
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
