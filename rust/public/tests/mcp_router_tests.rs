use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::middleware::{self, Next};
use http::{StatusCode, header};
use micromegas::servers::mcp::config::McpConfig;
use micromegas::servers::mcp::register_routes;
use micromegas_auth::types::{AuthContext, AuthType};
use std::sync::Arc;
use tower::ServiceExt;

async fn fake_auth(mut req: Request, next: Next) -> axum::response::Response {
    req.extensions_mut().insert(AuthContext {
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
    });
    next.run(req).await
}

fn router(config: &McpConfig) -> Router {
    register_routes(Router::new(), config, vec![])
        .expect("register")
        .layer(middleware::from_fn(fake_auth))
}

fn post(host: &str, origin: Option<&str>, body: &str) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::HOST, host)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(o) = origin {
        b = b.header(header::ORIGIN, o);
    }
    b.body(Body::from(body.to_string())).expect("request")
}

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;

async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .expect("body");
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn initialize_returns_instructions_and_skills_capability() {
    let resp = router(&McpConfig::default())
        .oneshot(post("localhost:3000", None, INITIALIZE))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp).await;
    assert!(body.contains("instructions"), "{body}");
    assert!(body.contains("Mandatory rules"), "{body}");
    assert!(body.contains("io.modelcontextprotocol/skills"), "{body}");
}

#[tokio::test]
async fn disallowed_host_is_forbidden() {
    let resp = router(&McpConfig::default())
        .oneshot(post("evil.example", None, INITIALIZE))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn configured_host_is_allowed() {
    let config = McpConfig {
        allowed_hosts: vec!["analytics.example".to_string()],
        ..McpConfig::default()
    };
    let resp = router(&config)
        .oneshot(post("analytics.example", None, INITIALIZE))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn disallowed_origin_is_forbidden() {
    let config = McpConfig {
        allowed_origins: vec!["https://app.example:443".to_string()],
        ..McpConfig::default()
    };
    let resp = router(&config)
        .oneshot(post("localhost", Some("https://evil.example"), INITIALIZE))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn no_origin_validation_by_default() {
    let resp = router(&McpConfig::default())
        .oneshot(post(
            "localhost",
            Some("https://anything.example"),
            INITIALIZE,
        ))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn oversized_body_is_rejected() {
    let config = McpConfig {
        max_request_body_bytes: 64,
        ..McpConfig::default()
    };
    let resp = router(&config)
        .oneshot(post("localhost", None, INITIALIZE))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
