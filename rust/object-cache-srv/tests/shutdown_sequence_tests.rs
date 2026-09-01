//! Integration coverage for `shutdown_sequence::run`: the post-axum
//! orchestration that aborts the prefetch worker, drains in-flight origin
//! fetch tasks, then closes the foyer cache.
//!
//! The tests below drive `run` against real components (a real
//! `spawn_prefetch_worker` handle, a real `FoyerBackend`-backed `RangeCache`)
//! and assert genuinely observable behavior: a queued prefetch item
//! abandoned by the abort, the fetch-task drain actually blocking on real
//! outstanding work, the overall deadline actually bounding `run`'s
//! wall-clock time, and `close()` actually persisting to disk. Driving `run`
//! against stub doubles instead would only prove that its three awaits
//! execute in the order they're written, which is already guaranteed by its
//! single linear async body.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    ObjectStoreExt, PutMultipartOptions, PutOptions, PutPayload, PutResult,
};

use micromegas::object_cache::backend::{FillHint, RangeCacheBackend};
use micromegas::object_cache::foyer_backend::{FoyerBackend, WriteTuning};
use micromegas::object_cache::prefetch::PrefetchItem;
use micromegas::object_cache::range_cache::{
    DEFAULT_DEMAND_RESERVED_FETCH_PERMITS, DEFAULT_MAX_COALESCED_GET_BYTES,
    DEFAULT_PROMOTE_WHOLE_BATCH, DEFAULT_TOTAL_FETCH_PERMITS, RangeCache,
};
use micromegas_object_cache_srv::prefetch_queue::spawn_prefetch_worker;
use micromegas_object_cache_srv::shutdown_sequence;

/// Wraps an `ObjectStore`, counting ranged `get_range` calls and, when
/// constructed `with_gate`, blocking each one on a semaphore until the test
/// releases it. HEAD requests always pass straight through. Duplicated from
/// `prefetch_tests.rs`'s identical double: each integration-test file in
/// this crate compiles as its own binary, so there's no import path to share
/// it without hoisting it into a new shared module -- not worth it for one
/// more call site.
#[derive(Debug)]
struct CountingStore {
    inner: Arc<dyn ObjectStore>,
    get_range_calls: AtomicUsize,
    gate: Option<Arc<tokio::sync::Semaphore>>,
}

impl CountingStore {
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

async fn new_foyer(dir: &str) -> Arc<FoyerBackend> {
    Arc::new(
        FoyerBackend::new_with_shards(
            dir,
            16 * 1024 * 1024,
            16 * 1024 * 1024,
            1,
            WriteTuning::default(),
            Arc::from(Vec::new()),
        )
        .await
        .expect("create FoyerBackend"),
    )
}

fn make_cache(origin: Arc<dyn ObjectStore>, foyer: Arc<FoyerBackend>) -> RangeCache {
    RangeCache::new(
        origin,
        foyer,
        4096,
        "test".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    )
}

/// End-to-end happy path: given a generous deadline, `run` (1) blocks on a
/// real outstanding fetch task until it's released, (2) drains it, and (3)
/// actually runs `foyer.close()` -- checked two ways, `ram_entry_count()`
/// dropping to 0 and a pre-populated entry surviving a restart, matching the
/// `foyer_backend_tests.rs::demand_fill_survives_close_without_eviction`
/// idiom.
#[tokio::test]
async fn run_drains_outstanding_fetch_and_closes_foyer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path").to_string();

    {
        let foyer = new_foyer(&dir_path).await;

        // Pre-populate a demand entry directly on the backend, independent of
        // the gated fetch below (whose bytes never reach the cache until the
        // origin GET completes): this is what proves stage 3 (`close()`)
        // actually ran, since its default eviction-to-zero flush is what
        // drives `ram_entry_count()` to 0.
        foyer
            .put(
                "pre".to_string(),
                Bytes::from(vec![9u8; 4096]),
                FillHint::Demand,
            )
            .await;
        assert!(
            foyer.ram_entry_count() > 0,
            "sanity: pre-population must land in RAM"
        );

        let store = Arc::new(InMemory::new());
        put_bytes(&store, "obj", &vec![1u8; 4096]).await;
        let (counting, gate) = CountingStore::with_gate(store.clone() as Arc<dyn ObjectStore>);
        let cache = make_cache(counting.clone() as Arc<dyn ObjectStore>, foyer.clone());

        let (_prefetch_tx, prefetch_worker) = spawn_prefetch_worker(cache.clone(), 16, 4);

        let fetch_cache = cache.clone();
        let fetch = tokio::spawn(async move { fetch_cache.get_range("obj", 0..4096).await });
        while counting.get_range_count() < 1 {
            tokio::task::yield_now().await;
        }
        assert!(cache.outstanding_fetch_tasks() >= 1);

        let run_cache = cache.clone();
        let run_foyer = foyer.clone();
        let run = tokio::spawn(async move {
            shutdown_sequence::run(
                Duration::from_secs(5),
                prefetch_worker,
                run_cache,
                run_foyer,
            )
            .await;
        });

        // `run` must not race past the fetch-task drain while the fetch it's
        // supposed to wait for is still gated open.
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert!(
            !run.is_finished(),
            "run must block on the outstanding fetch task, not resolve while it's still in flight"
        );

        gate.add_permits(1);
        tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("run timed out")
            .expect("run task panicked");

        fetch.await.expect("join fetch").expect("get_range");

        assert_eq!(cache.outstanding_fetch_tasks(), 0);
        assert_eq!(
            foyer.ram_entry_count(),
            0,
            "close()'s eviction-to-zero flush must empty the RAM tier"
        );
    } // every handle onto this FoyerBackend is dropped here.

