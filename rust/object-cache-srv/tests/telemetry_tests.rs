use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;
use micromegas_object_cache::memory_backend::MemoryBackend;
use micromegas_object_cache::range_cache::{
    DEFAULT_DEMAND_RESERVED_FETCH_PERMITS, DEFAULT_MAX_COALESCED_GET_BYTES,
    DEFAULT_PROMOTE_WHOLE_BATCH, DEFAULT_TOTAL_FETCH_PERMITS, RangeCache,
};
use micromegas_object_cache_srv::app_state::AppState;
use micromegas_object_cache_srv::handlers::{get_range_handler, head_handler, post_ranges_handler};
use micromegas_tracing::event::in_memory_sink::InMemorySink;
use micromegas_tracing::metrics::MetricsMsgQueueAny;
use micromegas_tracing::test_utils::init_in_memory_tracing;
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

    micromegas_tracing::dispatch::flush_metrics_buffer();
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

    micromegas_tracing::dispatch::flush_metrics_buffer();
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

    micromegas_tracing::dispatch::flush_metrics_buffer();
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

    micromegas_tracing::dispatch::flush_metrics_buffer();
    let pairs = tagged_status_prefix_pairs(&guard.sink, "object_cache_ranges_requests");
    assert_eq!(
        pairs.len(),
        1,
        "the wrapper must be the sole emitter of object_cache_ranges_requests: {pairs:?}"
    );
}
