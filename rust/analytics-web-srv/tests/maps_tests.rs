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
use analytics_web_srv::maps::{
    MapsState, is_direct_child, maps_blob, maps_catalog, maps_delete, maps_upload,
};
use axum::{
    Extension, Router,
    body::Body,
    extract::DefaultBodyLimit,
    http::{Request, StatusCode},
    middleware,
    routing::{get, put},
};
use flate2::read::GzDecoder;
use object_store::{
    ObjectStore, ObjectStoreExt, local::LocalFileSystem, memory::InMemory, path::Path,
};
use std::io::Read;
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

fn non_admin_user() -> ValidatedUser {
    ValidatedUser {
        subject: "reader".to_string(),
        email: Some("reader@example.com".to_string()),
        issuer: "local".to_string(),
        is_admin: false,
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
        admin_var_name: "MICROMEGAS_ADMINS".to_string(),
    }
}

/// Build a router wired the same way `build_protected_routes` does, but with
/// auth bypassed by pre-inserting a synthetic `ValidatedUser`. Used to exercise
/// handler behavior without standing up an OIDC mock.
fn build_handler_router(maps_state: MapsState) -> Router {
    build_handler_router_with_user(maps_state, anon_user())
}

fn build_handler_router_with_user(maps_state: MapsState, user: ValidatedUser) -> Router {
    Router::new()
        .route("/api/maps/catalog", get(maps_catalog))
        .route(
            "/api/maps/blob/{filename}",
            get(maps_blob).put(maps_upload).delete(maps_delete),
        )
        .layer(Extension(maps_state))
        .layer(Extension(AuthToken(String::new())))
        .layer(Extension(user))
}

/// Variant that scopes a per-route body limit to PUT, mirroring main.rs.
fn build_handler_router_with_upload_limit(maps_state: MapsState, max_bytes: usize) -> Router {
    Router::new()
        .route("/api/maps/catalog", get(maps_catalog))
        .route(
            "/api/maps/blob/{filename}",
            put(maps_upload)
                .layer(DefaultBodyLimit::max(max_bytes))
                .delete(maps_delete)
                .get(maps_blob),
        )
        .layer(Extension(maps_state))
        .layer(Extension(AuthToken(String::new())))
        .layer(Extension(anon_user()))
}

/// Gzip-compress `raw` with the default compression level. Used by tests
/// that need to assert what the handlers store on disk under the `.gz`
/// suffix.
fn gzip_bytes(raw: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut enc, raw).expect("gzip in test fixture");
    enc.finish().expect("gzip in test fixture")
}

/// Test fixture: gzip the raw payload and store it under `<name>.gz`,
/// matching the storage convention enforced by `maps_upload`.
async fn put_map(store: &Arc<dyn ObjectStore>, name: &str, raw: &[u8]) {
    let gz = gzip_bytes(raw);
    store
        .put(&Path::from(format!("{name}.gz")), gz.into())
        .await
        .expect("put map fixture");
}

#[tokio::test]
async fn catalog_returns_503_when_storage_unconfigured() {
    let app = build_handler_router(MapsState::new(None));

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
    let app = build_handler_router(MapsState::new(None));

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
async fn catalog_lists_flat_entries_alphabetized_with_stripped_suffix() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let main_gz_len = gzip_bytes(&[0u8; 1000]).len() as u64;
    let level_gz_len = gzip_bytes(&[0u8; 2000]).len() as u64;
    put_map(&store, "main.glb", &[0u8; 1000]).await;
    put_map(&store, "level_a.glb", &[0u8; 2000]).await;
    // Nested `.gz` is still nested — the catalog filters by direct
    // children so subdirectory clutter doesn't surface.
    store
        .put(&Path::from("nested/foo.glb.gz"), vec![0u8; 10].into())
        .await
        .expect("put nested");
    // A bare key (no `.gz`) sneaked in via some out-of-band path must not
    // surface in the catalog — only entries that follow the storage
    // convention show up.
    store
        .put(&Path::from("rogue.glb"), vec![0u8; 5].into())
        .await
        .expect("put rogue");

    let app = build_handler_router(MapsState::new(Some(store.clone())));

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
        "expected only flat .gz children — no nested, no bare-key rogue: {arr:?}"
    );
    assert_eq!(
        arr[0]["file"], "level_a.glb",
        "alphabetized, suffix stripped"
    );
    assert_eq!(arr[0]["size"], level_gz_len);
    assert_eq!(arr[1]["file"], "main.glb");
    assert_eq!(arr[1]["size"], main_gz_len);
}

