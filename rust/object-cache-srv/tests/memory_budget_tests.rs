use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use bytes::Bytes;
use futures::stream::BoxStream;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetRange, GetResult, ListResult, MultipartUpload, ObjectMeta,
    ObjectStore, ObjectStoreExt, PutMultipartOptions, PutOptions, PutPayload, PutResult,
};
use tokio::sync::Semaphore;

use micromegas::object_cache::CacheClientStore;
use micromegas::object_cache::memory_backend::MemoryBackend;
use micromegas::object_cache::range_cache::{
    DEFAULT_DEMAND_RESERVED_FETCH_PERMITS, DEFAULT_MAX_COALESCED_GET_BYTES,
    DEFAULT_PROMOTE_WHOLE_BATCH, DEFAULT_TOTAL_FETCH_PERMITS, RangeCache,
};
use micromegas_object_cache_srv::app_state::AppState;
use micromegas_object_cache_srv::handlers::{
    get_range_handler, head_handler, permits_for_bytes, post_ranges_handler, stream_window_bytes,
};

const BLOCK_SIZE: u64 = 1024 * 1024;

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

fn origin_error(store: &'static str, msg: &str) -> object_store::Error {
    object_store::Error::Generic {
        store,
        source: Box::new(std::io::Error::other(msg.to_string())),
    }
}

/// Every ranged `get_opts` call fails; HEAD (size lookups) always succeeds.
/// Models a dead origin discovered only once a fetch is actually attempted —
/// i.e. after upfront validation (key, size, bounds) has already passed.
#[derive(Debug)]
struct FailingRangedStore {
    inner: Arc<dyn ObjectStore>,
}

impl std::fmt::Display for FailingRangedStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FailingRangedStore({})", self.inner)
    }
}

#[async_trait]
impl ObjectStore for FailingRangedStore {
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
            return Err(origin_error("FailingRangedStore", "origin down"));
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

/// A ranged `get_opts` call fails once its requested start offset reaches
/// `fail_from`; earlier ranges succeed. Lets a test make exactly one fetch
/// window's origin call succeed (so the stream's first byte commits) while a
/// later window's call fails (a genuine mid-stream failure), independent of
/// how the runtime happens to interleave the pipelined window futures.
#[derive(Debug)]
struct FailAtOffsetStore {
    inner: Arc<dyn ObjectStore>,
    fail_from: u64,
}

impl std::fmt::Display for FailAtOffsetStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FailAtOffsetStore({})", self.inner)
    }
}

