use std::sync::Arc;

use bytes::Bytes;
use micromegas_object_cache::backend::RangeCacheBackend;
use micromegas_object_cache::memory_backend::MemoryBackend;
use micromegas_object_cache::range_cache::{
    DEFAULT_BLOCK_SIZE, DEFAULT_DEMAND_RESERVED_FETCH_PERMITS, DEFAULT_MAX_COALESCED_GET_BYTES,
    DEFAULT_PROMOTE_WHOLE_BATCH, DEFAULT_TOTAL_FETCH_PERMITS, RangeCache,
};
use micromegas_tracing::metrics::MetricsMsgQueueAny;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::HeterogeneousQueue;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};
use serial_test::serial;

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

/// Count how many times a tagged or untagged integer metric named `name`
/// fired since the guard was created. Requires `dispatch::flush_metrics_buffer`
/// to have been called first so buffered events are visible as blocks.
fn count_integer_metric(
    sink: &micromegas_tracing::event::in_memory_sink::InMemorySink,
    name: &str,
) -> u64 {
    let state = sink.state.lock().expect("sink lock");
    let mut count = 0u64;
    for block in &state.metrics_blocks {
        for evt in block.events.iter() {
            match evt {
                MetricsMsgQueueAny::IntegerMetricEvent(e) if e.desc.name == name => count += 1,
                MetricsMsgQueueAny::TaggedIntegerMetricEvent(e) if e.desc.name == name => {
                    count += 1
                }
                _ => {}
            }
        }
    }
    count
}

/// Regression guard for the double-size-resolution bug (#1206 audit
/// comment): `get_range_handler` resolves `size()` once for range
/// validation, then (pre-fix) called plain `stream_ranges`, which resolved
/// size *again* internally -- double-counting `range_cache_size_backend_hit`
/// on every warm ranged GET. This mirrors the handler's pattern directly
/// against `RangeCache`: one `size()` call (the handler's own lookup) plus
/// `get_range_with_size` (which must do zero additional lookups) should
/// total exactly one hit, not two.
#[test]
#[serial]
fn get_range_with_size_resolves_size_once() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let store = Arc::new(InMemory::new());
        let data = vec![9u8; 4096];
        put_bytes(&store, "obj", &data).await;
        let cache = make_cache(store.clone() as Arc<dyn ObjectStore>);

        // Warm the size cache before tracing starts: a cold lookup would hit
        // `range_cache_origin_head`, not `range_cache_size_backend_hit`.
        cache.size("obj").await.expect("warm size");

        let guard = init_in_memory_tracing();

        // Mirrors `get_range_handler`: resolve size once up front (for range
        // validation), then read with that size already in hand.
        let file_size = cache
            .size("obj")
            .await
            .expect("handler-level size resolution");
        let got = cache
            .get_range_with_size("obj", file_size, 0..1024)
            .await
            .expect("get_range_with_size");
        assert_eq!(&got[..], &data[0..1024]);
        micromegas_tracing::dispatch::flush_metrics_buffer();

        assert_eq!(
            count_integer_metric(&guard.sink, "range_cache_size_backend_hit"),
            1,
            "the handler's own size() call should be the only resolution -- \
             get_range_with_size must not re-resolve size internally"
        );
    });
}

/// `get_range_with_size` and `get_range` must return identical bytes for
/// both a cold key (fills the cache) and a warm key (serves from it).
#[test]
fn with_size_variants_match_no_size_variants() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let store = Arc::new(InMemory::new());
        let data: Vec<u8> = (0u8..=255).cycle().take(3 * 1024 * 1024).collect();
        put_bytes(&store, "obj", &data).await;
        let cache = make_cache(store.clone() as Arc<dyn ObjectStore>);
        let file_size = data.len() as u64;

        // Cold: via the `_with_size` variant.
        let cold_with_size = cache
            .get_range_with_size("obj", file_size, 500_000..1_500_000)
            .await
            .expect("cold get_range_with_size");
        assert_eq!(&cold_with_size[..], &data[500_000..1_500_000]);

        // Warm: plain `get_range` on the same key/range must match.
        let warm_plain = cache
            .get_range("obj", 500_000..1_500_000)
            .await
            .expect("warm get_range");
        assert_eq!(cold_with_size, warm_plain);

        // `get_ranges_with_size` vs `get_ranges` on a fresh key.
        put_bytes(&store, "obj2", &data).await;
        let ranges = vec![0u64..1000, 2_000_000u64..2_500_000];
        let via_with_size = cache
            .get_ranges_with_size("obj2", file_size, &ranges)
            .await
            .expect("get_ranges_with_size");
        let via_plain = cache.get_ranges("obj2", &ranges).await.expect("get_ranges");
        assert_eq!(via_with_size, via_plain);
        assert_eq!(&via_with_size[0][..], &data[0..1000]);
        assert_eq!(&via_with_size[1][..], &data[2_000_000..2_500_000]);
    });
}

