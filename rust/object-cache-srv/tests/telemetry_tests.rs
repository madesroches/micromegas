use std::sync::Arc;

use axum::Router;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use bytes::Bytes;
use micromegas::object_cache::CacheClientStore;
use micromegas::object_cache::memory_backend::MemoryBackend;
use micromegas::object_cache::range_cache::{
    DEFAULT_DEMAND_RESERVED_FETCH_PERMITS, DEFAULT_MAX_COALESCED_GET_BYTES,
    DEFAULT_PROMOTE_WHOLE_BATCH, DEFAULT_TOTAL_FETCH_PERMITS, RangeCache,
};
use micromegas::tracing::event::in_memory_sink::InMemorySink;
use micromegas::tracing::metrics::MetricsMsgQueueAny;
use micromegas::tracing::test_utils::init_in_memory_tracing;
use micromegas_object_cache_srv::app_state::AppState;
use micromegas_object_cache_srv::handlers::{get_range_handler, head_handler, post_ranges_handler};
use micromegas_transit::HeterogeneousQueue;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};
use serial_test::serial;

const BLOCK_SIZE: u64 = 1024;

fn make_state(origin: Arc<dyn ObjectStore>) -> AppState {
    let cache = RangeCache::new(
        origin,
        Arc::new(MemoryBackend::new()),
        BLOCK_SIZE,
        "test".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );
    // These tests never exercise prefetch; a throwaway sender with a dropped
    // receiver is enough to satisfy `AppState`'s shape.
    let (prefetch_tx, _rx) = tokio::sync::mpsc::channel(1);
    AppState::new(cache, vec!["obj".to_string()], 1024, prefetch_tx)
}

async fn put_bytes(store: &InMemory, key: &str, data: &[u8]) {
    store
        .put(&Path::from(key), Bytes::copy_from_slice(data).into())
        .await
        .expect("put");
}

fn range_header(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("range", value.parse().expect("header value"));
    headers
}

/// Collect `(status, prefix)` tag pairs for every fire of the tagged integer
/// metric `name`. Requires `dispatch::flush_metrics_buffer` to have been
/// called first so buffered events are visible as blocks.
fn tagged_status_prefix_pairs(sink: &InMemorySink, name: &str) -> Vec<(String, String)> {
    let state = sink.state.lock().expect("sink lock");
    let mut out = Vec::new();
    for block in &state.metrics_blocks {
        for evt in block.events.iter() {
            if let MetricsMsgQueueAny::TaggedIntegerMetricEvent(e) = evt
                && e.desc.name == name
            {
                let props = e.properties.get_properties();
                let status = props
                    .iter()
                    .find(|p| p.name.as_str() == "status")
                    .map(|p| p.value.as_str().to_string())
                    .unwrap_or_default();
                let prefix = props
                    .iter()
                    .find(|p| p.name.as_str() == "prefix")
                    .map(|p| p.value.as_str().to_string())
                    .unwrap_or_default();
                out.push((status, prefix));
            }
        }
    }
    out
}

/// Collect every value recorded for the untagged integer metric `name` (e.g.
/// `object_cache_get_bytes_served`, which carries no `status`/`prefix` tags,
/// unlike the metrics `tagged_status_prefix_pairs` collects). Requires
/// `dispatch::flush_metrics_buffer` to have been called first so buffered
/// events are visible as blocks.
fn integer_metric_values(sink: &InMemorySink, name: &str) -> Vec<u64> {
    let state = sink.state.lock().expect("sink lock");
    let mut out = Vec::new();
    for block in &state.metrics_blocks {
        for evt in block.events.iter() {
            if let MetricsMsgQueueAny::IntegerMetricEvent(e) = evt
                && e.desc.name == name
            {
                out.push(e.value);
            }
        }
    }
    out
}

