use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{BoxStream, TryStreamExt};
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetRange, GetResult, ListResult, MultipartUpload, ObjectMeta,
    ObjectStore, ObjectStoreExt, PutMultipartOptions, PutOptions, PutPayload, PutResult,
};

use micromegas_object_cache::backend::{FillHint, RangeCacheBackend};
use micromegas_object_cache::bounded_memory_backend::BoundedMemoryBackend;
use micromegas_object_cache::l1_store::L1CacheStore;

// ============================================================================
// BoundedMemoryBackend
// ============================================================================

#[tokio::test]
async fn bounded_memory_backend_get_put_round_trip() {
    let backend = BoundedMemoryBackend::new(1024 * 1024);
    assert!(backend.get("missing", 0).await.is_none());

    let data = Bytes::from_static(b"hello world");
    backend
        .put("key".to_string(), data.clone(), FillHint::Demand)
        .await;
    assert_eq!(backend.get("key", data.len() as u64).await, Some(data));
}

#[tokio::test]
async fn bounded_memory_backend_ignores_fill_hint() {
    let backend = BoundedMemoryBackend::new(1024 * 1024);
    let data = Bytes::from_static(b"prefetched data");
    backend
        .put("key".to_string(), data.clone(), FillHint::Prefetch)
        .await;
    // No disk tier: a prefetch fill lands in the same in-memory cache as a
    // demand fill (unlike `FoyerBackend`, which routes it SSD-only).
    assert_eq!(backend.get("key", data.len() as u64).await, Some(data));
}

// A `put` must detach (copy) the stored block from its parent buffer, or the
// LFU eviction structure keeps the whole coalesced-GET parent allocation
// alive even though the weigher only charges the slice length -- the same bug
// as `FoyerBackend`'s demand path (see #1276).
#[tokio::test]
async fn bounded_memory_backend_detaches_from_parent_buffer() {
    let backend = BoundedMemoryBackend::new(1024 * 1024);
    let parent = Bytes::from(vec![7u8; 8192]);
    let block = parent.slice(0..4096);
    let block_ptr = block.as_ptr();
    backend
        .put("k".to_string(), block.clone(), FillHint::Demand)
        .await;

    let got = backend.get("k", 4096).await.expect("hit");
    assert_eq!(got, vec![7u8; 4096]);
    assert_ne!(
        got.as_ptr(),
        block_ptr,
        "put must copy, detaching the cached block from its parent buffer"
    );
}

#[tokio::test]
async fn bounded_memory_backend_evicts_at_budget() {
    let budget = 800usize;
    let backend = BoundedMemoryBackend::new(budget);
    let entry = Bytes::from(vec![0u8; 100]);
    for i in 0..40 {
        backend
            .put(format!("key-{i}"), entry.clone(), FillHint::Demand)
            .await;
    }

    assert!(
        backend.usage() <= budget,
        "usage {} exceeds budget {budget}",
        backend.usage()
    );

    let mut hits = 0;
    for i in 0..40 {
        if backend
            .get(&format!("key-{i}"), entry.len() as u64)
            .await
            .is_some()
        {
            hits += 1;
        }
    }
    assert!(
        hits < 40,
        "expected eviction to have dropped some of the 40 entries put under an 800-byte budget, \
         but all were still present"
    );
}

// ============================================================================
// L1CacheStore
// ============================================================================

/// Wraps an `ObjectStore`, counting `get_opts` calls (which every read --
/// `get`, `get_range`, `head` -- desugars to via `ObjectStoreExt`'s default
/// impls).
#[derive(Debug)]
struct CountingStore {
    inner: Arc<dyn ObjectStore>,
    get_calls: AtomicUsize,
}

impl CountingStore {
    fn new(inner: Arc<dyn ObjectStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            get_calls: AtomicUsize::new(0),
        })
    }

    fn get_calls(&self) -> usize {
        self.get_calls.load(Ordering::SeqCst)
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
        self.get_calls.fetch_add(1, Ordering::SeqCst);
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

/// Like `CountingStore`, but every `get_opts` call fails until the
/// `fail_first_n`-th call, then succeeds -- exercising a scenario where
/// `RangeCache`'s internal fetch (e.g. the `size()` HEAD every `get_range`
/// issues first) fails but a fresh request straight to origin (as
/// `L1CacheStore`'s fallback issues) succeeds.
#[derive(Debug)]
struct FlakyStore {
    inner: Arc<dyn ObjectStore>,
    calls: AtomicUsize,
    fail_first_n: usize,
}

impl FlakyStore {
    fn new(inner: Arc<dyn ObjectStore>, fail_first_n: usize) -> Arc<Self> {
        Arc::new(Self {
            inner,
            calls: AtomicUsize::new(0),
            fail_first_n,
        })
    }
}

impl std::fmt::Display for FlakyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FlakyStore({})", self.inner)
    }
}

