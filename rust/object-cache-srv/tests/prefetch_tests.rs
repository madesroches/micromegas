use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use bytes::Bytes;
use futures::stream::BoxStream;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    ObjectStoreExt, PutMultipartOptions, PutOptions, PutPayload, PutResult,
};

use micromegas_object_cache::foyer_backend::FoyerBackend;
use micromegas_object_cache::memory_backend::MemoryBackend;
use micromegas_object_cache::prefetch::{PrefetchItem, PrefetchRequest, PrefetchResponse};
use micromegas_object_cache::range_cache::{
    DEFAULT_DEMAND_RESERVED_FETCH_PERMITS, DEFAULT_MAX_COALESCED_GET_BYTES,
    DEFAULT_PROMOTE_WHOLE_BATCH, DEFAULT_TOTAL_FETCH_PERMITS, RangeCache,
};
use micromegas_object_cache_srv::app_state::AppState;
use micromegas_object_cache_srv::handlers::{get_range_handler, prefetch_handler};
use micromegas_object_cache_srv::prefetch_queue::spawn_prefetch_worker;

/// Wraps an `ObjectStore`, counting `get_range` calls and, when constructed
/// `with_gate`, blocking each one on a semaphore until the test releases it.
/// HEAD requests always pass straight through.
#[derive(Debug)]
struct CountingStore {
    inner: Arc<dyn ObjectStore>,
    get_range_calls: AtomicUsize,
    gate: Option<Arc<tokio::sync::Semaphore>>,
}

impl CountingStore {
    fn new(inner: Arc<dyn ObjectStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            get_range_calls: AtomicUsize::new(0),
            gate: None,
        })
    }

    fn with_gate(inner: Arc<dyn ObjectStore>) -> (Arc<Self>, Arc<tokio::sync::Semaphore>) {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let store = Arc::new(Self {
            inner,
            get_range_calls: AtomicUsize::new(0),
            gate: Some(gate.clone()),
        });
        (store, gate)
    }

    fn get_range_count(&self) -> usize {
        self.get_range_calls.load(Ordering::SeqCst)
    }
}

impl std::fmt::Display for CountingStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CountingStore({})", self.inner)
    }
}

#[async_trait]
impl ObjectStore for CountingStore {
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
            self.get_range_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.gate {
                gate.acquire().await.expect("gate never closed").forget();
            }
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

fn memory_cache(origin: Arc<dyn ObjectStore>, block_size: u64) -> RangeCache {
    RangeCache::new(
        origin,
        Arc::new(MemoryBackend::new()),
        block_size,
        "test".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    )
}

async fn call_prefetch(state: &AppState, req: &PrefetchRequest) -> PrefetchResponse {
    let body = Bytes::from(serde_json::to_vec(req).expect("serialize PrefetchRequest"));
    let resp = prefetch_handler(State(state.clone()), body)
        .await
        .expect("prefetch_handler response");
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read prefetch response body");
    serde_json::from_slice(&body_bytes).expect("deserialize PrefetchResponse")
}

#[tokio::test]
async fn prefetch_warms_cache_for_later_demand_read() {
    let block_size = 1024u64;
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(4 * block_size as usize).collect();
    put_bytes(&store, "obj/a", &data).await;

    let counting = CountingStore::new(store.clone() as Arc<dyn ObjectStore>);
    let cache = memory_cache(counting.clone() as Arc<dyn ObjectStore>, block_size);
    let (prefetch_tx, worker) = spawn_prefetch_worker(cache.clone(), 16, 4);
    let state = AppState::new(cache.clone(), vec!["obj".to_string()], 1024, prefetch_tx);

    let req = PrefetchRequest {
        keys: vec![PrefetchItem {
            key: "obj/a".to_string(),
            size: data.len() as u64,
            ranges: None,
        }],
    };
    let resp = call_prefetch(&state, &req).await;
    assert_eq!(resp.accepted, 1);
    assert_eq!(resp.rejected, 0);
    assert_eq!(resp.dropped, 0);

    // Drain the worker: dropping every `AppState`/sender clone closes the
    // channel, and awaiting the handle guarantees every spawned fill has
    // completed (MemoryBackend needs no separate flush step).
    drop(state);
    worker.await.expect("worker task join");

    let gets_from_prefetch = counting.get_range_count();
    assert!(gets_from_prefetch > 0, "prefetch should have hit origin");

    let got = cache
        .get_range("obj/a", 0..data.len() as u64)
        .await
        .expect("demand read after warming");
    assert_eq!(&got[..], &data[..]);
    assert_eq!(
        counting.get_range_count(),
        gets_from_prefetch,
        "demand read must be served from cache, not origin"
    );
}

#[tokio::test]
async fn prefetch_ssd_only_leaves_ram_usage_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");

