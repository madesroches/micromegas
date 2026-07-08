use std::ops::Range;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{self, BoxStream};
use micromegas_tracing::prelude::*;
use object_store::{
    Attributes, CopyOptions, GetOptions, GetRange, GetResult, GetResultPayload, ListResult,
    MultipartUpload, ObjectMeta, ObjectStore, PutMultipartOptions, PutOptions, PutPayload,
    PutResult, path::Path,
};

use crate::backend::RangeCacheBackend;
use crate::bounded_memory_backend::BoundedMemoryBackend;
use crate::range_cache::{
    DEFAULT_BLOCK_SIZE, DEFAULT_MAX_COALESCED_GET_BYTES, DEFAULT_PROMOTE_WHOLE_BATCH, RangeCache,
};

/// Environment variable sizing the shared in-process L1 RAM budget, in MB.
/// `0` disables L1 (`l1_wrap` then returns the origin store unchanged).
const ENV_L1_CACHE_MB: &str = "MICROMEGAS_L1_CACHE_MB";
/// Matches the old whole-file `FileCache`'s default budget.
const DEFAULT_L1_CACHE_MB: u64 = 200;

/// Total number of origin GETs the in-process L1 cache allows concurrently.
/// L1 issues only demand reads (`RangeCache::get_range`/`get_ranges`), so
/// this is the only fetch-permit knob that matters here (see
/// `L1_DEMAND_RESERVED_FETCH_PERMITS`). Sized smaller than the dedicated
/// object-cache-srv's default (32): an in-process L1 shares the process with
/// DataFusion's own working set, so a smaller concurrent-fetch /
/// transient-buffer footprint (roughly
/// `L1_TOTAL_FETCH_PERMITS * DEFAULT_MAX_COALESCED_GET_BYTES`) is preferable.
const L1_TOTAL_FETCH_PERMITS: usize = 16;
/// Placeholder satisfying `RangeCache::new`'s `demand_reserved < total`
/// assertion. This sizes the prefetch-only semaphore, but L1 never issues
/// prefetch reads, so the value has no other effect.
const L1_DEMAND_RESERVED_FETCH_PERMITS: usize = 4;

/// The shared in-process L1 RAM budget, lazily sized once (on first use) from
/// `MICROMEGAS_L1_CACHE_MB`. `None` when L1 is disabled (budget `0`).
static SHARED_L1_BACKEND: OnceLock<Option<Arc<BoundedMemoryBackend>>> = OnceLock::new();

fn shared_l1_backend() -> Option<Arc<BoundedMemoryBackend>> {
    SHARED_L1_BACKEND
        .get_or_init(|| {
            let mb = match std::env::var(ENV_L1_CACHE_MB) {
                Ok(s) => s.parse::<u64>().unwrap_or_else(|_| {
                    warn!(
                        "Invalid {ENV_L1_CACHE_MB} value '{s}', using default {DEFAULT_L1_CACHE_MB} MB"
                    );
                    DEFAULT_L1_CACHE_MB
                }),
                Err(_) => DEFAULT_L1_CACHE_MB,
            };
            if mb == 0 {
                info!("{ENV_L1_CACHE_MB}=0, in-process L1 cache disabled");
                None
            } else {
                info!("in-process L1 cache enabled, budget={mb}MB");
                Some(Arc::new(BoundedMemoryBackend::new(
                    (mb * 1024 * 1024) as usize,
                )))
            }
        })
        .clone()
}

/// Wrap `origin` with the in-process L1 cache: a `RangeCache` over the shared
/// bounded RAM backend, namespaced by `ns` (e.g. `"lakehouse"`, `"static"`) so
/// distinct wrap sites share one RAM budget without their keys colliding.
///
/// Returns `origin` unchanged when L1 is disabled (`MICROMEGAS_L1_CACHE_MB=0`).
pub fn l1_wrap(origin: Arc<dyn ObjectStore>, ns: &str) -> Arc<dyn ObjectStore> {
    match shared_l1_backend() {
        Some(backend) => Arc::new(L1CacheStore::new(origin, backend, ns.to_string())),
        None => origin,
    }
}

/// An `ObjectStore` adapter fronting `origin` with a RAM-backed `RangeCache`.
///
/// `get_opts`/`get_ranges` are served from the cache (falling back to
/// `origin` on any cache error, mirroring `CacheClientStore`'s
/// graceful-degradation contract); everything else -- `put`, `list`,
/// `delete`, `copy`, and preconditioned or `head` gets -- passes straight
/// through to `origin`, which is never itself cached by this store.
pub struct L1CacheStore {
    cache: RangeCache,
    origin: Arc<dyn ObjectStore>,
}

