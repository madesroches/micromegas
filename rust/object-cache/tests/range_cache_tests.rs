use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
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
    DEFAULT_BLOCK_SIZE, DEFAULT_DEMAND_RESERVED_FETCH_PERMITS, DEFAULT_MAX_COALESCED_GET_BYTES,
    DEFAULT_PROMOTE_WHOLE_BATCH, DEFAULT_TOTAL_FETCH_PERMITS, DEMAND_WINDOW_BLOCKS, RangeCache,
    RangeError, StreamRangesCaller,
};

fn make_cache(origin: Arc<dyn ObjectStore>) -> RangeCache {
    let backend = Arc::new(MemoryBackend::new());
    RangeCache::new(
        origin,
        backend,
        DEFAULT_BLOCK_SIZE,
        "test".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    )
}

async fn put_bytes(store: &InMemory, key: &str, data: &[u8]) {
    store
        .put(&Path::from(key), Bytes::copy_from_slice(data).into())
        .await
        .expect("put");
}

/// Wraps an `ObjectStore`, counting `get_range`/`head` calls (both of which
/// desugar to `get_opts` via `ObjectStoreExt`'s default impls) and recording
/// the byte spans requested. When constructed `with_gate`, every `get_opts`
/// call blocks on the returned semaphore until the test releases it with
/// `add_permits`, letting tests deterministically hold origin fetches open to
/// observe concurrency, dedup, and cancellation behavior.
/// Which kind of `get_opts` call a `CountingStore`'s gate blocks.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum GateKind {
    /// Only ranged GETs (`get_range`) block. HEAD requests (which
    /// `RangeCache::size` issues before every block fetch) pass straight
    /// through, so a test gating fetches doesn't also — invisibly —
    /// deadlock on its own unreleased `size()` lookup.
    Range,
    /// Only HEAD requests block.
    Head,
}

#[derive(Debug)]
struct CountingStore {
    inner: Arc<dyn ObjectStore>,
    get_range_calls: AtomicUsize,
    head_calls: AtomicUsize,
    spans: Mutex<Vec<Range<u64>>>,
    gate: Option<(Arc<Semaphore>, GateKind)>,
    in_flight: AtomicUsize,
    peak_in_flight: AtomicUsize,
}

