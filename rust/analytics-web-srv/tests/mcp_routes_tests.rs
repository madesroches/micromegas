//! No-DB tests of the `/mcp` route as mounted by the web server.

use analytics_web_srv::mcp_auth::{McpOAuthMetadata, resolve_public_url};
use analytics_web_srv::web_server::{McpMount, build_mcp_routes};
use async_trait::async_trait;
use axum::body::Body;
use http::{Request, StatusCode, header};
use micromegas::auth::types::{AuthContext, AuthProvider, AuthType, RequestParts};
use micromegas::servers::mcp::config::McpConfig;
use std::sync::Arc;
use tower::ServiceExt;

struct StubProvider;

#[async_trait]
impl AuthProvider for StubProvider {
    fn name(&self) -> &'static str {
        "stub"
    }

    async fn validate_request(&self, parts: &dyn RequestParts) -> anyhow::Result<AuthContext> {
        match parts.bearer_token() {
            Some("good") => Ok(AuthContext {
                subject: "s".into(),
                email: None,
                issuer: "i".into(),
                audience: None,
                expires_at: None,
                auth_type: AuthType::ApiKey,
                allow_delegation: false,
                bound_audience: None,
                read_audiences: vec![],
                analytics_key_id: None,
                memberships: Arc::from([]),
            }),
            _ => anyhow::bail!("invalid token"),
        }
    }
}

fn mounted(base_path: &str) -> axum::Router {
    mounted_with(base_path, None)
}

fn mounted_with(base_path: &str, oauth: Option<McpOAuthMetadata>) -> axum::Router {
    let config = McpConfig::from_lookup(base_path, |_| None);
    build_mcp_routes(
        base_path,
        Some(McpMount {
            config,
            auth_provider: Arc::new(StubProvider),
            providers: vec![],
            oauth,
        }),
    )
    .expect("routes")
}

fn request(path: &str, token: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "localhost")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    b.body(Body::from(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
    ))
    .expect("request")
}

#[tokio::test]
async fn missing_bearer_is_401_with_challenge() {
    let resp = mounted("")
        .oneshot(request("/mcp", None))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(resp.headers().contains_key(header::WWW_AUTHENTICATE));
}

#[tokio::test]
async fn invalid_bearer_is_401() {
    let resp = mounted("/base")
        .oneshot(request("/base/mcp", Some("bad")))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(resp.headers().contains_key(header::WWW_AUTHENTICATE));
}

#[tokio::test]
async fn valid_bearer_reaches_the_service() {
    let resp = mounted("")
        .oneshot(request("/mcp", Some("good")))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .expect("body");
    assert!(String::from_utf8_lossy(&bytes).contains("instructions"));
}

#[tokio::test]
async fn disabled_route_answers_404() {
    for (base, path) in [("", "/mcp"), ("/base", "/base/mcp")] {
        let resp = build_mcp_routes(base, None)
            .expect("routes")
            .oneshot(request(path, Some("good")))
            .await
            .expect("response");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let metadata = format!("{}/.well-known/oauth-protected-resource", base);
        let resp = build_mcp_routes(base, None)
            .expect("routes")
            .oneshot(get_request(&metadata))
            .await
            .expect("response");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

fn get_request(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header(header::HOST, "localhost")
        .body(Body::empty())
        .expect("request")
}

fn oauth() -> McpOAuthMetadata {
    McpOAuthMetadata::new(
        "https://analytics.example.com/base/mcp",
        "https://idp.example.com",
    )
    .expect("metadata")
}

#[test]
fn public_url_defaults_to_redirect_origin() {
    let url = resolve_public_url(
        "https://analytics.example.com/base/auth/callback",
        "/base/mcp",
        None,
    )
    .expect("url");
    assert_eq!(url, "https://analytics.example.com/base/mcp");
}

#[test]
fn public_url_override_wins_and_must_end_in_mcp_path() {
    let url = resolve_public_url(
        "http://localhost:3000/callback",
        "/mcp",
        Some("https://public.example.com/mcp"),
    )
    .expect("url");
    assert_eq!(url, "https://public.example.com/mcp");
    for bad in [
        "https://public.example.com/other",
        "not a url",
        "ftp://x.example.com/mcp",
    ] {
        assert!(resolve_public_url("http://localhost:3000/callback", "/mcp", Some(bad)).is_err());
    }
}

#[test]
fn metadata_has_expected_urls_and_body() {
    let m = oauth();
    assert_eq!(m.resource(), "https://analytics.example.com/base/mcp");
    assert_eq!(
        m.metadata_url(),
        "https://analytics.example.com/base/.well-known/oauth-protected-resource"
    );
    assert_eq!(
        m.body(),
        &serde_json::json!({
            "resource": "https://analytics.example.com/base/mcp",
            "authorization_servers": ["https://idp.example.com"],
            "bearer_methods_supported": ["header"],
        })
    );
    assert!(McpOAuthMetadata::new("https://x.example.com/other", "i").is_err());
}

#[tokio::test]
async fn challenge_points_at_metadata() {
    let app = mounted_with("/base", Some(oauth()));
    let resp = app
        .clone()
        .oneshot(request("/base/mcp", None))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let challenge = resp.headers()[header::WWW_AUTHENTICATE]
        .to_str()
        .expect("str");
    assert!(challenge.contains(
        "resource_metadata=\"https://analytics.example.com/base/.well-known/oauth-protected-resource\""
    ));
    assert!(!challenge.contains("invalid_token"));

    let resp = app
        .oneshot(request("/base/mcp", Some("bad")))
        .await
        .expect("response");
    let challenge = resp.headers()[header::WWW_AUTHENTICATE]
        .to_str()
        .expect("str");
    assert!(challenge.contains("resource_metadata="));
    assert!(challenge.contains("error=\"invalid_token\""));
}

#[tokio::test]
async fn metadata_route_is_public_json() {
    let resp = mounted_with("/base", Some(oauth()))
        .oneshot(get_request("/base/.well-known/oauth-protected-resource"))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["authorization_servers"][0], "https://idp.example.com");
}
