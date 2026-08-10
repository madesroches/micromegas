// Tests for `micromegas::servers::axum_utils::auth_observability_middleware` --
// verifies the query-string redaction rule from the auth/flightsql
// observability gaps plan: `/auth/*` request/response log lines must carry
// only `uri.path()`, never the query string (which, for `/auth/callback`,
// carries the OAuth authorization code and the signed `state` embedding the
// PKCE verifier).
//
// This only exercises the middleware itself, not routing -- `micromegas`
// (this crate) never sees `analytics-web-srv`'s `build_auth_routes`; that's
// covered separately by `analytics-web-srv/tests/routing_tests.rs`.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware;
use axum::routing::get;
use micromegas::servers::axum_utils::{auth_observability_middleware, observability_middleware};
use micromegas_tracing::event::in_memory_sink::InMemorySink;
use micromegas_tracing::levels::{LevelFilter, set_max_level};
use micromegas_tracing::logs::LogMsgQueueAny;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;
use tower::ServiceExt;

async fn ok_handler() -> StatusCode {
    StatusCode::OK
}

/// `init_in_memory_tracing()` wires up the dispatch but doesn't raise the
/// process-global max log level (only the production `composite_event_sink`
/// init path does that) -- without this, `info!`'s own level check
/// (`Level::Info <= max_level()`) silently drops every call, since the
/// default global level is `LevelFilter::Off`.
fn enable_info_logging() {
    set_max_level(LevelFilter::Trace);
}

/// Walk every collected log block's event queue and collect the dynamic
/// (runtime-formatted) log messages as owned `String`s. The middleware's
/// `info!` calls all have runtime format args, so they record as
/// `LogMsgQueueAny::LogStringEvent`, not the static-string variants.
fn collect_log_messages(sink: &InMemorySink) -> Vec<String> {
    let state = sink.state.lock().expect("sink lock");
    let mut messages = Vec::new();
    for block in &state.log_blocks {
        for event in block.events.iter() {
            if let LogMsgQueueAny::LogStringEvent(evt) = event {
                messages.push(evt.msg.0.clone());
            }
        }
    }
    messages
}

#[tokio::test]
#[serial]
async fn auth_observability_middleware_never_logs_the_query_string() {
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let app = Router::new()
        .route("/auth/callback", get(ok_handler))
        .layer(middleware::from_fn(auth_observability_middleware));

    let request = Request::builder()
        .uri("/auth/callback?code=super-secret-auth-code&state=signed-state-with-pkce-verifier")
        .body(Body::empty())
        .expect("request");

    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    micromegas_tracing::dispatch::flush_log_buffer();

    let messages = collect_log_messages(&guard.sink);
    let request_line = messages
        .iter()
        .find(|m| m.starts_with("request "))
        .expect("a request= log line was captured");
    let response_line = messages
        .iter()
        .find(|m| m.starts_with("response "))
        .expect("a response= log line was captured");

    for line in [request_line, response_line] {
        assert!(
            line.contains("uri=/auth/callback"),
            "expected path-only uri=, got: {line}"
        );
        assert!(
            !line.contains("code="),
            "query string leaked into log line: {line}"
        );
        assert!(
            !line.contains("super-secret-auth-code"),
            "auth code leaked into log line: {line}"
        );
        assert!(
            !line.contains("state="),
            "query string leaked into log line: {line}"
        );
        assert!(
            !line.contains("signed-state-with-pkce-verifier"),
            "PKCE-carrying state leaked into log line: {line}"
        );
    }
}

#[tokio::test]
#[serial]
async fn auth_observability_middleware_logs_path_only_route_unchanged() {
    // A route with no query string at all: `uri=` should still be the path,
    // confirming the redaction rule doesn't corrupt the common case.
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let app = Router::new()
        .route("/auth/me", get(ok_handler))
        .layer(middleware::from_fn(auth_observability_middleware));

    let request = Request::builder()
        .uri("/auth/me")
        .body(Body::empty())
        .expect("request");

    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    micromegas_tracing::dispatch::flush_log_buffer();

    let messages = collect_log_messages(&guard.sink);
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("request ") && m.contains("uri=/auth/me"))
    );
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("response ") && m.contains("uri=/auth/me"))
    );
}

#[tokio::test]
#[serial]
async fn observability_middleware_logs_the_query_string() {
    // Unlike `auth_observability_middleware`, the plain `observability_middleware` (used for
    // `/api/*` and `/ingestion/*`) must keep logging the full query string. This pins the
    // `log_query_string: true` call site in `observability_middleware` against ever being
    // flipped to `false` -- which would silently strip query strings from every such route
    // without failing any other test in this file.
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let app = Router::new()
        .route("/some/path", get(ok_handler))
        .layer(middleware::from_fn(observability_middleware));

    let request = Request::builder()
        .uri("/some/path?foo=bar")
        .body(Body::empty())
        .expect("request");

    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    micromegas_tracing::dispatch::flush_log_buffer();

    let messages = collect_log_messages(&guard.sink);
    let request_line = messages
        .iter()
        .find(|m| m.starts_with("request "))
        .expect("a request= log line was captured");
    let response_line = messages
        .iter()
        .find(|m| m.starts_with("response "))
        .expect("a response= log line was captured");

    for line in [request_line, response_line] {
        assert!(
            line.contains("uri=/some/path?foo=bar"),
            "expected query string to be logged, got: {line}"
        );
    }
}