impl CountingStore {
    fn new(inner: Arc<dyn ObjectStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            get_range_calls: AtomicUsize::new(0),
            head_calls: AtomicUsize::new(0),
            spans: Mutex::new(Vec::new()),
            gate: None,
            in_flight: AtomicUsize::new(0),
            peak_in_flight: AtomicUsize::new(0),
        })
    }

    /// Returns the store plus a gate semaphore starting at 0 permits: every
    /// ranged GET blocks until the test calls `gate.add_permits(n)`. HEAD
    /// requests are never gated (see `GateKind::Range`).
    fn with_gate(inner: Arc<dyn ObjectStore>) -> (Arc<Self>, Arc<Semaphore>) {
        Self::with_gate_kind(inner, GateKind::Range)
    }

    /// Like `with_gate`, but blocks HEAD requests instead of ranged GETs.
    fn with_head_gate(inner: Arc<dyn ObjectStore>) -> (Arc<Self>, Arc<Semaphore>) {
        Self::with_gate_kind(inner, GateKind::Head)
    }

    fn with_gate_kind(inner: Arc<dyn ObjectStore>, kind: GateKind) -> (Arc<Self>, Arc<Semaphore>) {
        let gate = Arc::new(Semaphore::new(0));
        let store = Arc::new(Self {
            inner,
            get_range_calls: AtomicUsize::new(0),
            head_calls: AtomicUsize::new(0),
            spans: Mutex::new(Vec::new()),
            gate: Some((gate.clone(), kind)),
            in_flight: AtomicUsize::new(0),
            peak_in_flight: AtomicUsize::new(0),
        });
        (store, gate)
    }

    fn get_range_count(&self) -> usize {
        self.get_range_calls.load(Ordering::SeqCst)
    }

    fn head_count(&self) -> usize {
        self.head_calls.load(Ordering::SeqCst)
    }

    fn spans(&self) -> Vec<Range<u64>> {
        self.spans.lock().expect("lock").clone()
    }

    fn peak_in_flight(&self) -> usize {
        self.peak_in_flight.load(Ordering::SeqCst)
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
        if options.head {
            self.head_calls.fetch_add(1, Ordering::SeqCst);
        } else if let Some(range) = &options.range {
            self.get_range_calls.fetch_add(1, Ordering::SeqCst);
            // `GetOptions::range` isn't `Bounded` in every case the trait
            // supports, but every call this test suite makes goes through
            // `ObjectStoreExt::get_range`, which always sets `Bounded`.
            if let object_store::GetRange::Bounded(r) = range {
                self.spans.lock().expect("lock").push(r.clone());
            }
            let cur = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_in_flight.fetch_max(cur, Ordering::SeqCst);
        }

        if let Some((gate, kind)) = &self.gate {
            let should_gate = match kind {
                GateKind::Range => options.range.is_some(),
                GateKind::Head => options.head,
            };
            if should_gate {
                gate.acquire().await.expect("gate never closed").forget();
            }
        }

        let result = self.inner.get_opts(location, options.clone()).await;
        if options.range.is_some() {
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
        }
        result
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

/// Bounds a test body so a real deadlock in `FetchScheduler` fails fast with
/// a clear message instead of hanging the test run indefinitely.
async fn with_timeout<F: std::future::Future>(fut: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("test timed out -- likely a deadlock in FetchScheduler")
}

#[tokio::test]
async fn get_range_matches_direct() {
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(3 * 1024 * 1024).collect();
    put_bytes(&store, "test/obj", &data).await;

    let cache = make_cache(store.clone() as Arc<dyn ObjectStore>);
    let got = cache
        .get_range("test/obj", 500_000..2_500_000)
        .await
        .expect("get_range");
    assert_eq!(&got[..], &data[500_000..2_500_000]);
}

#[tokio::test]
async fn cold_read_populates_backend() {
    use micromegas_object_cache::backend::RangeCacheBackend;

    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = vec![42u8; 2 * 1024 * 1024];
    put_bytes(&store, "obj", &data).await;

    let backend = Arc::new(MemoryBackend::new());
    let cache = RangeCache::new(
        store.clone() as Arc<dyn ObjectStore>,
        backend.clone(),
        DEFAULT_BLOCK_SIZE,
        "test".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    let got1 = cache.get_range("obj", 0..1024).await.expect("get_range 1");
    let got2 = cache.get_range("obj", 0..1024).await.expect("get_range 2");

    assert_eq!(got1, got2);
    assert_eq!(&got1[..], &data[..1024]);

    let blk_key = "blk:test:obj:0".to_string();
    assert!(
        backend.get(&blk_key).await.is_some(),
        "block should be in backend"
    );
}

#[tokio::test]
async fn warm_read_does_not_refetch_cached_blocks() {
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = vec![7u8; 1024 * 1024];
    put_bytes(&store, "file", &data).await;

    let backend = Arc::new(MemoryBackend::new());
    let cache = RangeCache::new(
        store.clone() as Arc<dyn ObjectStore>,
        backend.clone(),
        DEFAULT_BLOCK_SIZE,
        "ns".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    cache
        .get_range("file", 0..512 * 1024)
        .await
        .expect("first read");
    store
        .delete(&Path::from("file"))
        .await
        .expect("delete origin");
    let got = cache
        .get_range("file", 0..512 * 1024)
        .await
        .expect("second read from cache");
    assert_eq!(got.len(), 512 * 1024);
}

#[tokio::test]
async fn get_ranges_returns_correct_bytes() {
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..255).cycle().take(4 * 1024 * 1024).collect();
    put_bytes(&store, "multi", &data).await;

    let cache = make_cache(store.clone() as Arc<dyn ObjectStore>);

    let ranges = vec![
        0u64..512_000u64,
        1_500_000u64..2_000_000u64,
        3_900_000u64..4_194_304u64,
    ];
    let results = cache
        .get_ranges("multi", &ranges)
        .await
        .expect("get_ranges");

    assert_eq!(results.len(), 3);
    assert_eq!(&results[0][..], &data[0..512_000]);
    assert_eq!(&results[1][..], &data[1_500_000..2_000_000]);
    assert_eq!(&results[2][..], &data[3_900_000..4_194_304]);
}

#[tokio::test]
async fn size_returns_file_size() {
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "sized_file", &[0u8; 12345]).await;

    let cache = make_cache(store.clone() as Arc<dyn ObjectStore>);
    let size = cache.size("sized_file").await.expect("size");
    assert_eq!(size, 12345);
}

// -- Phase 1: single-flight -------------------------------------------------

#[tokio::test]
async fn concurrent_identical_block_reads_dedup_to_one_get() {
    with_timeout(async move {
        let store = Arc::new(InMemory::new());
        let data: Vec<u8> = vec![5u8; DEFAULT_BLOCK_SIZE as usize];
        put_bytes(&store, "dedup", &data).await;

        let (counting, gate) = CountingStore::with_gate(store.clone() as Arc<dyn ObjectStore>);
        let cache = make_cache(counting.clone() as Arc<dyn ObjectStore>);

        const N: usize = 8;
        let entered = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..N {
            let cache = cache.clone();
            let entered = entered.clone();
            handles.push(tokio::spawn(async move {
                entered.fetch_add(1, Ordering::SeqCst);
                cache.get_range("dedup", 0..1024).await
            }));
        }

        while entered.load(Ordering::SeqCst) < N || counting.get_range_count() == 0 {
            tokio::task::yield_now().await;
        }
        gate.add_permits(1);

        for h in handles {
            let got = h.await.expect("task join").expect("get_range");
            assert_eq!(&got[..], &data[0..1024]);
        }
        assert_eq!(
            counting.get_range_count(),
            1,
            "single-flight should dedup to one origin GET"
        );
    })
    .await;
}

#[tokio::test]
async fn concurrent_size_misses_dedup_to_one_head() {
    with_timeout(async move {
        let store = Arc::new(InMemory::new());
        put_bytes(&store, "sized", &[0u8; 4096]).await;
        let (counting, gate) = CountingStore::with_head_gate(store.clone() as Arc<dyn ObjectStore>);
        let cache = make_cache(counting.clone() as Arc<dyn ObjectStore>);

        const N: usize = 8;
        let entered = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..N {
            let cache = cache.clone();
            let entered = entered.clone();
            handles.push(tokio::spawn(async move {
                entered.fetch_add(1, Ordering::SeqCst);
                cache.size("sized").await
            }));
        }

        while entered.load(Ordering::SeqCst) < N || counting.head_count() == 0 {
            tokio::task::yield_now().await;
        }
        gate.add_permits(1);

        for h in handles {
            let size = h.await.expect("task join").expect("size");
            assert_eq!(size, 4096);
        }
        assert_eq!(
            counting.head_count(),
            1,
            "single-flight should dedup to one head"
        );
    })
    .await;
}

#[tokio::test]
async fn owner_cancelled_mid_fetch_joiner_still_completes() {
    with_timeout(async move {
        let store = Arc::new(InMemory::new());
        let data: Vec<u8> = vec![3u8; DEFAULT_BLOCK_SIZE as usize];
        put_bytes(&store, "cancel", &data).await;
        let (counting, gate) = CountingStore::with_gate(store.clone() as Arc<dyn ObjectStore>);
        let cache = make_cache(counting.clone() as Arc<dyn ObjectStore>);

        let owner_cache = cache.clone();
        let owner = tokio::spawn(async move { owner_cache.get_range("cancel", 0..1024).await });

        while counting.get_range_count() == 0 {
            tokio::task::yield_now().await;
        }
        // Simulate a client disconnect: drop the owning request future. The
        // actual origin fetch runs in a detached task, so this must not
        // strand the joiner below.
        owner.abort();

        let joiner_cache = cache.clone();
        let joiner = tokio::spawn(async move { joiner_cache.get_range("cancel", 0..1024).await });
        tokio::task::yield_now().await;

        gate.add_permits(1);

        let got = joiner.await.expect("joiner task join").expect("get_range");
        assert_eq!(&got[..], &data[0..1024]);
        assert_eq!(
            counting.get_range_count(),
            1,
            "only the original owner's run should ever hit origin"
        );
    })
    .await;
}

// -- Phase 2: run coalescing -------------------------------------------------

#[tokio::test]
async fn cold_contiguous_read_coalesces_to_few_gets() {
    let block_size = 1024u64;
    let max_coalesced = 3 * block_size;
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(10 * block_size as usize).collect();
    put_bytes(&store, "big", &data).await;

    let counting = CountingStore::new(store.clone() as Arc<dyn ObjectStore>);
    let cache = RangeCache::new(
        counting.clone() as Arc<dyn ObjectStore>,
        Arc::new(MemoryBackend::new()),
        block_size,
        "ns".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        max_coalesced,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    // 5 contiguous blocks (0..5) at 3 blocks/run max -> runs 0..3, 3..5.
    let got = cache
        .get_range("big", 0..5 * block_size)
        .await
        .expect("get_range");
    assert_eq!(&got[..], &data[0..5 * block_size as usize]);
    assert_eq!(
        counting.get_range_count(),
        2,
        "5 contiguous blocks at 3/run should coalesce to 2 GETs"
    );
    let mut spans = counting.spans();
    spans.sort_by_key(|r| r.start);
    assert_eq!(
        spans,
        vec![0..3 * block_size, 3 * block_size..5 * block_size]
    );
}

#[tokio::test]
async fn partially_cached_read_never_refetches_cached_blocks() {
    let block_size = 1024u64;
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(6 * block_size as usize).collect();
    put_bytes(&store, "warm", &data).await;

    let counting = CountingStore::new(store.clone() as Arc<dyn ObjectStore>);
    let cache = RangeCache::new(
        counting.clone() as Arc<dyn ObjectStore>,
        Arc::new(MemoryBackend::new()),
        block_size,
        "ns".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    // Warm blocks 0..2.
    cache
        .get_range("warm", 0..2 * block_size)
        .await
        .expect("warm");
    let warm_gets = counting.get_range_count();

    // blocks 0..2 are now cached; 2..6 are missing.
    let got = cache
        .get_range("warm", 0..6 * block_size)
        .await
        .expect("second read");
    assert_eq!(&got[..], &data[0..6 * block_size as usize]);

    let spans = counting.spans();
    for span in &spans[warm_gets..] {
        assert!(
            span.start >= 2 * block_size,
            "must not refetch cached blocks 0..2, got span {span:?}"
        );
    }
}

#[tokio::test]
async fn scattered_read_stays_per_block() {
    let block_size = 1024u64;
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(20 * block_size as usize).collect();
    put_bytes(&store, "scattered", &data).await;

    let counting = CountingStore::new(store.clone() as Arc<dyn ObjectStore>);
    let cache = RangeCache::new(
        counting.clone() as Arc<dyn ObjectStore>,
        Arc::new(MemoryBackend::new()),
        block_size,
        "ns".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    let ranges = vec![
        0..10u64,
        4 * block_size..4 * block_size + 10,
        9 * block_size..9 * block_size + 10,
        15 * block_size..15 * block_size + 10,
    ];
    let results = cache
        .get_ranges("scattered", &ranges)
        .await
        .expect("get_ranges");
    assert_eq!(results.len(), 4);
    assert_eq!(
        counting.get_range_count(),
        4,
        "scattered blocks must not coalesce"
    );
    for span in counting.spans() {
        assert_eq!(
            span.end - span.start,
            block_size,
            "each scattered fetch should be exactly one block"
        );
    }
}

// -- Phase 3: priority budget -------------------------------------------------

#[tokio::test]
async fn demand_not_starved_under_prefetch_saturation() {
    with_timeout(async move {
        let block_size = 1024u64;
        let file_size = 20 * block_size;
        let store = Arc::new(InMemory::new());
        put_bytes(&store, "obj", &vec![1u8; file_size as usize]).await;
        let (counting, gate) = CountingStore::with_gate(store.clone() as Arc<dyn ObjectStore>);

        let total = 4usize;
        let demand_reserved = 1usize;
        let cache = RangeCache::new(
            counting.clone() as Arc<dyn ObjectStore>,
            Arc::new(MemoryBackend::new()),
            block_size,
            "ns".to_string(),
            total,
            demand_reserved,
            DEFAULT_MAX_COALESCED_GET_BYTES,
            DEFAULT_PROMOTE_WHOLE_BATCH,
        );

        // 5 scattered (non-adjacent) block indices -> 5 separate prefetch runs.
        let scattered = [0u64, 2, 4, 6, 8];
        let mut prefetch_handles = Vec::new();
        for &idx in &scattered {
            let cache = cache.clone();
            prefetch_handles.push(tokio::spawn(async move {
                cache.prefetch_blocks("obj", file_size, &[idx]).await
            }));
        }

        // prefetch_permits = total - demand_reserved = 3: only 3 of the 5
        // scattered prefetch runs can be in flight; the rest queue behind them.
        while counting.get_range_count() < 3 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            counting.get_range_count(),
            3,
            "prefetch must not exceed its budget"
        );

        let demand_cache = cache.clone();
        let demand_range = 10 * block_size..10 * block_size + 10;
        let demand = tokio::spawn(async move { demand_cache.get_range("obj", demand_range).await });

        while counting.get_range_count() < 4 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            counting.get_range_count(),
            4,
            "demand must find a reserved permit despite prefetch saturation"
        );
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            counting.get_range_count(),
            4,
            "queued prefetch must not sneak into the permit reserved for demand"
        );

        gate.add_permits(10);
        let got = demand.await.expect("join").expect("get_range");
        assert_eq!(got.len(), 10);
        for h in prefetch_handles {
            h.await.expect("join").expect("prefetch_blocks");
        }
    })
    .await;
}

#[tokio::test]
async fn promotion_lets_demand_start_before_remaining_prefetch() {
    with_timeout(async move {
        let block_size = 1024u64;
        let file_size = 20 * block_size;
        let store = Arc::new(InMemory::new());
        put_bytes(&store, "obj", &vec![2u8; file_size as usize]).await;
        let (counting, gate) = CountingStore::with_gate(store.clone() as Arc<dyn ObjectStore>);

        // total=2, demand_reserved=1 => prefetch_permits=1: only one of the
        // three scattered prefetch blocks (a, b, c) can run at a time.
        let cache = RangeCache::new(
            counting.clone() as Arc<dyn ObjectStore>,
            Arc::new(MemoryBackend::new()),
            block_size,
            "ns".to_string(),
            2,
            1,
            DEFAULT_MAX_COALESCED_GET_BYTES,
            DEFAULT_PROMOTE_WHOLE_BATCH,
        );

        let (a, b, c) = (0u64, 5u64, 10u64);
        let prefetch = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.prefetch_blocks("obj", file_size, &[a, b, c]).await })
        };

        // One prefetch run (a) is now in flight; b and c are queued behind
        // the single prefetch permit.
        while counting.get_range_count() < 1 {
            tokio::task::yield_now().await;
        }
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(counting.get_range_count(), 1);

        // A demand read for block `b` promotes its queued prefetch entry: it
        // no longer needs a prefetch permit, only a (still-free) shared
        // permit, so it starts even though `c` remains queued.
        let demand_cache = cache.clone();
        let demand = tokio::spawn(async move {
            demand_cache
                .get_range("obj", b * block_size..b * block_size + 10)
                .await
        });

        while counting.get_range_count() < 2 {
            tokio::task::yield_now().await;
        }
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            counting.get_range_count(),
            2,
            "promoted block b should start before queued block c"
        );

        gate.add_permits(3);
        let got = demand.await.expect("join").expect("get_range");
        assert_eq!(got.len(), 10);
        prefetch.await.expect("join").expect("prefetch_blocks");

        assert_eq!(counting.get_range_count(), 3);
        let spans = counting.spans();
        let block_c_start = c * block_size;
        assert!(
            spans[..2].iter().all(|s| s.start != block_c_start),
            "block c must not have started before b's promotion: {spans:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn prefetch_blocks_with_empty_indices_is_a_no_op() {
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj", &[0u8; 4096]).await;
    let counting = CountingStore::new(store.clone() as Arc<dyn ObjectStore>);
    let cache = RangeCache::new(
        counting.clone() as Arc<dyn ObjectStore>,
        Arc::new(MemoryBackend::new()),
        DEFAULT_BLOCK_SIZE,
        "ns".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );
    cache
        .prefetch_blocks("obj", 0, &[])
        .await
        .expect("empty index set is a no-op");
    assert_eq!(counting.get_range_count(), 0);
    assert_eq!(counting.head_count(), 0);
}