    // Reopen a fresh backend over the same directory: the only way this can
    // read the pre-populated entry back is if `close()` actually flushed it
    // to disk, since a fresh `FoyerBackend` starts with an empty RAM tier.
    let reopened = new_foyer(&dir_path).await;
    let got = reopened
        .get("pre", 4096)
        .await
        .expect("pre-populated entry must survive close() across a restart");
    assert_eq!(got, Bytes::from(vec![9u8; 4096]));
}

/// A short `remaining` must bound `run`'s wall-clock time: it must return
/// once its own deadline elapses rather than hang on a fetch task that never
/// drains, and the stages after the elapsed one must be left unfinished (the
/// fetch task still outstanding, `close()` never run).
#[tokio::test]
async fn run_returns_promptly_when_deadline_elapses_mid_drain() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path").to_string();
    let foyer = new_foyer(&dir_path).await;

    foyer
        .put(
            "pre".to_string(),
            Bytes::from(vec![9u8; 4096]),
            FillHint::Demand,
        )
        .await;
    assert!(foyer.ram_entry_count() > 0, "sanity: pre-population");

    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj", &vec![1u8; 4096]).await;
    let (counting, gate) = CountingStore::with_gate(store.clone() as Arc<dyn ObjectStore>);
    let cache = make_cache(counting.clone() as Arc<dyn ObjectStore>, foyer.clone());
    let (_prefetch_tx, prefetch_worker) = spawn_prefetch_worker(cache.clone(), 16, 4);

    let fetch_cache = cache.clone();
    let fetch = tokio::spawn(async move { fetch_cache.get_range("obj", 0..4096).await });
    while counting.get_range_count() < 1 {
        tokio::task::yield_now().await;
    }
    assert!(cache.outstanding_fetch_tasks() >= 1);

    let before = std::time::Instant::now();
    // The gate is never released, so `wait_for_fetch_tasks_drain()` alone
    // would hang forever; only `run`'s own 20ms deadline can end this.
    tokio::time::timeout(
        Duration::from_secs(5),
        shutdown_sequence::run(
            Duration::from_millis(20),
            prefetch_worker,
            cache.clone(),
            foyer.clone(),
        ),
    )
    .await
    .expect(
        "shutdown_sequence::run must return once its own deadline elapses, \
         not hang on the outer test timeout",
    );
    assert!(
        before.elapsed() < Duration::from_secs(2),
        "run should return close to its own `remaining` budget: took {:?}",
        before.elapsed()
    );

    assert!(
        cache.outstanding_fetch_tasks() >= 1,
        "the drain stage must have been abandoned mid-flight, not silently satisfied"
    );
    assert!(
        foyer.ram_entry_count() > 0,
        "close() must not have run once the deadline elapsed first"
    );

    // Clean up: release the gate so the abandoned fetch task can actually
    // finish rather than leaking for the rest of the process.
    gate.add_permits(1);
    let _ = fetch.await;
}

/// Stage 1 must actually stop the prefetch worker before stage 2 begins:
/// with `worker_concurrency` 1, a second queued item must never be started
/// once `run` has aborted the worker, even after the first item's (gated)
/// fetch is released and drains.
#[tokio::test]
async fn run_aborts_prefetch_worker_before_the_drain_finishes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path").to_string();
    let foyer = new_foyer(&dir_path).await;

    let store = Arc::new(InMemory::new());
    put_bytes(&store, "a", &vec![1u8; 4096]).await;
    put_bytes(&store, "b", &vec![2u8; 4096]).await;
    let (counting, gate) = CountingStore::with_gate(store.clone() as Arc<dyn ObjectStore>);
    let cache = make_cache(counting.clone() as Arc<dyn ObjectStore>, foyer.clone());

    let (prefetch_tx, prefetch_worker) = spawn_prefetch_worker(cache.clone(), 16, 1);
    prefetch_tx
        .send(PrefetchItem {
            key: "a".to_string(),
            size: 4096,
            ranges: None,
        })
        .await
        .expect("send a");
    prefetch_tx
        .send(PrefetchItem {
            key: "b".to_string(),
            size: 4096,
            ranges: None,
        })
        .await
        .expect("send b");

    // Let the worker pick up `a` and block on the gate -- `b` is still
    // sitting unread in the channel behind it (`worker_concurrency` 1).
    while counting.get_range_count() < 1 {
        tokio::task::yield_now().await;
    }
    assert_eq!(cache.outstanding_fetch_tasks(), 1);

    let run_cache = cache.clone();
    let run_foyer = foyer.clone();
    let run = tokio::spawn(async move {
        shutdown_sequence::run(
            Duration::from_secs(5),
            prefetch_worker,
            run_cache,
            run_foyer,
        )
        .await;
    });

    // Give the run task a chance to execute its abort() before releasing
    // `a`'s gate -- if the worker weren't actually stopped here, releasing
    // the gate would let it immediately dequeue and start `b` too.
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    gate.add_permits(1);

    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("run timed out")
        .expect("run task panicked");

    assert_eq!(cache.outstanding_fetch_tasks(), 0);
    assert_eq!(
        counting.get_range_count(),
        1,
        "the aborted prefetch worker must never have started b's fetch"
    );
}
