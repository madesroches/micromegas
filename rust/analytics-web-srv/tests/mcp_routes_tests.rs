//! No-DB tests of the `/mcp` route as mounted by the web server.

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
    let config = McpConfig::from_lookup(base_path, |_| None);
    build_mcp_routes(
        base_path,
        Some(McpMount {
            config,
            auth_provider: Arc::new(StubProvider),
            providers: vec![],
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
    }
}