    let block_size = 1024u64;
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = vec![7u8; 2 * block_size as usize];
    put_bytes(&store, "obj/b", &data).await;

    let counting = CountingStore::new(store.clone() as Arc<dyn ObjectStore>);
    let foyer = Arc::new(
        FoyerBackend::new_with_shards(dir_path, 16 * 1024 * 1024, 16 * 1024 * 1024, 1)
            .await
            .expect("create FoyerBackend"),
    );
    let cache = RangeCache::new(
        counting.clone() as Arc<dyn ObjectStore>,
        foyer.clone(),
        block_size,
        "test".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );
    let (prefetch_tx, worker) = spawn_prefetch_worker(cache.clone(), 16, 4);
    let state = AppState::new(cache.clone(), vec!["obj".to_string()], 1024, prefetch_tx);

    let ram_before = foyer.ram_usage();

    let req = PrefetchRequest {
        keys: vec![PrefetchItem {
            key: "obj/b".to_string(),
            size: data.len() as u64,
            ranges: None,
        }],
    };
    let resp = call_prefetch(&state, &req).await;
    assert_eq!(resp.accepted, 1);

    drop(state);
    worker.await.expect("worker task join");
    // Flush the SSD tier: the ephemeral RAM record's async disk write must be
    // durable before reads/assertions below.
    foyer.close().await.expect("close backend");

    let ram_after = foyer.ram_usage();
    assert_eq!(
        ram_after, ram_before,
        "a prefetch fill must not grow RAM-tier usage"
    );

    let gets_from_prefetch = counting.get_range_count();
    assert!(gets_from_prefetch > 0);
    let got = cache
        .get_range("obj/b", 0..data.len() as u64)
        .await
        .expect("demand read after SSD-only warming");
    assert_eq!(&got[..], &data[..]);
    assert_eq!(
        counting.get_range_count(),
        gets_from_prefetch,
        "demand read should be served from the SSD tier, not origin"
    );
}

#[tokio::test]
async fn zero_size_prefetch_is_a_no_op() {
    // Exercises the empty-span guard end-to-end: `blocks_for_range` underflows
    // on `end == 0` (debug_assert! in test builds), so a broken guard would
    // panic this test rather than silently pass.
    let store = Arc::new(InMemory::new());
    let counting = CountingStore::new(store as Arc<dyn ObjectStore>);
    let cache = memory_cache(counting.clone() as Arc<dyn ObjectStore>, 1024);
    let (prefetch_tx, worker) = spawn_prefetch_worker(cache.clone(), 16, 4);
    let state = AppState::new(cache, vec!["obj".to_string()], 1024, prefetch_tx);

    let req = PrefetchRequest {
        keys: vec![PrefetchItem {
            key: "obj/empty".to_string(),
            size: 0,
            ranges: None,
        }],
    };
    let resp = call_prefetch(&state, &req).await;
    assert_eq!(resp.accepted, 1);

    drop(state);
    worker.await.expect("worker task join");

    assert_eq!(
        counting.get_range_count(),
        0,
        "size == 0 must produce an empty window set, not an origin call"
    );
}