#[tokio::test]
async fn blob_streams_gzipped_bytes_and_sets_headers() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let payload: Vec<u8> = (0..256u32).map(|i| i as u8).collect();
    put_map(&store, "main.glb", &payload).await;
    let stored_len = gzip_bytes(&payload).len();

    let app = build_handler_router(MapsState::new(Some(store)));

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
            .get(http::header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok()),
        Some("gzip"),
        "GET always advertises gzip — the `.gz` suffix is the contract",
    );
    assert_eq!(
        headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<usize>().ok()),
        Some(stored_len),
        "Content-Length is the on-disk (gzipped) byte count",
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut decoder = GzDecoder::new(&body[..]);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .expect("gzip decompress");
    assert_eq!(decompressed, payload, "decoded body matches what was put");
}

#[tokio::test]
async fn blob_rejects_filenames_containing_slashes() {
    // The prefix is reserved for map assets, so we no longer enforce a
    // `.glb` extension or character set. The only defense-in-depth check
    // is that the decoded filename segment must not contain a `/`, in
    // case a percent-encoded `/` makes it past axum's path routing.
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    put_map(&store, "main.glb", b"hi").await;
    let app = build_handler_router(MapsState::new(Some(store)));

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
    // assets, so the `.glb` extension isn't enforced — only the storage
    // convention is (`<name>.gz` on disk).
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    put_map(&store, "readme.txt", b"hello").await;
    let app = build_handler_router(MapsState::new(Some(store)));

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
    let app = build_handler_router(MapsState::new(Some(store)));

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
        .layer(Extension(MapsState::new(Some(store))))
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
    put_map(&store, "main.glb", b"SECRET-GLB-BYTES").await;

    let app = Router::new()
        .route("/api/maps/blob/{filename}", get(maps_blob))
        .layer(Extension(MapsState::new(Some(store))))
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

// ---------------------------------------------------------------------------
// Catalog last_modified
// ---------------------------------------------------------------------------

#[tokio::test]
async fn catalog_includes_last_modified() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    put_map(&store, "main.glb", &[0u8; 4]).await;

    let app = build_handler_router(MapsState::new(Some(store)));

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
    let entry = &entries.as_array().expect("array")[0];
    let last_modified = entry["last_modified"]
        .as_str()
        .expect("last_modified is RFC3339 string");
    chrono::DateTime::parse_from_rfc3339(last_modified).expect("RFC3339 parses");
}

// ---------------------------------------------------------------------------
// Upload handler
// ---------------------------------------------------------------------------

fn put_glb_request(uri: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header(http::header::CONTENT_TYPE, "model/gltf-binary")
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn upload_stores_at_gz_suffix_and_round_trips() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = build_handler_router(MapsState::new(Some(store.clone())));

    let payload: Vec<u8> = (0..1024u32).map(|i| i as u8).collect();
    let response = app
        .oneshot(put_glb_request("/api/maps/blob/new.glb", payload.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        parsed["file"], "new.glb",
        "response echoes user-facing name"
    );
    let wire_size = parsed["size"].as_u64().expect("size");
    assert!(wire_size > 0);

    // Stored under the `.gz` suffix; the user-facing key has nothing on it.
    let bare = store
        .get(&Path::from("new.glb"))
        .await
        .expect_err("user-facing key must not exist");
    assert!(matches!(bare, object_store::Error::NotFound { .. }));

    let got = store
        .get(&Path::from("new.glb.gz"))
        .await
        .expect("stored at .gz suffix");
    let stored = got.bytes().await.expect("read stored bytes");
    assert_eq!(
        stored.len() as u64,
        wire_size,
        "reported size matches on-disk byte count",
    );

    let mut decoder = GzDecoder::new(&stored[..]);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .expect("gzip decompress");
    assert_eq!(decompressed, payload);
}

#[tokio::test]
async fn upload_passes_through_client_gzipped_body() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = build_handler_router(MapsState::new(Some(store.clone())));

    // Construct a real gzip frame so the client-pregzipped branch stores it
    // verbatim — a non-empty body, not just a flag check.
    let gzipped = gzip_bytes(b"hello world");

    let req = Request::builder()
        .method("PUT")
        .uri("/api/maps/blob/pre.glb")
        .header(http::header::CONTENT_TYPE, "model/gltf-binary")
        .header(http::header::CONTENT_ENCODING, "gzip")
        .body(Body::from(gzipped.clone()))
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let got = store
        .get(&Path::from("pre.glb.gz"))
        .await
        .expect("stored at .gz suffix");
    let stored = got.bytes().await.expect("read stored bytes");
    assert_eq!(
        stored.as_ref(),
        gzipped.as_slice(),
        "pre-gzipped body must be stored verbatim — no double encode"
    );
}