/// Regression guard for the success-only request-counting bug (#1206): every
/// outcome `get_range_handler`'s body can produce -- not just 200/206 -- must
/// bump `object_cache_get_requests`, tagged with the final `status`.
#[tokio::test]
#[serial]
async fn get_range_handler_counts_every_outcome() {
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj/a", &[1u8; 100]).await;
    let state = make_state(store as Arc<dyn ObjectStore>);

    let guard = init_in_memory_tracing();

    // 206: default (no Range header) serves the whole object as a partial
    // response (see `get_range_handler_inner`'s synthesized `bytes=0-N` range).
    let resp = get_range_handler(
        AxumPath("obj/a".to_string()),
        State(state.clone()),
        HeaderMap::new(),
    )
    .await
    .expect("206 response");
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);

    // 400: malformed Range header.
    let err = get_range_handler(
        AxumPath("obj/a".to_string()),
        State(state.clone()),
        range_header("garbage"),
    )
    .await
    .expect_err("bad range header must 400");
    assert_eq!(err, StatusCode::BAD_REQUEST);

    // 404: missing key.
    let err = get_range_handler(
        AxumPath("obj/missing".to_string()),
        State(state.clone()),
        HeaderMap::new(),
    )
    .await
    .expect_err("missing key must 404");
    assert_eq!(err, StatusCode::NOT_FOUND);

    // 416: range past EOF.
    let err = get_range_handler(
        AxumPath("obj/a".to_string()),
        State(state.clone()),
        range_header("bytes=1000-2000"),
    )
    .await
    .expect_err("out-of-bounds range must 416");
    assert_eq!(err, StatusCode::RANGE_NOT_SATISFIABLE);

    micromegas::tracing::dispatch::flush_metrics_buffer();
    let pairs = tagged_status_prefix_pairs(&guard.sink, "object_cache_get_requests");
    let statuses: Vec<&str> = pairs.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(
        statuses.len(),
        4,
        "every outcome (success and failure alike) must be counted exactly once: {pairs:?}"
    );
    assert!(statuses.contains(&"206"));
    assert!(statuses.contains(&"400"));
    assert!(statuses.contains(&"404"));
    assert!(statuses.contains(&"416"));
    // No `with_prefix_labels` was applied to this test's cache, so every key
    // classifies as "other" -- the classifier itself is covered separately
    // in `object-cache/tests/metric_tags_tests.rs`.
    assert!(pairs.iter().all(|(_, p)| p == "other"));
}

/// Same regression guard for `HEAD /obj/{key}`.
#[tokio::test]
#[serial]
async fn head_handler_counts_every_outcome() {
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj/a", &[1u8; 100]).await;
    let state = make_state(store as Arc<dyn ObjectStore>);

    let guard = init_in_memory_tracing();

    // 200: existing key.
    let resp = head_handler(AxumPath("obj/a".to_string()), State(state.clone()))
        .await
        .expect("200 response");
    assert_eq!(resp.status(), StatusCode::OK);

    // 400: key outside the allowed `obj` prefix.
    let err = head_handler(AxumPath("bad/x".to_string()), State(state.clone()))
        .await
        .expect_err("disallowed prefix must 400");
    assert_eq!(err, StatusCode::BAD_REQUEST);

    // 404: missing key.
    let err = head_handler(AxumPath("obj/missing".to_string()), State(state.clone()))
        .await
        .expect_err("missing key must 404");
    assert_eq!(err, StatusCode::NOT_FOUND);

    micromegas::tracing::dispatch::flush_metrics_buffer();
    let pairs = tagged_status_prefix_pairs(&guard.sink, "object_cache_head_requests");
    let statuses: Vec<&str> = pairs.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(
        statuses.len(),
        3,
        "every outcome (success and failure alike) must be counted exactly once: {pairs:?}"
    );
    assert!(statuses.contains(&"200"));
    assert!(statuses.contains(&"400"));
    assert!(statuses.contains(&"404"));
    // No `with_prefix_labels` was applied to this test's cache, so every key
    // classifies as "other" -- the classifier itself is covered separately
    // in `object-cache/tests/metric_tags_tests.rs`.
    assert!(pairs.iter().all(|(_, p)| p == "other"));
}