#[tokio::test]
async fn full_queue_load_sheds_excess_items() {
    // No consumer drains this channel, so its single buffer slot fills
    // immediately and every subsequent `try_send` observes `Full`
    // deterministically -- no timing dependency needed.
    let (prefetch_tx, _rx) = tokio::sync::mpsc::channel::<PrefetchItem>(1);

    let store = Arc::new(InMemory::new());
    let cache = memory_cache(store as Arc<dyn ObjectStore>, 1024);
    let state = AppState::new(cache, vec!["obj".to_string()], 1024, prefetch_tx);

    let req = PrefetchRequest {
        keys: vec![
            PrefetchItem {
                key: "obj/a".to_string(),
                size: 10,
                ranges: None,
            },
            PrefetchItem {
                key: "obj/b".to_string(),
                size: 10,
                ranges: None,
            },
            PrefetchItem {
                key: "obj/c".to_string(),
                size: 10,
                ranges: None,
            },
        ],
    };
    let resp = call_prefetch(&state, &req).await;
    assert_eq!(resp.accepted, 1, "only the first item fits the queue");
    assert_eq!(resp.rejected, 0);
    assert_eq!(resp.dropped, 2, "the rest must be load-shed, not blocked");
}

#[tokio::test]
async fn invalid_items_are_rejected_valid_ones_still_enqueued() {
    let (prefetch_tx, _rx) = tokio::sync::mpsc::channel::<PrefetchItem>(16);
    let store = Arc::new(InMemory::new());
    let cache = memory_cache(store as Arc<dyn ObjectStore>, 1024);
    let state = AppState::new(cache, vec!["obj".to_string()], 1024, prefetch_tx);

    let req = PrefetchRequest {
        keys: vec![
            // Outside the allowed prefix.
            PrefetchItem {
                key: "secret/x".to_string(),
                size: 10,
                ranges: None,
            },
            // Inverted range.
            PrefetchItem {
                key: "obj/a".to_string(),
                size: 10,
                ranges: Some(vec![[5, 3]]),
            },
            // Out-of-bounds range.
            PrefetchItem {
                key: "obj/a".to_string(),
                size: 10,
                ranges: Some(vec![[0, 20]]),
            },
            // Valid.
            PrefetchItem {
                key: "obj/valid".to_string(),
                size: 10,
                ranges: None,
            },
        ],
    };
    let resp = call_prefetch(&state, &req).await;
    assert_eq!(resp.accepted, 1);
    assert_eq!(resp.rejected, 3);
    assert_eq!(resp.dropped, 0);
}

#[tokio::test]
async fn huge_declared_size_is_accepted_no_cap() {
    // There is no per-item size cap: the streaming worker (not the handler)
    // is what bounds per-item work, so even a garbage `u64::MAX` size must be
    // accepted rather than rejected.
    let (prefetch_tx, _rx) = tokio::sync::mpsc::channel::<PrefetchItem>(16);
    let store = Arc::new(InMemory::new());
    let cache = memory_cache(store as Arc<dyn ObjectStore>, 1024);
    let state = AppState::new(cache, vec!["obj".to_string()], 1024, prefetch_tx);

    let req = PrefetchRequest {
        keys: vec![PrefetchItem {
            key: "obj/huge".to_string(),
            size: u64::MAX,
            ranges: None,
        }],
    };
    let resp = call_prefetch(&state, &req).await;
    assert_eq!(resp.accepted, 1);
    assert_eq!(resp.rejected, 0);
    assert_eq!(resp.dropped, 0);
}

