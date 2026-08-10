//! Tests for `ingestion_keys_proxy.rs` — the ingestion key-management proxy
//! (#1411).
//!
//! Modeled on `maps_tests.rs`'s `build_handler_router_with_user` +
//! `.oneshot(...)` pattern for layering a `ValidatedUser` extension the way
//! `--disable-auth` does and exercising the real `AdminUser` extractor. A
//! loopback `wiremock` server stands in for ingestion, verifying the proxy
//! forwards method/path/query/body/`Content-Type` and passes the response
//! status/body back verbatim. `IngestionProxyConfig.credentials` is built
//! from `Arc::new(TrivialRequestDecorator {})` rather than a real
//! `OidcClientCredentialsDecorator`, so no OIDC token endpoint needs
//! stubbing and `forward`'s `.decorate()` call is a no-op.

use analytics_web_srv::auth::{AuthToken, ValidatedUser};
use analytics_web_srv::ingestion_keys_proxy::{
    IngestionProxyConfig, IngestionProxyState, ingestion_keys_proxy_router,
};
use axum::{Extension, Router, body::Body, http::Request, http::StatusCode};
use micromegas::telemetry_sink::request_decorator::TrivialRequestDecorator;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn admin_user() -> ValidatedUser {
    ValidatedUser {
        subject: "admin".to_string(),
        email: Some("admin@example.com".to_string()),
        issuer: "local".to_string(),
        is_admin: true,
    }
}

fn non_admin_user() -> ValidatedUser {
    ValidatedUser {
        subject: "reader".to_string(),
        email: Some("reader@example.com".to_string()),
        issuer: "local".to_string(),
        is_admin: false,
    }
}

fn configured_state(base_url: String) -> IngestionProxyState {
    IngestionProxyState {
        config: Some(Arc::new(IngestionProxyConfig {
            base_url,
            credentials: Arc::new(TrivialRequestDecorator {}),
            client: reqwest::Client::new(),
        })),
    }
}

fn build_router(state: IngestionProxyState, user: ValidatedUser) -> Router {
    ingestion_keys_proxy_router("")
        .layer(Extension(state))
        .layer(Extension(AuthToken(String::new())))
        .layer(Extension(user))
}

fn post_request(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request")
}

fn get_request(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("build request")
}

fn delete_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .expect("build request")
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("reading response body");
    serde_json::from_slice(&bytes).expect("parsing response body as json")
}

// ---------------------------------------------------------------------------
// Forwarding: method/path/query/body/Content-Type, status+body passthrough.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mint_forwards_method_path_body_and_content_type() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth/api_keys"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "key_id": "b3f6d9d2-0000-0000-0000-000000000001",
            "name": "game-client-42",
            "created_at": "2026-01-01T00:00:00Z",
            "key": "mmk_test"
        })))
        .mount(&mock_server)
        .await;

    let app = build_router(configured_state(mock_server.uri()), admin_user());
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys",
            r#"{"name": "game-client-42"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    assert_eq!(body["name"], "game-client-42");
    assert_eq!(body["key"], "mmk_test");
}

#[tokio::test]
async fn list_forwards_query_string() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/auth/api_keys"))
        .and(query_param("limit", "10"))
        .and(query_param("include_revoked", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&mock_server)
        .await;

    let app = build_router(configured_state(mock_server.uri()), admin_user());
    let response = app
        .oneshot(get_request(
            "/api/ingestion-api-keys?limit=10&include_revoked=false",
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn revoke_forwards_path() {
    let key_id = uuid::Uuid::new_v4();
    let mock_server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!("/auth/api_keys/{key_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "revoked_at": "2026-01-03T00:00:00Z",
            "effective_within_seconds": 60
        })))
        .mount(&mock_server)
        .await;

    let app = build_router(configured_state(mock_server.uri()), admin_user());
    let response = app
        .oneshot(delete_request(&format!("/api/ingestion-api-keys/{key_id}")))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["effective_within_seconds"], 60);
}

/// A legitimate `ApiKeyError::NotFound` — a non-empty JSON 404 body — must be
/// forwarded verbatim, not translated into the "ingestion route missing"
/// synthesized error.
#[tokio::test]
async fn non_empty_404_is_forwarded_verbatim() {
    let key_id = uuid::Uuid::new_v4();
    let mock_server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!("/auth/api_keys/{key_id}")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "key not found"})))
        .mount(&mock_server)
        .await;

    let app = build_router(configured_state(mock_server.uri()), admin_user());
    let response = app
        .oneshot(delete_request(&format!("/api/ingestion-api-keys/{key_id}")))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = json_body(response).await;
    assert_eq!(body["message"], "key not found");
}

/// An **empty**-body 404 — axum's no-matching-route default, never
/// `ApiKeyError`'s response shape — is mapped to a clearer error instead of
/// forwarded as a bare 404, since it almost always means ingestion auth is
/// disabled on the target service.
#[tokio::test]
async fn empty_404_is_mapped_to_a_clearer_error() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/auth/api_keys"))
        .respond_with(ResponseTemplate::new(404).set_body_string(""))
        .mount(&mock_server)
        .await;

    let app = build_router(configured_state(mock_server.uri()), admin_user());
    let response = app
        .oneshot(get_request("/api/ingestion-api-keys"))
        .await
        .expect("call service");
    assert_ne!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

// ---------------------------------------------------------------------------
// Rejected before forwarding: non-admin, and missing config. Neither ever
// reaches the mock server.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_admin_rejected_before_forwarding() {
    let mock_server = MockServer::start().await;
    // Deliberately no `Mock::given(...)` mounted — any request reaching the
    // mock server would still be recorded by `received_requests()` below,
    // matched or not.

    let app = build_router(configured_state(mock_server.uri()), non_admin_user());
    let response = app
        .oneshot(post_request("/api/ingestion-api-keys", r#"{"name": "x"}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let received = mock_server
        .received_requests()
        .await
        .expect("request recording enabled");
    assert!(
        received.is_empty(),
        "a non-admin caller must never trigger an outbound call to ingestion"
    );
}

#[tokio::test]
async fn missing_config_returns_503_without_forwarding() {
    let app = build_router(IngestionProxyState { config: None }, admin_user());
    let response = app
        .oneshot(get_request("/api/ingestion-api-keys"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