// -- Size-trust guard --------------------------------------------------------

#[tokio::test]
async fn undersized_prefetch_size_is_healed_on_demand_read() {
    let block_size = 1024u64;
    let true_size = 2 * block_size; // 2048
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(true_size as usize).collect();
    put_bytes(&store, "obj", &data).await;

    let counting = CountingStore::new(store.clone() as Arc<dyn ObjectStore>);
    let cache = RangeCache::new(
        counting.clone() as Arc<dyn ObjectStore>,
        Arc::new(MemoryBackend::new()),
        block_size,
        "ns".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    // Prefetch both blocks under an undersized caller-supplied size (1500):
    // block 1's byte range clamps to 1024..1500 (476 bytes) instead of the
    // true 1024..2048 (1024 bytes), storing a truncated final block.
    let undersized_size = 1500u64;
    cache
        .prefetch_blocks("obj", undersized_size, &[0, 1])
        .await
        .expect("prefetch with undersized size should still succeed (in-bounds GET)");
    assert_eq!(
        counting.get_range_count(),
        1,
        "prefetch should have issued exactly one coalesced GET"
    );

    // A demand read at the true size must detect the short block, refetch
    // it, and return the full correct bytes.
    let got = cache
        .get_range("obj", 1200..1600)
        .await
        .expect("demand get_range at the true size");
    assert_eq!(&got[..], &data[1200..1600]);
    assert_eq!(
        counting.get_range_count(),
        2,
        "the mismatched block must be refetched from origin"
    );
}

#[tokio::test]
async fn oversized_prefetch_size_fails_fill_without_storing() {
    use micromegas_object_cache::backend::RangeCacheBackend;

    let block_size = 1024u64;
    let true_size = 2 * block_size; // 2048
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(true_size as usize).collect();
    put_bytes(&store, "obj2", &data).await;

    let backend = Arc::new(MemoryBackend::new());
    let counting = CountingStore::new(store.clone() as Arc<dyn ObjectStore>);
    let cache = RangeCache::new(
        counting.clone() as Arc<dyn ObjectStore>,
        backend.clone(),
        block_size,
        "ns".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    // Prefetch block index 2 (bytes 2048..3000) under an oversized caller
    // size of 3000: the object is really only 2048 bytes, so the origin GET
    // past EOF must fail and nothing gets stored.
    let oversized_size = 3000u64;
    cache
        .prefetch_blocks("obj2", oversized_size, &[2])
        .await
        .expect_err("origin GET past EOF must fail the fill");
    assert!(
        backend.get("blk:ns:obj2:2").await.is_none(),
        "a failed prefetch fill must not store anything"
    );

    // A subsequent demand read at the true size is unaffected.
    let got = cache
        .get_range("obj2", 0..true_size)
        .await
        .expect("demand get_range at the true size");
    assert_eq!(&got[..], &data[..]);
}

// A corrupt/misdecoded 8-byte cached size (e.g. from an old-format disk entry
// misframed by a new decoder, #1287) must degrade to a miss and re-resolve
// from origin instead of being trusted, since a garbage size can otherwise
// drive a catastrophic allocation downstream.
#[tokio::test]
async fn implausible_cached_size_degrades_to_a_miss() {
    use micromegas_object_cache::backend::{FillHint, RangeCacheBackend};
    use micromegas_object_cache::range_cache::MAX_PLAUSIBLE_OBJECT_SIZE;

    let block_size = 1024u64;
    let true_size = 4096u64;
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(true_size as usize).collect();
    put_bytes(&store, "obj", &data).await;

    let backend = Arc::new(MemoryBackend::new());
    let counting = CountingStore::new(store.clone() as Arc<dyn ObjectStore>);
    let ns = "ns".to_string();
    let cache = RangeCache::new(
        counting.clone() as Arc<dyn ObjectStore>,
        backend.clone(),
        block_size,
        ns.clone(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    // Poison the meta entry with an implausible size (well above the
    // ceiling), simulating a misdecoded/corrupt cached value.
    let garbage_size = MAX_PLAUSIBLE_OBJECT_SIZE + 1;
    backend
        .put(
            format!("meta:{ns}:obj"),
            Bytes::from(garbage_size.to_le_bytes().to_vec()),
            FillHint::Demand,
        )
        .await;

    let size = cache.size("obj").await.expect("size must not panic/error");
    assert_eq!(
        size, true_size,
        "an implausible cached size must be rejected and re-resolved from origin"
    );
    assert_eq!(
        counting.head_count(),
        1,
        "rejecting the poisoned meta entry must issue an origin HEAD"
    );
}

#[tokio::test]
async fn total_concurrency_never_exceeds_total() {
    with_timeout(async move {
        let block_size = 1024u64;
        let file_size = 20 * block_size;
        let store = Arc::new(InMemory::new());
        put_bytes(&store, "obj", &vec![9u8; file_size as usize]).await;
        let (counting, gate) = CountingStore::with_gate(store.clone() as Arc<dyn ObjectStore>);

        let total = 3usize;
        let cache = RangeCache::new(
            counting.clone() as Arc<dyn ObjectStore>,
            Arc::new(MemoryBackend::new()),
            block_size,
            "ns".to_string(),
            total,
            1,
            DEFAULT_MAX_COALESCED_GET_BYTES,
            DEFAULT_PROMOTE_WHOLE_BATCH,
        );

        let scattered: Vec<u64> = (0..10).map(|i| i * 2).collect();
        let mut handles = Vec::new();
        for &idx in &scattered {
            let cache = cache.clone();
            handles.push(tokio::spawn(async move {
                cache
                    .get_range("obj", idx * block_size..idx * block_size + 10)
                    .await
            }));
        }

        while counting.get_range_count() < total {
            tokio::task::yield_now().await;
        }
        for _ in 0..30 {
            tokio::task::yield_now().await;
        }
        assert!(
            counting.peak_in_flight() <= total,
            "peak in-flight origin GETs {} exceeded total {total}",
            counting.peak_in_flight()
        );

        gate.add_permits(scattered.len());
        for h in handles {
            let got = h.await.expect("join").expect("get_range");
            assert_eq!(got.len(), 10);
        }
        assert!(counting.peak_in_flight() <= total);
    })
    .await;
}

// -- stream_ranges ------------------------------------------------------

/// A ranged `get_opts` call fails once its requested start offset reaches
/// `fail_from`; earlier ranges succeed. Lets a test make an early fetch
/// window's origin call succeed (so the stream commits its first bytes)
/// while a later window's call fails with a genuine mid-stream error.
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
            return Err(object_store::Error::Generic {
                store: "FailAtOffsetStore",
                source: Box::new(std::io::Error::other("origin down mid-stream")),
            });
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

/// Drain a `stream_ranges` stream into one flat `Bytes`, for tests covering a
/// single range where the exact chunk boundaries don't matter.
async fn collect_stream(
    stream: impl futures::Stream<Item = anyhow::Result<Bytes>> + Send + 'static,
) -> anyhow::Result<Bytes> {
    let mut stream = Box::pin(stream);
    let mut buf = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
    }
    Ok(buf.freeze())
}

#[tokio::test]
async fn stream_ranges_multi_window_matches_direct() {
    let block_size = 1024u64;
    let store = Arc::new(InMemory::new());
    // Spans 4 fetch windows (`DEMAND_WINDOW_BLOCKS` blocks each).
    let total = DEMAND_WINDOW_BLOCKS * block_size * 4;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    put_bytes(&store, "obj", &data).await;

    let cache = RangeCache::new(
        store.clone() as Arc<dyn ObjectStore>,
        Arc::new(MemoryBackend::new()),
        block_size,
        "ns".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    #[allow(clippy::single_range_in_vec_init)]
    let stream = cache
        .stream_ranges("obj", vec![0..total], StreamRangesCaller::Range)
        .await
        .expect("stream_ranges");
    let got = collect_stream(stream).await.expect("collect");
    assert_eq!(&got[..], &data[..]);
}

#[tokio::test]
async fn stream_ranges_non_block_aligned_boundary() {
    let block_size = 1024u64;
    let store = Arc::new(InMemory::new());
    let total = 10 * block_size;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    put_bytes(&store, "obj", &data).await;

    let cache = RangeCache::new(
        store.clone() as Arc<dyn ObjectStore>,
        Arc::new(MemoryBackend::new()),
        block_size,
        "ns".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    let range = 500u64..(9 * block_size + 37);
    let stream = cache
        .stream_ranges("obj", vec![range.clone()], StreamRangesCaller::Range)
        .await
        .expect("stream_ranges");
    let got = collect_stream(stream).await.expect("collect");
    assert_eq!(&got[..], &data[range.start as usize..range.end as usize]);
}

#[tokio::test]
async fn stream_ranges_multiple_ranges_sharing_blocks() {
    let block_size = 1024u64;
    let store = Arc::new(InMemory::new());
    let total = 5 * block_size;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    put_bytes(&store, "obj", &data).await;

    let cache = RangeCache::new(
        store.clone() as Arc<dyn ObjectStore>,
        Arc::new(MemoryBackend::new()),
        block_size,
        "ns".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    // The second and third ranges both touch block 0; the fourth is
    // degenerate and must contribute nothing to the flat chunk sequence.
    let ranges = vec![
        100u64..300,
        200u64..900,
        3 * block_size..3 * block_size + 50,
        50u64..50,
    ];
    let mut stream = Box::pin(
        cache
            .stream_ranges("obj", ranges.clone(), StreamRangesCaller::Ranges)
            .await
            .expect("stream_ranges"),
    );

    // Reconstruct each range's bytes from the flat, ordered chunk sequence
    // by known length, the same way the `get_ranges` collector does.
    let mut pending: Option<Bytes> = None;
    let mut results: Vec<Bytes> = Vec::new();
    for r in &ranges {
        if r.start >= r.end {
            results.push(Bytes::new());
            continue;
        }
        let need = (r.end - r.start) as usize;
        let mut collected = BytesMut::with_capacity(need);
        while collected.len() < need {
            let chunk = match pending.take() {
                Some(c) => c,
                None => stream
                    .next()
                    .await
                    .expect("stream under-yielded")
                    .expect("chunk ok"),
            };
            let remaining = need - collected.len();
            if chunk.len() > remaining {
                collected.extend_from_slice(&chunk[..remaining]);
                pending = Some(chunk.slice(remaining..));
            } else {
                collected.extend_from_slice(&chunk);
            }
        }
        results.push(collected.freeze());
    }

    for (r, got) in ranges.iter().zip(results.iter()) {
        assert_eq!(&got[..], &data[r.start as usize..r.end as usize]);
    }
}

#[tokio::test]
async fn stream_ranges_missing_key_errors_upfront() {
    let store = Arc::new(InMemory::new());
    let cache = make_cache(store as Arc<dyn ObjectStore>);

    #[allow(clippy::single_range_in_vec_init)]
    let result = cache
        .stream_ranges("missing", vec![0..10], StreamRangesCaller::Range)
        .await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("missing key must error before any stream is returned"),
    };
    let os_err = err
        .downcast_ref::<object_store::Error>()
        .expect("NotFound should survive the downcast");
    assert!(matches!(os_err, object_store::Error::NotFound { .. }));
}

#[tokio::test]
async fn stream_ranges_out_of_bounds_errors_upfront() {
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj", &[0u8; 100]).await;
    let cache = make_cache(store as Arc<dyn ObjectStore>);

    #[allow(clippy::single_range_in_vec_init)]
    let result = cache
        .stream_ranges("obj", vec![0..200], StreamRangesCaller::Ranges)
        .await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("out-of-bounds range must error before any stream is returned"),
    };
    assert!(err.downcast_ref::<RangeError>().is_some());
}

#[tokio::test]
async fn stream_ranges_mid_stream_origin_failure_surfaces_as_stream_err() {
    let block_size = 1024u64;
    let store = Arc::new(InMemory::new());
    let total = DEMAND_WINDOW_BLOCKS * block_size * 3; // spans 3 fetch windows
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    put_bytes(&store, "obj", &data).await;

    // The first fetch window succeeds; the second (and any later) fails.
    let flaky = Arc::new(FailAtOffsetStore {
        inner: store.clone() as Arc<dyn ObjectStore>,
        fail_from: DEMAND_WINDOW_BLOCKS * block_size,
    });
    let cache = RangeCache::new(
        flaky as Arc<dyn ObjectStore>,
        Arc::new(MemoryBackend::new()),
        block_size,
        "ns".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    #[allow(clippy::single_range_in_vec_init)]
    let mut stream = Box::pin(
        cache
            .stream_ranges("obj", vec![0..total], StreamRangesCaller::Range)
            .await
            .expect("stream_ranges upfront validation passes"),
    );

    // The first window's bytes are yielded successfully...
    let first = stream
        .next()
        .await
        .expect("first chunk present")
        .expect("first window succeeds");
    assert_eq!(
        &first[..],
        &data[..DEMAND_WINDOW_BLOCKS as usize * block_size as usize]
    );

    // ...then the second window's origin failure surfaces as the stream's
    // terminal `Err` item, not a panic or a silently truncated success.
    let second = stream.next().await.expect("stream yields the failure");
    assert!(second.is_err(), "mid-stream origin failure must be an Err");
}
