//! Tests for the maps endpoints.
//!
//! Two flavors of test live here:
//!
//! 1. Handler-behavior tests bypass `cookie_auth_middleware` and pre-insert
//!    a `ValidatedUser` extension (the same shape `--disable-auth` uses).
//!    These cover 503 / 200 / 400 / 404 paths.
//! 2. An auth-regression guard wraps the routes with the real
//!    `cookie_auth_middleware`, mirroring `build_protected_routes`. The
//!    middleware short-circuits at the cookie-jar lookup so no live OIDC
//!    server is needed.

use analytics_web_srv::auth::{AuthState, AuthToken, OidcClientConfig, ValidatedUser};
use analytics_web_srv::maps::{MapsState, is_direct_child, maps_blob, maps_catalog};
use axum::{
    Extension, Router,
    body::Body,
    http::{Request, StatusCode},
    middleware,
    routing::get,
};
use object_store::{
    Attribute, AttributeValue, Attributes, ObjectStore, PutOptions, memory::InMemory, path::Path,
};
use std::sync::Arc;
use tower::ServiceExt;

fn anon_user() -> ValidatedUser {
    ValidatedUser {
        subject: "anonymous".to_string(),
        email: None,
        issuer: "local".to_string(),
        is_admin: true,
    }
}

fn create_test_auth_state() -> AuthState {
    AuthState {
        oidc_provider: Arc::new(tokio::sync::OnceCell::new()),
        auth_provider: Arc::new(tokio::sync::OnceCell::new()),
        config: OidcClientConfig {
            issuer: "https://issuer.example.com".to_string(),
            client_id: "test-client".to_string(),
            redirect_uri: "http://localhost:3000/auth/callback".to_string(),
        },
        cookie_domain: None,
        secure_cookies: false,
        state_signing_secret: b"test-secret-key-32-bytes-long!!!".to_vec(),
        base_path: String::new(),
    }
}

/// Build a router wired the same way `build_protected_routes` does, but with
/// auth bypassed by pre-inserting a synthetic `ValidatedUser`. Used to exercise
/// handler behavior without standing up an OIDC mock.
fn build_handler_router(maps_state: MapsState) -> Router {
    Router::new()
        .route("/api/maps/catalog", get(maps_catalog))
        .route("/api/maps/blob/{filename}", get(maps_blob))
        .layer(Extension(maps_state))
        .layer(Extension(AuthToken(String::new())))
        .layer(Extension(anon_user()))
}

async fn put_glb(store: &Arc<dyn ObjectStore>, name: &str, bytes: Vec<u8>) {
    store
        .put(&Path::from(name), bytes.into())
        .await
        .expect("put glb");
}

async fn put_with_attrs(
    store: &Arc<dyn ObjectStore>,
    name: &str,
    bytes: Vec<u8>,
    attrs: Attributes,
) {
    let opts = PutOptions {
        attributes: attrs,
        ..PutOptions::default()
    };
    store
        .put_opts(&Path::from(name), bytes.into(), opts)
        .await
        .expect("put_opts glb");
}

#[tokio::test]
async fn catalog_returns_503_when_storage_unconfigured() {
    let app = build_handler_router(MapsState { store: None });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/maps/catalog")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn blob_returns_503_when_storage_unconfigured() {
    let app = build_handler_router(MapsState { store: None });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/maps/blob/main.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn catalog_lists_flat_entries_alphabetized_with_size() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    put_glb(&store, "main.glb", vec![0u8; 1000]).await;
    put_glb(&store, "level_a.glb", vec![0u8; 2000]).await;
    // Nested content has a `/` in the location returned by `list` — the
    // catalog filters those out so subdirectory clutter doesn't surface.
    store
        .put(&Path::from("nested/foo.glb"), vec![0u8; 10].into())
        .await
        .expect("put nested");

    let app = build_handler_router(MapsState {
        store: Some(store.clone()),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/maps/catalog")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let entries: serde_json::Value = serde_json::from_slice(&body).expect("json catalog");
    let arr = entries.as_array().expect("array");
    assert_eq!(
        arr.len(),
        2,
        "expected only flat children, no nested: {arr:?}"
    );
    assert_eq!(arr[0]["file"], "level_a.glb", "alphabetized");
    assert_eq!(arr[0]["size"], 2000);
    assert_eq!(arr[1]["file"], "main.glb");
    assert_eq!(arr[1]["size"], 1000);
}

#[tokio::test]
async fn blob_streams_bytes_and_sets_content_type_and_length() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let payload: Vec<u8> = (0..256u32).map(|i| i as u8).collect();
    put_glb(&store, "main.glb", payload.clone()).await;

    let app = build_handler_router(MapsState { store: Some(store) });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/maps/blob/main.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(
        headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("model/gltf-binary")
    );
    assert_eq!(
        headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<usize>().ok()),
        Some(payload.len())
    );
    assert!(
        headers.get(http::header::CONTENT_ENCODING).is_none(),
        "no Content-Encoding when upstream object has none"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.to_vec(), payload, "body matches what was put");
}