impl L1CacheStore {
    pub fn new(
        origin: Arc<dyn ObjectStore>,
        backend: Arc<dyn RangeCacheBackend>,
        ns: String,
    ) -> Self {
        let cache = RangeCache::new(
            origin.clone(),
            backend,
            DEFAULT_BLOCK_SIZE,
            ns,
            L1_TOTAL_FETCH_PERMITS,
            L1_DEMAND_RESERVED_FETCH_PERMITS,
            DEFAULT_MAX_COALESCED_GET_BYTES,
            DEFAULT_PROMOTE_WHOLE_BATCH,
        );
        Self { cache, origin }
    }

    fn key(location: &Path) -> String {
        location.as_ref().to_string()
    }

    async fn fallback_get_opts(
        &self,
        location: &Path,
        options: GetOptions,
        error: anyhow::Error,
    ) -> object_store::Result<GetResult> {
        imetric!("l1_cache_fallback", "count", 1_u64);
        debug!("L1 cache miss for {location}, falling back to origin: {error}");
        self.origin.get_opts(location, options).await
    }
}

impl std::fmt::Debug for L1CacheStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "L1CacheStore({})", self.origin)
    }
}

impl std::fmt::Display for L1CacheStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "L1CacheStore({})", self.origin)
    }
}

/// Build a single-chunk streaming `GetResult` from already-fetched bytes.
fn single_chunk_get_result(
    data: Bytes,
    range: Range<u64>,
    object_size: u64,
    location: &Path,
) -> GetResult {
    let meta = ObjectMeta {
        location: location.clone(),
        last_modified: chrono::Utc::now(),
        size: object_size,
        e_tag: None,
        version: None,
    };
    let payload = GetResultPayload::Stream(Box::pin(stream::once(async move { Ok(data) })));
    GetResult {
        payload,
        meta,
        range,
        attributes: Attributes::default(),
    }
}

#[async_trait]
impl ObjectStore for L1CacheStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        self.origin.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.origin.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        // The L1 protocol carries no conditional/version preconditions and a
        // head-only request needs no bytes: both go straight to origin.
        if options.if_match.is_some()
            || options.if_none_match.is_some()
            || options.if_modified_since.is_some()
            || options.if_unmodified_since.is_some()
            || options.version.is_some()
            || options.head
        {
            return self.origin.get_opts(location, options).await;
        }

        let key = Self::key(location);
        let range = options.range.clone();

        let resolved: anyhow::Result<(Bytes, Range<u64>, u64)> = async {
            match &range {
                Some(GetRange::Bounded(r)) => {
                    let r = r.clone();
                    let data = self.cache.get_range(&key, r.clone()).await?;
                    // Skip resolving the full object size here: our only
                    // caller for this arm is `ObjectStore::get_range`'s
                    // default impl, which reads the result via `.bytes()`
                    // -- that only consults `range`, never `meta.size` --
                    // so avoid an extra `cache.size()` round trip and reuse
                    // the requested end as a placeholder.
                    let placeholder_size = r.end;
                    Ok((data, r, placeholder_size))
                }
                other => {
                    let size = self.cache.size(&key).await?;
                    let resolved_range = match other {
                        None => 0..size,
                        Some(GetRange::Offset(offset)) => *offset..size,
                        Some(GetRange::Suffix(suffix)) => size.saturating_sub(*suffix)..size,
                        Some(GetRange::Bounded(_)) => unreachable!("handled above"),
                    };
                    let data = self.cache.get_range(&key, resolved_range.clone()).await?;
                    Ok((data, resolved_range, size))
                }
            }
        }
        .await;

        match resolved {
            Ok((data, range, size)) => Ok(single_chunk_get_result(data, range, size, location)),
            Err(e) => self.fallback_get_opts(location, options, e).await,
        }
    }

    async fn get_ranges(
        &self,
        location: &Path,
        ranges: &[Range<u64>],
    ) -> object_store::Result<Vec<Bytes>> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }
        let key = Self::key(location);
        match self.cache.get_ranges(&key, ranges).await {
            Ok(results) => Ok(results),
            Err(e) => {
                imetric!("l1_cache_fallback", "count", 1_u64);
                debug!("L1 cache miss for {location} (ranges), falling back to origin: {e}");
                self.origin.get_ranges(location, ranges).await
            }
        }
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        self.origin.delete_stream(locations)
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.origin.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        self.origin.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        self.origin.copy_opts(from, to, options).await
    }
}