/// Same regression guard for `POST /ranges`.
#[tokio::test]
#[serial]
async fn post_ranges_handler_counts_every_outcome() {
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj/b", &[2u8; 100]).await;
    let state = make_state(store as Arc<dyn ObjectStore>);

    let guard = init_in_memory_tracing();

    // 200: valid in-bounds ranges.
    let resp = post_ranges_handler(
        AxumPath("obj/b".to_string()),
        State(state.clone()),
        Bytes::from_static(br#"{"ranges":[[0,10]]}"#),
    )
    .await
    .expect("200 response");
    assert_eq!(resp.status(), StatusCode::OK);

    // 400: malformed JSON.
    let err = post_ranges_handler(
        AxumPath("obj/b".to_string()),
        State(state.clone()),
        Bytes::from_static(b"not json"),
    )
    .await
    .expect_err("malformed JSON must 400");
    assert_eq!(err, StatusCode::BAD_REQUEST);

    // 404: missing key.
    let err = post_ranges_handler(
        AxumPath("obj/missing".to_string()),
        State(state.clone()),
        Bytes::from_static(br#"{"ranges":[[0,10]]}"#),
    )
    .await
    .expect_err("missing key must 404");
    assert_eq!(err, StatusCode::NOT_FOUND);

    // 416: range past EOF.
    let err = post_ranges_handler(
        AxumPath("obj/b".to_string()),
        State(state.clone()),
        Bytes::from_static(br#"{"ranges":[[0,1000]]}"#),
    )
    .await
    .expect_err("out-of-bounds range must 416");
    assert_eq!(err, StatusCode::RANGE_NOT_SATISFIABLE);

    micromegas::tracing::dispatch::flush_metrics_buffer();
    let pairs = tagged_status_prefix_pairs(&guard.sink, "object_cache_ranges_requests");
    let statuses: Vec<&str> = pairs.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(
        statuses.len(),
        4,
        "every outcome (success and failure alike) must be counted exactly once: {pairs:?}"
    );
    assert!(statuses.contains(&"200"));
    assert!(statuses.contains(&"400"));
    assert!(statuses.contains(&"404"));
    assert!(statuses.contains(&"416"));
}

/// A `{"ranges":[]}` request must fire `object_cache_ranges_requests`
/// exactly once via the wrapper -- not a second time from the inner
/// handler's empty-ranges short-circuit (the double-count the plan calls
/// out fixing).
#[tokio::test]
#[serial]
async fn empty_ranges_request_is_not_double_counted() {
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj/c", &[3u8; 100]).await;
    let state = make_state(store as Arc<dyn ObjectStore>);

    let guard = init_in_memory_tracing();
    let resp = post_ranges_handler(
        AxumPath("obj/c".to_string()),
        State(state),
        Bytes::from_static(br#"{"ranges":[]}"#),
    )
    .await
    .expect("empty ranges is a valid 200");
    assert_eq!(resp.status(), StatusCode::OK);

    micromegas::tracing::dispatch::flush_metrics_buffer();
    let pairs = tagged_status_prefix_pairs(&guard.sink, "object_cache_ranges_requests");
    assert_eq!(
        pairs.len(),
        1,
        "the wrapper must be the sole emitter of object_cache_ranges_requests: {pairs:?}"
    );
}

/// Reproduction for #1279: `object_cache_get_bytes_served` is emitted from
/// `count_bytes_served`'s `on_complete` callback, which historically only
/// fired once the wrapped stream was polled to a terminal `None`. A `GET`
/// response is framed with an explicit `Content-Length` header, so the
/// transport (hyper) stops polling the body once the declared byte count has
/// been written and never performs that terminal poll -- the metric was a
/// structural zero under real HTTP serving despite every direct-call test in
/// this file passing. Driving the handler through actual HTTP (`axum::serve`
/// plus a real client reading the full body), rather than calling the
/// handler directly or draining its body in-process, is what surfaces the
/// bug and proves the fix.
#[tokio::test]
#[serial]
async fn get_range_bytes_served_fires_over_real_http() {
    let data = vec![7u8; 10 * BLOCK_SIZE as usize];
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj/h", &data).await;
    let state = make_state(store as Arc<dyn ObjectStore>);

    let guard = init_in_memory_tracing();

    let app = Router::new()
        .route("/obj/{*key}", get(get_range_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    // The client's direct fallback store is left empty: a passing assertion
    // on the returned bytes can only be explained by a genuine round trip
    // through the real HTTP server, not a silent fallback.
    let direct = Arc::new(InMemory::new());
    let client = CacheClientStore::new(format!("http://{addr}"), None, direct);
    let path = Path::from("obj/h");
    let range = 0u64..(data.len() as u64);
    let got = client
        .get_range(&path, range.clone())
        .await
        .expect("real HTTP GET must succeed");
    assert_eq!(&got[..], &data[..]);

    micromegas::tracing::dispatch::flush_metrics_buffer();
    let values = integer_metric_values(&guard.sink, "object_cache_get_bytes_served");
    assert_eq!(
        values,
        vec![range.end - range.start],
        "GET bytes-served must be recorded exactly once with the full requested byte count"
    );

    server.abort();
    let _ = server.await;
}

/// Regression guard mirroring the reproduction above for `POST /ranges`:
/// this path already works today (chunked transfer-encoding polls the body
/// to a terminal `None`), but exercising it over real HTTP with the same
/// shape of assertion keeps both call sites symmetric and guards against a
/// future regression in either one.
#[tokio::test]
#[serial]
async fn post_ranges_bytes_served_fires_over_real_http() {
    let data = vec![9u8; 10 * BLOCK_SIZE as usize];
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj/i", &data).await;
    let state = make_state(store as Arc<dyn ObjectStore>);

    let guard = init_in_memory_tracing();

    let app = Router::new()
        .route("/ranges/{*key}", post(post_ranges_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let direct = Arc::new(InMemory::new());
    let client = CacheClientStore::new(format!("http://{addr}"), None, direct);
    let path = Path::from("obj/i");
    let ranges = [0u64..1000, 2000u64..(data.len() as u64)];
    let got = client
        .get_ranges(&path, &ranges)
        .await
        .expect("real HTTP POST /ranges must succeed");
    assert_eq!(got.len(), ranges.len());
    for (chunk, range) in got.iter().zip(ranges.iter()) {
        assert_eq!(&chunk[..], &data[range.start as usize..range.end as usize]);
    }

    let framed_total: u64 =
        ranges.iter().map(|r| r.end - r.start).sum::<u64>() + 8 * ranges.len() as u64;

    micromegas::tracing::dispatch::flush_metrics_buffer();
    let values = integer_metric_values(&guard.sink, "object_cache_ranges_bytes_served");
    assert_eq!(
        values,
        vec![framed_total],
        "ranges bytes-served must be recorded exactly once with the full framed byte count"
    );

    server.abort();
    let _ = server.await;
}