#[tokio::test]
async fn upload_rejects_dot_gz_filename() {
    // The `.gz` suffix is server-owned; accepting `level.glb.gz` would mean
    // either double-suffixing on disk (`level.glb.gz.gz`) or letting the
    // user pre-claim the stored key. Reject up front.
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = build_handler_router(MapsState::new(Some(store)));

    let response = app
        .oneshot(put_glb_request(
            "/api/maps/blob/level.glb.gz",
            vec![1, 2, 3],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn upload_rejects_wrong_content_type() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = build_handler_router(MapsState::new(Some(store)));

    let req = Request::builder()
        .method("PUT")
        .uri("/api/maps/blob/x.glb")
        .header(http::header::CONTENT_TYPE, "text/plain")
        .body(Body::from("not a glb"))
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn upload_rejects_invalid_filename() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = build_handler_router(MapsState::new(Some(store)));

    let response = app
        .oneshot(put_glb_request("/api/maps/blob/%2Fevil.glb", vec![1, 2, 3]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn upload_returns_503_when_storage_unconfigured() {
    let app = build_handler_router(MapsState::new(None));

    let response = app
        .oneshot(put_glb_request("/api/maps/blob/x.glb", vec![1, 2, 3]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn upload_requires_admin() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = build_handler_router_with_user(MapsState::new(Some(store.clone())), non_admin_user());

    let response = app
        .oneshot(put_glb_request("/api/maps/blob/blocked.glb", vec![0u8; 8]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // And nothing was stored (neither under the bare name nor `.gz`).
    let bare = store
        .get(&Path::from("blocked.glb"))
        .await
        .expect_err("not stored");
    assert!(matches!(bare, object_store::Error::NotFound { .. }));
    let gz = store
        .get(&Path::from("blocked.glb.gz"))
        .await
        .expect_err("not stored");
    assert!(matches!(gz, object_store::Error::NotFound { .. }));
}

#[tokio::test]
async fn upload_non_admin_does_not_buffer_body() {
    // Regression for the memory-exhaustion vector: a non-admin authenticated
    // user must not be able to force the server to buffer an upload body
    // before getting 403. We send a body whose stream errors on the *first*
    // poll — if the `Bytes` extractor ran, axum would surface a 400
    // body-read error. The `AdminUser` extractor is `FromRequestParts`, so
    // it runs before any body extractor; non-admins should see a clean 403
    // with the stream untouched.
    use futures::stream;
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = build_handler_router_with_user(MapsState::new(Some(store.clone())), non_admin_user());

    let err_body = Body::from_stream(stream::once(async {
        Err::<axum::body::Bytes, std::io::Error>(std::io::Error::other("body must not be read"))
    }));
    let req = Request::builder()
        .method("PUT")
        .uri("/api/maps/blob/blocked.glb")
        .header(http::header::CONTENT_TYPE, "model/gltf-binary")
        .body(err_body)
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn upload_rejects_oversize_body() {
    // Use a tiny limit so the test doesn't synthesize hundreds of MiB —
    // the per-route layer is identical to what main.rs applies; this
    // verifies the wiring scopes the cap to PUT.
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = build_handler_router_with_upload_limit(MapsState::new(Some(store)), 128);

    let response = app
        .oneshot(put_glb_request("/api/maps/blob/big.glb", vec![0u8; 1024]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn upload_body_limit_does_not_apply_to_delete_or_get() {
    // Sanity check: the per-route limit attached to put(...) must not
    // bleed onto the sibling DELETE / GET methods on the same path —
    // they share a `MethodRouter` but the layer is scoped to PUT.
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    put_map(&store, "main.glb", &[0u8; 2048]).await;

    let app = build_handler_router_with_upload_limit(MapsState::new(Some(store)), 128);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/maps/blob/main.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/maps/blob/main.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // GET should still report 404 (we just deleted it) — not 413, which
    // would imply the limit layered onto a non-PUT method.
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Delete handler
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_removes_object_and_returns_204() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    put_map(&store, "main.glb", b"glb-bytes").await;

    let app = build_handler_router(MapsState::new(Some(store.clone())));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/maps/blob/main.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let after = store
        .get(&Path::from("main.glb.gz"))
        .await
        .expect_err("gone");
    assert!(matches!(after, object_store::Error::NotFound { .. }));
}

#[tokio::test]
async fn delete_is_idempotent_for_missing_object() {
    // S3 returns 204 for DELETE on a missing key. LocalFileSystem and
    // some other backends surface `Error::NotFound`; the handler swallows
    // it so the API surface stays uniform.
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = build_handler_router(MapsState::new(Some(store)));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/maps/blob/never_existed.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn delete_returns_503_when_storage_unconfigured() {
    let app = build_handler_router(MapsState::new(None));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/maps/blob/main.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn delete_requires_admin() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    put_map(&store, "main.glb", b"glb").await;
    let stored_size = gzip_bytes(b"glb").len() as u64;

    let app = build_handler_router_with_user(MapsState::new(Some(store.clone())), non_admin_user());

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/maps/blob/main.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Object survives — admin gate ran before any store call.
    let got = store
        .get(&Path::from("main.glb.gz"))
        .await
        .expect("still there");
    assert_eq!(got.meta.size, stored_size);
}

#[tokio::test]
async fn delete_rejects_invalid_filename() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = build_handler_router(MapsState::new(Some(store)));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/maps/blob/%2Fevil.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Auth regression guards for the mutation routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unauthenticated_upload_returns_401() {
    let auth_state = create_test_auth_state();
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = Router::new()
        .route("/api/maps/blob/{filename}", put(maps_upload))
        .layer(Extension(MapsState::new(Some(store))))
        .layer(middleware::from_fn_with_state(
            auth_state,
            analytics_web_srv::auth::cookie_auth_middleware,
        ));

    let response = app
        .oneshot(put_glb_request("/api/maps/blob/x.glb", vec![1, 2, 3]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unauthenticated_delete_returns_401() {
    let auth_state = create_test_auth_state();
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let app = Router::new()
        .route(
            "/api/maps/blob/{filename}",
            axum::routing::delete(maps_delete),
        )
        .layer(Extension(MapsState::new(Some(store))))
        .layer(middleware::from_fn_with_state(
            auth_state,
            analytics_web_srv::auth::cookie_auth_middleware,
        ));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/maps/blob/x.glb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// LocalFileSystem backend (file:// URIs)
// ---------------------------------------------------------------------------
//
// `object_store::LocalFileSystem::put_opts` rejects any non-empty
// `PutOptions::attributes` with `NotImplemented`. The `.gz` suffix
// convention sidesteps that: PUT uses plain `put()` and the encoding
// signal lives in the key name, not in object metadata. This test pins
// the end-to-end behavior on a real on-disk store.

#[tokio::test]
async fn upload_round_trips_through_local_filesystem() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let local = LocalFileSystem::new_with_prefix(tmp.path()).expect("local fs");
    let store: Arc<dyn ObjectStore> = Arc::new(local);

    let app = build_handler_router(MapsState::new(Some(store.clone())));

    let payload: Vec<u8> = (0..1024u32).map(|i| i as u8).collect();
    let response = app
        .clone()
        .oneshot(put_glb_request("/api/maps/blob/local.glb", payload.clone()))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "upload to LocalFileSystem must succeed",
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(parsed["file"], "local.glb");
    let wire_size = parsed["size"].as_u64().expect("size");
    assert!(wire_size > 0);

    // Stored under the `.gz` suffix even on LocalFileSystem — no attributes
    // touched, so the backend's `NotImplemented` for `put_opts` is never
    // hit. We confirm by re-fetching and decoding the gzip frame.
    let got = store
        .get(&Path::from("local.glb.gz"))
        .await
        .expect("stored at .gz suffix");
    let stored = got.bytes().await.expect("read stored bytes");
    let mut decoder = GzDecoder::new(&stored[..]);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .expect("gzip decompress");
    assert_eq!(decompressed, payload, "round-trip through gzip");

    // Round-trip the GET handler too, since the read path is what users
    // actually hit. Response body is still gzipped (the browser decodes).
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/maps/blob/local.glb")
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
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut decoder = GzDecoder::new(&body[..]);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .expect("gzip decompress");
    assert_eq!(decompressed, payload);
}