#[tokio::test]
async fn oversized_declared_size_streams_and_stops_at_true_eof() {
    // The worker streams block-index windows in chunks of WINDOW_BLOCKS (64,
    // `prefetch_queue.rs`). Picking a small block_size here keeps the test
    // fast while still exercising several full windows before the declared
    // size runs the stream off the end of the real object.
    let block_size = 16u64;
    let true_size = 5 * 64 * block_size; // exactly 5 full windows: bytes [0, 5120)
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(true_size as usize).collect();
    put_bytes(&store, "obj/huge", &data).await;

    let counting = CountingStore::new(store.clone() as Arc<dyn ObjectStore>);
    let cache = memory_cache(counting.clone() as Arc<dyn ObjectStore>, block_size);
    let (prefetch_tx, worker) = spawn_prefetch_worker(cache.clone(), 16, 4);
    let state = AppState::new(cache.clone(), vec!["obj".to_string()], 1024, prefetch_tx);

    // A garbage caller-supplied size: the handler applies no size cap, and the
    // streaming worker must not OOM or hang -- it streams windows lazily and
    // stops at the first origin error past the true EOF.
    let req = PrefetchRequest {
        keys: vec![PrefetchItem {
            key: "obj/huge".to_string(),
            size: u64::MAX,
            ranges: None,
        }],
    };
    let resp = call_prefetch(&state, &req).await;
    assert_eq!(resp.accepted, 1);

    drop(state);
    worker.await.expect("worker task join");

    // The windows fully inside the true object were warmed before the worker
    // hit the first out-of-bounds window and stopped.
    let gets_from_prefetch = counting.get_range_count();
    assert!(
        gets_from_prefetch > 0,
        "the in-bounds windows must have been fetched"
    );

    let got = cache
        .get_range("obj/huge", 0..true_size)
        .await
        .expect("demand read over the real object");
    assert_eq!(&got[..], &data[..]);
    assert_eq!(
        counting.get_range_count(),
        gets_from_prefetch,
        "the real range must be served entirely from cache, not origin"
    );
}

#[tokio::test]
async fn prefetch_never_acquires_a_mem_permit() {
    // capacity 16 is generous enough that the item is never dropped, but no
    // consumer runs, so the (large, declared) fill never actually executes --
    // this test only needs to observe the handler's own behavior.
    let (prefetch_tx, _rx) = tokio::sync::mpsc::channel::<PrefetchItem>(16);
    let store = Arc::new(InMemory::new());
    let cache = memory_cache(store as Arc<dyn ObjectStore>, 1024 * 1024);
    // A 1 MiB budget: a demand request for this many bytes would need far
    // more permits than exist and would be rejected with 413.
    let state = AppState::new(cache, vec!["obj".to_string()], 1, prefetch_tx);
    let permits_before = state.mem_permits.available_permits();

    let req = PrefetchRequest {
        keys: vec![PrefetchItem {
            key: "obj/huge".to_string(),
            size: 100 * 1024 * 1024, // 100 MiB, far past the 1 MiB budget
            ranges: None,
        }],
    };
    let resp = call_prefetch(&state, &req).await;
    assert_eq!(
        resp.accepted, 1,
        "prefetch must succeed regardless of the memory budget"
    );
    assert_eq!(
        state.mem_permits.available_permits(),
        permits_before,
        "prefetch must never acquire a mem_permit"
    );
}

#[tokio::test]
async fn prefetch_priority_does_not_starve_demand_read() {
    let block_size = 1024u64;
    let file_size = 20 * block_size;
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj/x", &vec![1u8; file_size as usize]).await;
    let (counting, gate) = CountingStore::with_gate(store.clone() as Arc<dyn ObjectStore>);

    let total = 4usize;
    let demand_reserved = 1usize;
    let cache = RangeCache::new(
        counting.clone() as Arc<dyn ObjectStore>,
        Arc::new(MemoryBackend::new()),
        block_size,
        "test".to_string(),
        total,
        demand_reserved,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );
    let (prefetch_tx, worker) = spawn_prefetch_worker(cache.clone(), 16, 8);
    let state = AppState::new(cache.clone(), vec!["obj".to_string()], 1024, prefetch_tx);

    // 5 scattered (non-adjacent) single-block ranges -> 5 separate prefetch
    // runs, submitted as one batch.
    let scattered = [0u64, 2, 4, 6, 8];
    let req = PrefetchRequest {
        keys: scattered
            .iter()
            .map(|&idx| PrefetchItem {
                key: "obj/x".to_string(),
                size: file_size,
                ranges: Some(vec![[idx * block_size, idx * block_size + 1]]),
            })
            .collect(),
    };
    let resp = call_prefetch(&state, &req).await;
    assert_eq!(resp.accepted, 5);

    // prefetch_permits = total - demand_reserved = 3: only 3 of the 5
    // scattered prefetch runs can be in flight; the rest queue behind them.
    while counting.get_range_count() < 3 {
        tokio::task::yield_now().await;
    }
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        counting.get_range_count(),
        3,
        "prefetch must not exceed its budget"
    );

    let demand_range = 10 * block_size..10 * block_size + 10;
    let demand = tokio::spawn({
        let cache = cache.clone();
        async move { cache.get_range("obj/x", demand_range).await }
    });

    while counting.get_range_count() < 4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        counting.get_range_count(),
        4,
        "demand must find a reserved permit despite prefetch saturation"
    );

    gate.add_permits(10);
    let got = demand.await.expect("join").expect("get_range");
    assert_eq!(got.len(), 10);

    drop(state);
    worker.await.expect("worker task join");
}

