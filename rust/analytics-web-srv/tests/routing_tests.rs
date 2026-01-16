//! Tests for base path routing and trailing slash normalization
//!
//! These tests verify that:
//! 1. The exact base path (/micromegas) serves index.html with config injection
//! 2. Trailing slashes (/micromegas/) are normalized BEFORE routing
//! 3. Nested paths (/micromegas/processes) work correctly
//! 4. Static files (/micromegas/_next/*) are served correctly
//!
//! Key insight: `Router::layer()` runs AFTER routing, so `NormalizePathLayer`
//! must WRAP the router using `layer()` method, not be added via `Router::layer()`.

use axum::{
    Router, body::Body, extract::State, http::StatusCode, response::IntoResponse, routing::get,
};
use http::{Request, header};
use tower::{Layer, ServiceExt};
use tower_http::normalize_path::NormalizePathLayer;

#[derive(Clone)]
struct IndexState {
    base_path: String,
}

/// Mock handler that returns HTML with base tag and config injected
async fn serve_index_with_config(State(state): State<IndexState>) -> impl IntoResponse {
    let base_href = if state.base_path.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", state.base_path)
    };
    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
<base href="{}"><script>window.__MICROMEGAS_CONFIG__={{basePath:"{}"}}</script>
</head>
<body>Index</body>
</html>"#,
        base_href, state.base_path
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html)
}

/// Build a test router that mimics the real app's routing structure
fn build_test_router(base_path: &str) -> Router {
    let index_state = IndexState {
        base_path: base_path.to_string(),
    };

    // Simple handler for static files (just returns "static file")
    let static_handler = get(|| async { "static file" });

    // SPA fallback handler for unmatched paths
    let spa_fallback = get(serve_index_with_config).with_state(index_state.clone());

    // Build frontend router for paths UNDER base_path (not base_path itself)
    // The "/" route is NOT included here to avoid conflict with the explicit base_path route
    let frontend = Router::new()
        .route("/index.html", get(serve_index_with_config))
        .with_state(index_state.clone())
        .route("/_next/static/test.js", static_handler)
        // Fallback to index for SPA routes
        .fallback(spa_fallback);

    // Build the app with explicit base path route + nest
    Router::new()
        // Explicit route for exact base_path match
        .route(
            base_path,
            get(serve_index_with_config).with_state(index_state),
        )
        // Nested router for everything under base_path/*
        .nest(base_path, frontend)
}

/// Build the full service with trailing slash normalization
/// NOTE: The NormalizePathLayer must wrap the router, not be added via .layer()
/// because Router::layer runs AFTER routing, but we need normalization BEFORE routing
fn build_test_service(base_path: &str) -> tower_http::normalize_path::NormalizePath<Router> {
    let router = build_test_router(base_path);
    NormalizePathLayer::trim_trailing_slash().layer(router)
}

#[tokio::test]
async fn test_base_path_exact_match() {
    let app = build_test_router("/micromegas");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/micromegas")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        body_str.contains("__MICROMEGAS_CONFIG__"),
        "Response should contain config injection"
    );
    assert!(
        body_str.contains(r#"basePath:"/micromegas""#),
        "Config should have correct base path"
    );
    assert!(
        body_str.contains(r#"<base href="/micromegas/">"#),
        "Response should contain base tag with trailing slash"
    );
}

#[tokio::test]
async fn test_base_path_with_trailing_slash() {
    // Use build_test_service which wraps router with NormalizePathLayer
    // This is required because Router::layer runs AFTER routing
    let app = build_test_service("/micromegas");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/micromegas/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    println!("Response status for /micromegas/: {:?}", response.status());

    // /micromegas/ gets normalized to /micromegas BEFORE routing
    // So it matches the explicit route for the base path
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "Trailing slash should be normalized and route should match"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        body_str.contains("__MICROMEGAS_CONFIG__"),
        "Response should contain config injection"
    );
}

#[tokio::test]
async fn test_nested_path_index() {
    let app = build_test_router("/micromegas");

    // Test /micromegas/ with nested router's "/" route
    // After trailing slash normalization, /micromegas/ becomes /micromegas
    // which is handled by the explicit route, not the nested router
    let response = app
        .oneshot(
            Request::builder()
                .uri("/micromegas/index.html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        body_str.contains("__MICROMEGAS_CONFIG__"),
        "Nested index.html should have config injection"
    );
}

#[tokio::test]
async fn test_static_file_route() {
    let app = build_test_router("/micromegas");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/micromegas/_next/static/test.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    assert_eq!(body_str, "static file");
}

#[tokio::test]
async fn test_trailing_slash_with_query_string() {
    // Use build_test_service which wraps router with NormalizePathLayer
    let app = build_test_service("/micromegas");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/micromegas/?foo=bar")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "Trailing slash with query string should work"
    );
}

#[tokio::test]
async fn test_root_path_not_affected() {
    // Root "/" should not be affected by trailing slash normalization.
    // Note: In production, MICROMEGAS_BASE_PATH="/" is trimmed to "" (empty string).
    let index_state = IndexState {
        base_path: String::new(),
    };

    let router = Router::new().route("/", get(serve_index_with_config).with_state(index_state));

    // Wrap with NormalizePathLayer
    let app = NormalizePathLayer::trim_trailing_slash().layer(router);

    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        body_str.contains(r#"<base href="/">"#),
        "Root deployment should have base href='/'. Got: {body_str}"
    );
}

#[tokio::test]
async fn test_unmatched_route_returns_404() {
    let app = build_test_router("/micromegas");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/other-path")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "Unmatched routes should return 404"
    );
}

#[tokio::test]
async fn test_deeply_nested_trailing_slash() {
    // Use build_test_service which wraps router with NormalizePathLayer
    let app = build_test_service("/micromegas");

    // Test that deeply nested paths with trailing slashes also get normalized
    // The SPA fallback should handle this and return index.html
    let response = app
        .oneshot(
            Request::builder()
                .uri("/micromegas/some/deep/path/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // SPA fallback returns index.html for unmatched paths under base_path
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        body_str.contains("__MICROMEGAS_CONFIG__"),
        "SPA fallback should return index with config"
    );
}

/// Test that deep URLs receive the base tag with correct href
/// This is the core fix: without <base href>, a request to /micromegas/screen/foo
/// would resolve ./assets/x.js to /micromegas/screen/assets/x.js (wrong)
/// instead of /micromegas/assets/x.js (correct)
#[tokio::test]
async fn test_deep_url_has_base_tag_for_asset_resolution() {
    let app = build_test_router("/micromegas");

    // Simulate a hard refresh on a deep SPA route
    let response = app
        .oneshot(
            Request::builder()
                .uri("/micromegas/screen/processes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    // The base tag must be present with trailing slash for correct relative URL resolution
    assert!(
        body_str.contains(r#"<base href="/micromegas/">"#),
        "Deep URLs must have base tag with trailing slash for asset resolution. Got: {body_str}"
    );

    // Config should also be present
    assert!(
        body_str.contains(r#"basePath:"/micromegas""#),
        "Config basePath should not have trailing slash"
    );
}

/// Test that empty base_path produces <base href="/">
#[tokio::test]
async fn test_empty_base_path_produces_root_base_href() {
    let index_state = IndexState {
        base_path: String::new(),
    };

    let html = serve_index_with_config(State(index_state))
        .await
        .into_response();

    let body = axum::body::to_bytes(html.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        body_str.contains(r#"<base href="/">"#),
        "Empty base_path should produce root base href. Got: {body_str}"
    );
    assert!(
        body_str.contains(r#"basePath:"""#),
        "Config should have empty basePath"
    );
}