#[async_trait]
impl ObjectStore for FailAtOffsetStore {
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
        if let Some(object_store::GetRange::Bounded(r)) = &options.range
            && r.start >= self.fail_from
        {
            return Err(origin_error("FailAtOffsetStore", "origin down mid-stream"));
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
        BLOCK_SIZE,
        "test".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );
    // These tests never exercise prefetch; a throwaway sender with a
    // dropped receiver is enough to satisfy `AppState`'s shape.
    let (prefetch_tx, _rx) = tokio::sync::mpsc::channel(1);
    AppState::new(
        cache,
        vec!["obj".to_string()],
        memory_budget_mb,
        prefetch_tx,
    )
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

/// Two large (>= the fixed streaming window) concurrent reads gate on
/// `memory_budget_mb` via the window-capped proportional charge. A budget
/// between one and two windows lets the first read's charge through but
/// leaves too little for the second, which must block until the first's
/// body is dropped.
#[tokio::test]
async fn concurrent_large_reads_gate_on_budget() {
    let window_bytes = stream_window_bytes(BLOCK_SIZE);
    let window_mb = permits_for_bytes(window_bytes);
    let budget = window_mb + window_mb / 2;

    let inner = Arc::new(InMemory::new());
    let data = vec![7u8; window_bytes as usize];
    put_bytes(&inner, "obj/a", &data).await;
    put_bytes(&inner, "obj/b", &data).await;

    let (origin, gate) = DelayedStore::new(inner.clone() as Arc<dyn ObjectStore>);
    let state = make_state(origin as Arc<dyn ObjectStore>, budget);

    let s1 = state.clone();
    let first = tokio::spawn(async move {
        get_range_handler(
            AxumPath("obj/a".to_string()),
            State(s1),
            range_header(0, window_bytes - 1),
        )
        .await
    });

    // The first request's charge (a full window) is acquired before its
    // (gated) origin fetch even starts.
    while state.mem_permits.available_permits() as u32 > budget - window_mb {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        state.mem_permits.available_permits() as u32,
        budget - window_mb
    );

    let s2 = state.clone();
    let second = tokio::spawn(async move {
        get_range_handler(
            AxumPath("obj/b".to_string()),
            State(s2),
            range_header(0, window_bytes - 1),
        )
        .await
    });

    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(
        !second.is_finished(),
        "second large read should block on the exhausted memory budget, not proceed"
    );

    // Budget exhaustion has already been observed above; fully open the
    // origin gate so both requests' remaining fetches can complete.
    gate.add_permits(1000);

    let resp1 = first.await.expect("join").expect("first response");
    assert_eq!(resp1.status(), StatusCode::PARTIAL_CONTENT);

    // Dropping the first response's body releases its permits synchronously,
    // unblocking the second request's budget acquisition.
    drop(resp1.into_body());

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

/// A single-range GET spanning several fetch windows now streams
/// successfully end-to-end with byte-correct framing, where it previously
/// would have hit the (now-removed) 512 MiB / whole-budget 413 rejections.
#[tokio::test]
async fn large_range_streams_across_multiple_windows() {
    let window_bytes = stream_window_bytes(BLOCK_SIZE);
    let total = window_bytes + 4 * 1024 * 1024; // spans 3 fetch windows
    let inner = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    put_bytes(&inner, "obj/big", &data).await;

    let state = make_state(
        inner as Arc<dyn ObjectStore>,
        permits_for_bytes(window_bytes),
    );

    let resp = get_range_handler(
        AxumPath("obj/big".to_string()),
        State(state),
        range_header(0, total - 1),
    )
    .await
    .expect("large range must now stream successfully instead of 413");
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read streamed body");
    assert_eq!(&body[..], &data[..]);
}

/// Scattered small ranges are charged for every distinct block they touch
/// (not just the tiny framed-response size), since each touched block is
/// retained in full for the life of the request — but that charge is still
/// capped at the fixed streaming window, so it stays within the minimum
/// valid budget (`window_mb`, the same floor `object_cache_srv.rs` enforces
/// at startup) and the request streams successfully rather than hanging.
#[tokio::test]
async fn scattered_small_ranges_now_stream_successfully() {
    let inner = Arc::new(InMemory::new());
    let data = vec![3u8; 3 * 1024 * 1024];
    put_bytes(&inner, "obj/z", &data).await;

    let body = Bytes::from_static(br#"{"ranges": [[0,1],[1048576,1048577],[2097152,2097153]]}"#);
    let window_mb = permits_for_bytes(stream_window_bytes(BLOCK_SIZE));
    let state = make_state(inner as Arc<dyn ObjectStore>, window_mb);
    let resp = post_ranges_handler(AxumPath("obj/z".to_string()), State(state), body)
        .await
        .expect("scattered small ranges across few blocks must stream successfully at the minimum valid budget");
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read streamed ranges body");
    let mut cursor = &body_bytes[..];
    let mut chunks = Vec::new();
    for _ in 0..3 {
        let len = u64::from_le_bytes(cursor[..8].try_into().expect("8-byte prefix")) as usize;
        cursor = &cursor[8..];
        chunks.push(cursor[..len].to_vec());
        cursor = &cursor[len..];
    }
    assert_eq!(chunks[0], data[0..1]);
    assert_eq!(chunks[1], data[1048576..1048577]);
    assert_eq!(chunks[2], data[2097152..2097153]);
}

/// A dead origin discovered on the very first fetch window (before any byte
/// has been sent) must surface as 500, not an aborted 200/206 — the
/// commit-before-stream guarantee.
#[tokio::test]
async fn origin_down_before_first_byte_returns_500() {
    let inner = Arc::new(InMemory::new());
    put_bytes(&inner, "obj/dead", &[9u8; 4 * 1024 * 1024]).await;
    let failing = Arc::new(FailingRangedStore {
        inner: inner.clone() as Arc<dyn ObjectStore>,
    });
    let state = make_state(failing as Arc<dyn ObjectStore>, 1024);

    let err = get_range_handler(
        AxumPath("obj/dead".to_string()),
        State(state),
        range_header(0, 1024 * 1024 - 1),
    )
    .await
    .expect_err("a dead origin before the first byte must surface as 500");
    assert_eq!(err, StatusCode::INTERNAL_SERVER_ERROR);
}

/// A mid-stream origin failure (after the first fetch window has already
/// committed the response) truncates the framing on the wire; the served
/// `POST /ranges` response ends in an error and `CacheClientStore::get_ranges`
/// detects the truncation/transport failure and falls back to the direct
/// store, returning correct data.
#[tokio::test]
async fn mid_stream_origin_failure_falls_back_to_direct_via_client() {
    let window_bytes = stream_window_bytes(BLOCK_SIZE); // 16 MiB
    let fetch_window_bytes = window_bytes / 2; // one `DEMAND_WINDOW_BLOCKS` fetch window: 8 MiB
    let total = window_bytes + 4 * 1024 * 1024; // spans 3 fetch windows

    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();

    let origin_data = Arc::new(InMemory::new());
    put_bytes(&origin_data, "obj/flaky", &data).await;
    let flaky_origin = Arc::new(FailAtOffsetStore {
        inner: origin_data as Arc<dyn ObjectStore>,
        fail_from: fetch_window_bytes,
    });
    let state = make_state(
        flaky_origin as Arc<dyn ObjectStore>,
        permits_for_bytes(window_bytes),
    );

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

    // The client's direct fallback store has the same, uncorrupted data, so
    // a correct fallback read is distinguishable from a truncated one.
    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/flaky", &data).await;

    let client = CacheClientStore::new(format!("http://{addr}"), None, direct);
    #[allow(clippy::single_range_in_vec_init)]
    let got = client
        .get_ranges(&Path::from("obj/flaky"), &[0..total])
        .await
        .expect("client must fall back to direct on truncated framing");
    assert_eq!(got.len(), 1);
    assert_eq!(&got[0][..], &data[..]);

    server.abort();
    let _ = server.await;
}

/// `CacheClientStore::get_opts` now streams ranged GETs instead of buffering
/// them with `.bytes()` (see `get_range_stream` in `client.rs`); exercise all
/// three `GetRange` shapes end to end against a real `get_range_handler` /
/// `head_handler` server, spanning several fetch windows so the streamed
/// body is delivered in multiple chunks. The client's fallback `direct`
/// store deliberately holds different bytes, so a passing assertion can only
/// be explained by the streamed cache path actually running, not a silent
/// fallback.
#[tokio::test]
async fn ranged_get_via_client_streams_bounded_offset_and_suffix() {
    let window_bytes = stream_window_bytes(BLOCK_SIZE); // 16 MiB
    let total = window_bytes + 4 * 1024 * 1024; // spans several fetch windows

    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();

    let origin = Arc::new(InMemory::new());
    put_bytes(&origin, "obj/g", &data).await;
    let state = make_state(
        origin as Arc<dyn ObjectStore>,
        permits_for_bytes(window_bytes) * 4,
    );

    let app = Router::new()
        .route("/obj/{*key}", get(get_range_handler).head(head_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    // Mismatched fallback data: only a genuine streamed read from the cache
    // server can produce a correct result below.
    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/g", &vec![0u8; data.len()]).await;

    let client = CacheClientStore::new(format!("http://{addr}"), None, direct);
    let path = Path::from("obj/g");

    // Bounded range spanning multiple fetch windows.
    let bounded_range = 1_000u64..(total - 1_000);
    let bounded = client
        .get_range(&path, bounded_range.clone())
        .await
        .expect("bounded get_range");
    assert_eq!(
        &bounded[..],
        &data[bounded_range.start as usize..bounded_range.end as usize]
    );

    // Open-ended range.
    let offset = total - 12345;
    let offset_result = client
        .get_opts(
            &path,
            GetOptions {
                range: Some(GetRange::Offset(offset)),
                ..Default::default()
            },
        )
        .await
        .expect("offset get_opts");
    assert_eq!(offset_result.meta.size, total);
    assert_eq!(offset_result.range, offset..total);
    let offset_bytes = offset_result.bytes().await.expect("offset bytes");
    assert_eq!(&offset_bytes[..], &data[offset as usize..]);

    // Suffix range.
    let suffix_len = 54321u64;
    let suffix_result = client
        .get_opts(
            &path,
            GetOptions {
                range: Some(GetRange::Suffix(suffix_len)),
                ..Default::default()
            },
        )
        .await
        .expect("suffix get_opts");
    assert_eq!(suffix_result.meta.size, total);
    assert_eq!(suffix_result.range, (total - suffix_len)..total);
    let suffix_bytes = suffix_result.bytes().await.expect("suffix bytes");
    assert_eq!(&suffix_bytes[..], &data[(total - suffix_len) as usize..]);

    server.abort();
    let _ = server.await;
}
