use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;
use futures::stream::BoxStream;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    ObjectStoreExt, PutMultipartOptions, PutOptions, PutPayload, PutResult,
};
use tokio::sync::Semaphore;

use micromegas_object_cache::memory_backend::MemoryBackend;
use micromegas_object_cache::range_cache::{
    DEFAULT_DEMAND_RESERVED_FETCH_PERMITS, DEFAULT_MAX_COALESCED_GET_BYTES,
    DEFAULT_PROMOTE_WHOLE_BATCH, DEFAULT_TOTAL_FETCH_PERMITS, RangeCache,
};
use micromegas_object_cache_srv::app_state::AppState;
use micromegas_object_cache_srv::handlers::{get_range_handler, post_ranges_handler};

/// Wraps an `ObjectStore`, blocking every *ranged* `get_opts` call (i.e. every
/// `get_range`) on a semaphore gate until the test calls `add_permits`. HEAD
/// requests pass straight through so `RangeCache::size` never contends with
/// the gate. Lets tests hold an origin fetch open to observe memory-budget
/// gating deterministically.
#[derive(Debug)]
struct DelayedStore {
    inner: Arc<dyn ObjectStore>,
    gate: Arc<Semaphore>,
}

impl DelayedStore {
    fn new(inner: Arc<dyn ObjectStore>) -> (Arc<Self>, Arc<Semaphore>) {
        let gate = Arc::new(Semaphore::new(0));
        (
            Arc::new(Self {
                inner,
                gate: gate.clone(),
            }),
            gate,
        )
    }
}

impl std::fmt::Display for DelayedStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DelayedStore({})", self.inner)
    }
}

#[async_trait]
impl ObjectStore for DelayedStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        if options.range.is_some() {
            self.gate
                .acquire()
                .await
                .expect("gate never closed")
                .forget();
        }
        self.inner.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        self.inner.delete_stream(locations)
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        self.inner.copy_opts(from, to, options).await
    }
}

async fn put_bytes(store: &InMemory, key: &str, data: &[u8]) {
    store
        .put(&Path::from(key), Bytes::copy_from_slice(data).into())
        .await
        .expect("put");
}

fn make_state(origin: Arc<dyn ObjectStore>, memory_budget_mb: u32) -> AppState {
    let cache = RangeCache::new(
        origin,
        Arc::new(MemoryBackend::new()),
        1024 * 1024, // 1 MiB blocks
        "test".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );
    AppState::new(cache, vec!["obj".to_string()], memory_budget_mb)
}

fn range_header(start: u64, end_inclusive: u64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "range",
        format!("bytes={start}-{end_inclusive}")
            .parse()
            .expect("header value"),
    );
    headers
}

#[tokio::test]
async fn concurrent_large_reads_gate_on_budget() {
    let inner = Arc::new(InMemory::new());
    let data = vec![7u8; 4 * 1024 * 1024];
    put_bytes(&inner, "obj/a", &data).await;
    put_bytes(&inner, "obj/b", &data).await;

    let (origin, gate) = DelayedStore::new(inner.clone() as Arc<dyn ObjectStore>);
    let state = make_state(origin as Arc<dyn ObjectStore>, 2);

    let s1 = state.clone();
    let first = tokio::spawn(async move {
        get_range_handler(
            AxumPath("obj/a".to_string()),
            State(s1),
            range_header(0, 2 * 1024 * 1024 - 1),
        )
        .await
    });

    // The first request acquires both mem permits before its (gated) origin
    // fetch even starts.
    while state.mem_permits.available_permits() > 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(state.mem_permits.available_permits(), 0);

    let s2 = state.clone();
    let second = tokio::spawn(async move {
        get_range_handler(
            AxumPath("obj/b".to_string()),
            State(s2),
            range_header(0, 1024 * 1024 - 1),
        )
        .await
    });

    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(
        !second.is_finished(),
        "second request should block on the exhausted memory budget, not proceed"
    );

    // Let the first request's origin fetch complete.
    gate.add_permits(1);
    let resp1 = first.await.expect("join").expect("first response");
    assert_eq!(resp1.status(), StatusCode::PARTIAL_CONTENT);

    // Dropping the first response's body releases its permits, unblocking
    // the second request's budget acquisition.
    drop(resp1.into_body());

    gate.add_permits(1);
    let resp2 = second.await.expect("join").expect("second response");
    assert_eq!(resp2.status(), StatusCode::PARTIAL_CONTENT);
}

#[tokio::test]
async fn permit_released_on_body_drop() {
    let inner = Arc::new(InMemory::new());
    let data = vec![1u8; 2 * 1024 * 1024];
    put_bytes(&inner, "obj/x", &data).await;
    let state = make_state(inner as Arc<dyn ObjectStore>, 2);

    let resp = get_range_handler(
        AxumPath("obj/x".to_string()),
        State(state.clone()),
        range_header(0, 2 * 1024 * 1024 - 1),
    )
    .await
    .expect("response");
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        state.mem_permits.available_permits(),
        0,
        "permits held while the response body is alive"
    );

    // Dropping the body's boxed stream drops the `PermitBody` wrapper
    // synchronously (regular `Drop`, not async), releasing its permits
    // immediately without needing to poll/consume the stream.
    drop(resp.into_body());
    assert_eq!(
        state.mem_permits.available_permits(),
        2,
        "permits released once the body is dropped"
    );
}

#[tokio::test]
async fn scattered_small_ranges_charge_blocks_touched() {
    let inner = Arc::new(InMemory::new());
    let data = vec![3u8; 3 * 1024 * 1024];
    put_bytes(&inner, "obj/z", &data).await;

    // Three 1-byte ranges in three distinct 1 MiB blocks retain three full
    // blocks during assembly, so they must be charged 3 permits (blocks
    // touched), not 1 (requested bytes) — exceeding a 2 MiB budget.
    let body = Bytes::from_static(br#"{"ranges": [[0,1],[1048576,1048577],[2097152,2097153]]}"#);
    let state = make_state(inner.clone() as Arc<dyn ObjectStore>, 2);
    let err = post_ranges_handler(
        AxumPath("obj/z".to_string()),
        State(state.clone()),
        body.clone(),
    )
    .await
    .expect_err("blocks touched exceed the whole budget");
    assert_eq!(err, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(state.mem_permits.available_permits(), 2);

    // With a 4 MiB budget the same request goes through, holding 3 permits
    // for the response body's lifetime.
    let state = make_state(inner as Arc<dyn ObjectStore>, 4);
    let resp = post_ranges_handler(AxumPath("obj/z".to_string()), State(state.clone()), body)
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        state.mem_permits.available_permits(),
        1,
        "permits charged for the three distinct blocks touched"
    );
    drop(resp.into_body());
    assert_eq!(state.mem_permits.available_permits(), 4);
}

#[tokio::test]
async fn oversize_request_rejected_413() {
    let inner = Arc::new(InMemory::new());
    let data = vec![1u8; 4 * 1024 * 1024];
    put_bytes(&inner, "obj/y", &data).await;
    let state = make_state(inner as Arc<dyn ObjectStore>, 1);

    let err = get_range_handler(
        AxumPath("obj/y".to_string()),
        State(state.clone()),
        range_header(0, 2 * 1024 * 1024 - 1),
    )
    .await
    .expect_err("request larger than the whole budget must be rejected");
    assert_eq!(err, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        state.mem_permits.available_permits(),
        1,
        "no permits should be held after an outright rejection"
    );
}