#[tokio::test]
async fn client_prefetch_round_trip_over_http() {
    let block_size = 1024u64;
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(4 * block_size as usize).collect();
    put_bytes(&store, "obj/c", &data).await;

    let counting = CountingStore::new(store.clone() as Arc<dyn ObjectStore>);
    let cache = memory_cache(counting.clone() as Arc<dyn ObjectStore>, block_size);
    let (prefetch_tx, worker) = spawn_prefetch_worker(cache.clone(), 16, 4);
    let state = AppState::new(cache.clone(), vec!["obj".to_string()], 1024, prefetch_tx);

    let app = Router::new()
        .route("/prefetch", post(prefetch_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let client = micromegas_object_cache::CacheClientStore::new(
        format!("http://{addr}"),
        None,
        Arc::new(InMemory::new()),
    );
    let resp = client
        .prefetch(vec![PrefetchItem {
            key: "obj/c".to_string(),
            size: data.len() as u64,
            ranges: None,
        }])
        .await
        .expect("prefetch over http");
    assert_eq!(resp.accepted, 1);

    // Shut the server down: this drops the only remaining `AppState` clone
    // (the router's), which drops the last `prefetch_tx` sender and closes
    // the channel, letting the worker drain to completion.
    server.abort();
    let _ = server.await;
    worker.await.expect("worker task join");

    let got = cache
        .get_range("obj/c", 0..data.len() as u64)
        .await
        .expect("demand read after warming");
    assert_eq!(&got[..], &data[..]);
    assert!(counting.get_range_count() > 0);
}

#[tokio::test]
async fn client_prefetch_returns_err_when_unreachable() {
    // Port 1 is reserved and never listens; the connect attempt fails fast.
    let client = micromegas_object_cache::CacheClientStore::new(
        "http://127.0.0.1:1".to_string(),
        None,
        Arc::new(InMemory::new()),
    );
    let result = client
        .prefetch(vec![PrefetchItem {
            key: "obj/x".to_string(),
            size: 10,
            ranges: None,
        }])
        .await;
    assert!(result.is_err(), "unreachable server must yield Err");
}

#[tokio::test]
async fn get_range_handler_still_works_alongside_prefetch_state() {
    // Sanity check that adding `prefetch_tx` to `AppState` didn't disturb the
    // existing demand-read handler.
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj/d", &[3u8; 4096]).await;
    let cache = memory_cache(store as Arc<dyn ObjectStore>, 1024);
    let (prefetch_tx, _rx) = tokio::sync::mpsc::channel::<PrefetchItem>(1);
    let state = AppState::new(cache, vec!["obj".to_string()], 1024, prefetch_tx);

    let resp = get_range_handler(
        axum::extract::Path("obj/d".to_string()),
        State(state),
        axum::http::HeaderMap::new(),
    )
    .await
    .expect("get_range_handler response");
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
}