/// `fetch_budget_stats`/`inflight_len` reflect acquired permits and
/// in-flight entries under a controlled fetch, gated on an origin store that
/// blocks until the test releases it.
#[tokio::test]
async fn fetch_budget_stats_reflect_in_flight_fetch() {
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use object_store::{
        CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta,
        PutMultipartOptions, PutOptions, PutPayload, PutResult,
    };
    use tokio::sync::Semaphore;

    #[derive(Debug)]
    struct GatedStore {
        inner: Arc<dyn ObjectStore>,
        gate: Arc<Semaphore>,
    }
    impl std::fmt::Display for GatedStore {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "GatedStore({})", self.inner)
        }
    }
    #[async_trait]
    impl ObjectStore for GatedStore {
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
        fn list(
            &self,
            prefix: Option<&Path>,
        ) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
            self.inner.list(prefix)
        }
        async fn list_with_delimiter(
            &self,
            prefix: Option<&Path>,
        ) -> object_store::Result<ListResult> {
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

    let store = Arc::new(InMemory::new());
    put_bytes(&store, "obj", &vec![1u8; 4096]).await;
    let gate = Arc::new(Semaphore::new(0));
    let gated = Arc::new(GatedStore {
        inner: store.clone() as Arc<dyn ObjectStore>,
        gate: gate.clone(),
    });
    let cache = RangeCache::new(
        gated as Arc<dyn ObjectStore>,
        Arc::new(MemoryBackend::new()),
        DEFAULT_BLOCK_SIZE,
        "ns".to_string(),
        4,
        1,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    let (shared_avail0, shared_total, prefetch_avail0, prefetch_total) = cache.fetch_budget_stats();
    assert_eq!(shared_total, 4);
    assert_eq!(prefetch_total, 3);
    assert_eq!(shared_avail0, 4);
    assert_eq!(prefetch_avail0, 3);
    assert_eq!(cache.inflight_len(), 0);

    let fetch_cache = cache.clone();
    let handle = tokio::spawn(async move { fetch_cache.get_range("obj", 0..10).await });

    while cache.inflight_len() == 0 {
        tokio::task::yield_now().await;
    }
    let (shared_avail1, _, _, _) = cache.fetch_budget_stats();
    assert_eq!(
        shared_avail1,
        shared_total - 1,
        "an in-flight demand fetch must hold one shared permit"
    );
    assert_eq!(cache.inflight_len(), 1);

    gate.add_permits(1);
    let got = handle.await.expect("join").expect("get_range");
    assert_eq!(got.len(), 10);

    assert_eq!(cache.inflight_len(), 0);
    let (shared_avail2, _, prefetch_avail2, _) = cache.fetch_budget_stats();
    assert_eq!(shared_avail2, shared_total);
    assert_eq!(prefetch_avail2, prefetch_total);
}

/// `MemoryBackend` has no disk tier, so `disk_stats()` must stay `None` --
/// this is the branch the saturation monitor's foyer disk gauges skip
/// entirely for a non-foyer backend.
#[tokio::test]
async fn memory_backend_disk_stats_is_none() {
    let backend = MemoryBackend::new();
    assert!(RangeCacheBackend::disk_stats(&backend).is_none());

    let store = Arc::new(InMemory::new());
    let cache = make_cache(store as Arc<dyn ObjectStore>);
    assert!(
        cache.backend_disk_stats().is_none(),
        "RangeCache::backend_disk_stats must delegate to the backend's None default"
    );
}