#[async_trait]
impl ObjectStore for FlakyStore {
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
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call <= self.fail_first_n {
            return Err(object_store::Error::Generic {
                store: "FlakyStore",
                source: "synthetic failure".into(),
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

async fn put_bytes(store: &dyn ObjectStore, key: &str, data: &[u8]) -> Path {
    let path = Path::from(key);
    store
        .put(&path, Bytes::copy_from_slice(data).into())
        .await
        .expect("put");
    path
}

fn make_backend() -> Arc<BoundedMemoryBackend> {
    Arc::new(BoundedMemoryBackend::new(1024 * 1024))
}

#[tokio::test]
async fn repeat_ranged_read_hits_cache_no_extra_origin_calls() {
    let inmemory = Arc::new(InMemory::new());
    let data = vec![7u8; 4096];
    let path = put_bytes(inmemory.as_ref(), "views/foo.parquet", &data).await;

    let counting = CountingStore::new(inmemory as Arc<dyn ObjectStore>);
    let l1 = L1CacheStore::new(
        counting.clone() as Arc<dyn ObjectStore>,
        make_backend(),
        "lakehouse".to_string(),
    );

    let first = l1.get_range(&path, 0..100).await.expect("first read");
    assert_eq!(first, Bytes::copy_from_slice(&data[0..100]));
    let calls_after_first = counting.get_calls();
    assert!(calls_after_first > 0, "first read must reach origin");

    let second = l1.get_range(&path, 0..100).await.expect("second read");
    assert_eq!(second, Bytes::copy_from_slice(&data[0..100]));
    assert_eq!(
        counting.get_calls(),
        calls_after_first,
        "a repeat read of an already-cached block/size must not touch origin again"
    );
}

#[tokio::test]
async fn ranged_full_and_suffix_reads_return_correct_bytes() {
    let inmemory = Arc::new(InMemory::new());
    let data: Vec<u8> = (0..2000u32).map(|i| (i % 251) as u8).collect();
    let path = put_bytes(inmemory.as_ref(), "views/bar.parquet", &data).await;

    let l1 = L1CacheStore::new(
        inmemory as Arc<dyn ObjectStore>,
        make_backend(),
        "lakehouse".to_string(),
    );

    // Bounded range.
    let ranged = l1.get_range(&path, 500..600).await.expect("ranged read");
    assert_eq!(ranged, Bytes::copy_from_slice(&data[500..600]));

    // Full (unranged) read.
    let full = l1
        .get_opts(&path, GetOptions::default())
        .await
        .expect("full get_opts")
        .bytes()
        .await
        .expect("full bytes");
    assert_eq!(full, Bytes::copy_from_slice(&data));

    // Suffix read.
    let suffix_opts = GetOptions {
        range: Some(GetRange::Suffix(50)),
        ..Default::default()
    };
    let suffix = l1
        .get_opts(&path, suffix_opts)
        .await
        .expect("suffix get_opts")
        .bytes()
        .await
        .expect("suffix bytes");
    assert_eq!(suffix, Bytes::copy_from_slice(&data[data.len() - 50..]));

    // Open-ended (offset) read.
    let offset_opts = GetOptions {
        range: Some(GetRange::Offset(1900)),
        ..Default::default()
    };
    let offset = l1
        .get_opts(&path, offset_opts)
        .await
        .expect("offset get_opts")
        .bytes()
        .await
        .expect("offset bytes");
    assert_eq!(offset, Bytes::copy_from_slice(&data[1900..]));
}

#[tokio::test]
async fn origin_error_falls_back_to_a_fresh_origin_request() {
    let inmemory = Arc::new(InMemory::new());
    let data = vec![3u8; 256];
    let path = put_bytes(inmemory.as_ref(), "views/baz.parquet", &data).await;

    // The first `get_opts` call `RangeCache::get_range` issues internally is
    // the `size()` HEAD; failing exactly that one call forces
    // `L1CacheStore::get_opts` down its fallback path, which reissues the
    // *original* (ranged) request straight to origin -- the second call,
    // which this store lets through.
    let flaky = FlakyStore::new(inmemory as Arc<dyn ObjectStore>, 1);
    let l1 = L1CacheStore::new(
        flaky as Arc<dyn ObjectStore>,
        make_backend(),
        "lakehouse".to_string(),
    );

    let result = l1.get_range(&path, 10..20).await.expect("fallback read");
    assert_eq!(result, Bytes::copy_from_slice(&data[10..20]));
}

#[tokio::test]
async fn put_list_and_delete_pass_through_to_origin() {
    let inmemory = Arc::new(InMemory::new());
    let l1 = L1CacheStore::new(
        inmemory.clone() as Arc<dyn ObjectStore>,
        make_backend(),
        "lakehouse".to_string(),
    );

    let path = Path::from("views/passthrough.parquet");
    l1.put(&path, Bytes::from_static(b"payload").into())
        .await
        .expect("put through L1CacheStore");

    // The write must be visible directly on the origin store (no caching of
    // writes), and via L1's own passthrough `list`.
    assert_eq!(
        inmemory
            .get(&path)
            .await
            .expect("origin has it")
            .bytes()
            .await
            .expect("bytes"),
        Bytes::from_static(b"payload")
    );
    let listed: Vec<Path> = l1
        .list(None)
        .map_ok(|meta| meta.location)
        .try_collect()
        .await
        .expect("list");
    assert!(listed.contains(&path));

    l1.delete(&path).await.expect("delete through L1CacheStore");
    assert!(inmemory.get(&path).await.is_err());
}