#[tokio::test]
async fn blob_passes_through_content_encoding_from_object_metadata() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let mut attrs = Attributes::new();
    attrs.insert(Attribute::ContentEncoding, AttributeValue::from("gzip"));
    attrs.insert(
        Attribute::ContentType,
        AttributeValue::from("model/gltf-binary"),
    );
    put_with_attrs(&store, "main.glb", vec![1u8, 2, 3, 4], attrs).await;

    let app = build_handler_router(MapsState { store: Some(store) });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/maps/blob/main.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok()),
        Some("gzip"),
        "gzip encoding from object metadata should be passed through"
    );
}

#[tokio::test]
async fn blob_rejects_filenames_containing_slashes() {
    // The prefix is reserved for map assets, so we no longer enforce a
    // `.glb` extension or character set. The only defense-in-depth check
    // is that the decoded filename segment must not contain a `/`, in
    // case a percent-encoded `/` makes it past axum's path routing.
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    put_glb(&store, "main.glb", b"hi".to_vec()).await;
    let app = build_handler_router(MapsState { store: Some(store) });

    let bad_names = ["..%2Fmain.glb", "foo%2Fbar.glb"];
    for name in bad_names {
        let uri = format!("/api/maps/blob/{name}");
        let response = app
            .clone()
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "expected 400 for {name}"
        );
    }
}

#[tokio::test]
async fn blob_serves_non_glb_extensions_from_prefix() {
    // Reserve-the-prefix policy: the maps URI is committed to hold map
    // assets, so the extension isn't enforced server-side. Anything flat
    // in the bucket is fetchable.
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    put_glb(&store, "readme.txt", b"hello".to_vec()).await;
    let app = build_handler_router(MapsState { store: Some(store) });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/maps/blob/readme.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn blob_returns_404_for_missing_object() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    // Empty store — main.glb doesn't exist.
    let app = build_handler_router(MapsState { store: Some(store) });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/maps/blob/main.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Auth regression guard
// ---------------------------------------------------------------------------
//
// Hard requirement: an unauthenticated request to /api/maps/catalog and
// /api/maps/blob/{filename} returns 401 when the routes are wrapped by
// `cookie_auth_middleware`. Guards against accidental future refactors
// that move the routes out of the protected group.
//
// `cookie_auth_middleware` returns Unauthorized at the cookie-jar lookup
// before initializing the OIDC provider, so this path is reachable without
// a live JWKS endpoint.

#[tokio::test]
async fn unauthenticated_catalog_returns_401() {
    let auth_state = create_test_auth_state();
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = Router::new()
        .route("/api/maps/catalog", get(maps_catalog))
        .layer(Extension(MapsState { store: Some(store) }))
        .layer(middleware::from_fn_with_state(
            auth_state,
            analytics_web_srv::auth::cookie_auth_middleware,
        ));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/maps/catalog")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        !body.windows(4).any(|w| w == b"file"),
        "catalog body must not leak entries to unauthenticated callers"
    );
}

#[tokio::test]
async fn unauthenticated_blob_returns_401() {
    let auth_state = create_test_auth_state();
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    put_glb(&store, "main.glb", b"SECRET-GLB-BYTES".to_vec()).await;

    let app = Router::new()
        .route("/api/maps/blob/{filename}", get(maps_blob))
        .layer(Extension(MapsState { store: Some(store) }))
        .layer(middleware::from_fn_with_state(
            auth_state,
            analytics_web_srv::auth::cookie_auth_middleware,
        ));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/maps/blob/main.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        !body
            .windows(b"SECRET-GLB-BYTES".len())
            .any(|w| w == b"SECRET-GLB-BYTES"),
        "blob bytes must not leak to unauthenticated callers"
    );
}

// ---------------------------------------------------------------------------
// is_direct_child unit tests
// ---------------------------------------------------------------------------

#[test]
fn is_direct_child_accepts_flat_names() {
    assert!(is_direct_child("main.glb"));
    assert!(is_direct_child("level_a.glb"));
    assert!(is_direct_child("Arena_North_01.glb"));
    assert!(is_direct_child("a-b_c.0.glb"));
    assert!(is_direct_child("readme.txt"), "extension is not enforced");
}

#[test]
fn is_direct_child_rejects_paths_with_slashes() {
    assert!(!is_direct_child(""));
    assert!(!is_direct_child("nested/foo.glb"));
    assert!(!is_direct_child("/etc/passwd"));
    assert!(!is_direct_child("../main.glb"));
}
