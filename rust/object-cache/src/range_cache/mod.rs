use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;

use anyhow::Result;
use async_stream::try_stream;
use bytes::{BufMut, Bytes, BytesMut};
use futures::stream::{self, Stream, StreamExt};
use micromegas_tracing::prelude::*;
use object_store::{ObjectStore, ObjectStoreExt, path::Path};

use super::backend::{BackendDiskStats, FillHint, RangeCacheBackend};
use super::blocks::{assemble_range, block_byte_range, blocks_for_range};
use super::metric_tags::{self, PrefixTags};

mod error;
mod fetch;
mod scheduler;

pub use error::{RangeError, StreamRangesCaller};
use scheduler::{
    FetchScheduler, FulfillGuard, Ownership, Priority, decode_size, reconstruct_shared_error,
};

pub const DEFAULT_BLOCK_SIZE: u64 = 1024 * 1024;

/// Number of blocks per streamed fetch window used by `stream_ranges`. At the
/// default 1 MiB block size, 8 blocks (8 MiB) matches one coalesced origin
/// GET run (`DEFAULT_MAX_COALESCED_GET_BYTES`), with `buffered(2)` giving
/// modest pipeline overlap. This bounds peak in-flight memory per stream to
/// roughly `2 * DEMAND_WINDOW_BLOCKS * block_size`, independent of how large
/// the requested range is.
pub const DEMAND_WINDOW_BLOCKS: u64 = 8;

/// Default total number of origin GETs allowed to run concurrently. See
/// `RangeCache::new`.
pub const DEFAULT_TOTAL_FETCH_PERMITS: usize = 32;
/// Default number of `DEFAULT_TOTAL_FETCH_PERMITS` slots reserved for demand
/// reads (never consumed by prefetch).
pub const DEFAULT_DEMAND_RESERVED_FETCH_PERMITS: usize = 8;
/// Default max byte span of one coalesced run GET.
pub const DEFAULT_MAX_COALESCED_GET_BYTES: u64 = 8 * 1024 * 1024;
/// Default promotion granularity: promote only the run(s) covering a demanded
/// block, not the whole prefetch batch.
pub const DEFAULT_PROMOTE_WHOLE_BATCH: bool = false;

/// Upper bound on a plausible cached object size. No micromegas lake object
/// (parquet partition or blob) approaches this; a decoded size above it means a
/// corrupt/misdecoded cache entry, which is treated as a miss and re-resolved
/// from origin rather than driving a catastrophic allocation (#1287).
pub const MAX_PLAUSIBLE_OBJECT_SIZE: u64 = 1 << 48; // 256 TiB

/// Range-aware read cache over an origin object store.
///
/// # Cache invalidation
///
/// This cache assumes object keys are **write-once and content-addressed**: a
/// given key always maps to the same bytes for the lifetime of the object. The
/// size and block caches therefore carry no TTL, etag, or generation in their
/// keys and are never invalidated. Overwriting an existing key with different
/// content would cause stale size/block data to be served indefinitely. This is
/// safe for micromegas lake objects (blocks, parquet) which are never
/// overwritten; do not point this cache at a mutable namespace.
///
/// # In-flight map and priority
///
/// Concurrent fetches of the same block or size are collapsed via an
/// in-flight map (`FetchScheduler`): the first caller to ask for a key
/// becomes its owner and issues the origin request (spawned as a detached
/// task, so a cancelled caller never strands the others waiting on it);
/// every other concurrent caller joins and observes the same result.
/// Contiguous missing blocks the owner controls are coalesced into one
/// `origin.get_range` per run. Every origin GET is either `Demand` or
/// `Prefetch` priority; a demand caller joining a prefetch-priority fetch
/// promotes it (see `own_or_join`), so a late demand read is never stuck
/// behind unrelated prefetch traffic.
#[derive(Clone)]
pub struct RangeCache {
    origin: Arc<dyn ObjectStore>,
    backend: Arc<dyn RangeCacheBackend>,
    block_size: u64,
    ns: String,
    scheduler: Arc<FetchScheduler>,
    max_coalesced_get_bytes: u64,
    /// Configured `prefix` labels (the server's `allowed_prefixes`, leaked to
    /// `'static` once at startup), in the same order as `prefix_tags`. Empty
    /// unless `with_prefix_labels` was used, in which case every key
    /// classifies as `metric_tags::PREFIX_OTHER`.
    prefix_labels: Arc<[&'static str]>,
    /// Precomputed `PrefixTags` parallel to `prefix_labels` (`prefix_tags[i]`
    /// corresponds to `prefix_labels[i]`), so `classify_tags` is an array
    /// lookup rather than an allocation + intern-lock per call.
    prefix_tags: Arc<[PrefixTags]>,
    /// Precomputed tags for a key matching none of `prefix_labels`.
    other_tags: PrefixTags,
}

impl RangeCache {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        origin: Arc<dyn ObjectStore>,
        backend: Arc<dyn RangeCacheBackend>,
        block_size: u64,
        ns: String,
        total_fetch_permits: usize,
        demand_reserved_fetch_permits: usize,
        max_coalesced_get_bytes: u64,
        promote_whole_batch: bool,
    ) -> Self {
        Self {
            origin,
            backend,
            block_size,
            ns,
            scheduler: Arc::new(FetchScheduler::new(
                total_fetch_permits,
                demand_reserved_fetch_permits,
                promote_whole_batch,
            )),
            max_coalesced_get_bytes,
            prefix_labels: Arc::from(Vec::new()),
            prefix_tags: Arc::from(Vec::new()),
            other_tags: PrefixTags::new(metric_tags::PREFIX_OTHER),
        }
    }

    /// Attach a `prefix` classifier for the dimensioned hit-rate/request
    /// metrics: `labels` are the server's configured `allowed_prefixes`,
    /// leaked to `'static` once at startup by the caller (bounded,
    /// low-cardinality, set once). Every request key is then
    /// longest-prefix-matched against `labels` (see `classify`); a key
    /// matching none classifies as `metric_tags::PREFIX_OTHER`.
    ///
    /// `RangeCache::new` itself leaves this empty (every key classifies as
    /// `"other"`), so its existing callers/tests compile unmodified; only
    /// `object_cache_srv.rs` opts in.
    pub fn with_prefix_labels(mut self, labels: Arc<[&'static str]>) -> Self {
        let tags: Vec<PrefixTags> = labels.iter().map(|&label| PrefixTags::new(label)).collect();
        self.prefix_tags = Arc::from(tags);
        self.prefix_labels = labels;
        self
    }

    /// The precomputed tags for the `prefix` `key` falls under, resolved by
    /// longest-prefix match against `prefix_labels` (`other_tags` on no
    /// match). Private: callers needing just the label use `classify`;
    /// hot-path callers inside this module use the tags directly.
    fn classify_tags(&self, key: &str) -> &PrefixTags {
        match metric_tags::longest_prefix_match(&self.prefix_labels, key) {
            Some(i) => &self.prefix_tags[i],
            None => &self.other_tags,
        }
    }

    /// The `prefix` label `key` falls under (e.g. `"blobs"`, `"views"`, per
    /// the server's configured `allowed_prefixes`), or
    /// `metric_tags::PREFIX_OTHER` if it matches none. See
    /// `with_prefix_labels`.
    pub fn classify(&self, key: &str) -> &'static str {
        self.classify_tags(key).label
    }

    /// `(shared_available, shared_total, prefetch_available, prefetch_total)`
    /// -- the fetch-permit budget's current occupancy, for the saturation
    /// sampler.
    pub fn fetch_budget_stats(&self) -> (usize, usize, usize, usize) {
        self.scheduler.fetch_budget_stats()
    }

    /// Number of keys (blocks or `size()` heads) currently in flight to
    /// origin, for the saturation sampler.
    pub fn inflight_len(&self) -> usize {
        self.scheduler.inflight_len()
    }

    /// Backend disk write-path counters (`None` for a backend with no disk
    /// tier), for the saturation sampler's per-second foyer disk gauges.
    pub fn backend_disk_stats(&self) -> Option<BackendDiskStats> {
        self.backend.disk_stats()
    }

    /// Accounted RAM-tier usage (`None` for a backend with no RAM tier), for
    /// the saturation sampler's residency gauge.
    pub fn backend_ram_usage(&self) -> Option<usize> {
        self.backend.ram_usage_bytes()
    }

    /// Size in bytes of one cache block. Every distinct block a request
    /// touches is fetched and held whole, so callers gating memory (e.g. the
    /// server's cross-request budget) need this to account for amplification
    /// from small scattered ranges.
    pub fn block_size(&self) -> u64 {
        self.block_size
    }

    #[span_fn]
    pub async fn size(&self, key: &str) -> Result<u64> {
        // The cache key carries no etag/version: see the module docs — keys
        // are assumed write-once and content-addressed, so a cached size is
        // never invalidated.
        let meta_key = format!("meta:{}:{key}", self.ns);
        let prefix_tag = self.classify_tags(key).prefix;

        if let Some(data) = self.backend.get(&meta_key).await
            && data.len() == 8
        {
            let size = decode_size(&data)?;
            if size <= MAX_PLAUSIBLE_OBJECT_SIZE {
                imetric!("range_cache_size_backend_hit", "count", prefix_tag, 1_u64);
                return Ok(size);
            }
            imetric!("range_cache_size_implausible", "count", prefix_tag, 1_u64);
            warn!("range_cache implausible cached size {size} for key={key}; treating as miss");
            // fall through to origin HEAD, which repopulates meta:{ns}:{key}
        }

        match self
            .scheduler
            .own_or_join(meta_key.clone(), Priority::Demand, None)
        {
            Ownership::Owner(entry) => {
                let origin = self.origin.clone();
                let backend = self.backend.clone();
                let scheduler = self.scheduler.clone();
                let key_owned = key.to_string();
                let meta_key_owned = meta_key.clone();
                let task_entry = entry.clone();
                tokio::spawn(async move {
                    let guard = FulfillGuard::new(
                        scheduler.clone(),
                        vec![(meta_key_owned.clone(), task_entry.clone())],
                    );
                    imetric!("range_cache_origin_head", "count", prefix_tag, 1_u64);
                    let head_path = Path::from(key_owned.as_str());
                    let head_result = instrument_named!(
                        origin.head(&head_path),
                        "range_cache_origin_head_latency"
                    )
                    .await;
                    match head_result {
                        Ok(object_meta) => {
                            let size = object_meta.size;
                            debug!("range_cache origin head key={key_owned} size={size}");
                            let size_bytes = Bytes::from(size.to_le_bytes().to_vec());
                            backend
                                .put(meta_key_owned.clone(), size_bytes.clone(), FillHint::Demand)
                                .await;
                            task_entry.fulfill(Ok(size_bytes));
                        }
                        Err(e) => {
                            task_entry.fulfill(Err(Arc::new(anyhow::Error::from(e))));
                        }
                    }
                    scheduler.remove_entry(&meta_key_owned);
                    guard.disarm();
                });
                let data = entry
                    .join()
                    .await
                    .map_err(|e| reconstruct_shared_error(&e))?;
                decode_size(&data)
            }
            Ownership::Joiner(entry) => {
                let data = entry
                    .join()
                    .await
                    .map_err(|e| reconstruct_shared_error(&e))?;
                decode_size(&data)
            }
        }
    }

    /// Stream the bytes for `ranges` (each half-open `[start, end)`) without
    /// materializing more than a couple of `DEMAND_WINDOW_BLOCKS` windows of
    /// blocks at a time, regardless of how large the ranges are in total.
    ///
    /// Upfront validation — the `size()` lookup and every range's
    /// out-of-bounds check — runs before the stream is constructed and
    /// returned, so 404/416-shaped errors surface synchronously to the
    /// caller with proper status codes; only a failure *after* that point
    /// (a mid-stream `fetch_blocks` error, e.g. the origin going down) is
    /// yielded as the stream's terminal `Err` item. A degenerate
    /// `start >= end` range is validated like any other (its `end` is still
    /// checked against `file_size`) but yields no bytes.
    ///
    /// Yields a flat, ordered sequence of chunks: ranges are processed in
    /// the order given and each range's bytes are emitted contiguously
    /// (possibly split into several `Bytes` chunks at window boundaries)
    /// before the next range's bytes begin. There is no cross-range block
    /// dedup — a block shared by two ranges is fetched once per range it
    /// appears in, though a repeat is always a backend hit or an in-flight
    /// join, never a second origin GET (see `own_or_join`).
    ///
    /// `caller` selects which of the two distinct error counters
    /// (`range_cache_get_range_error` / `range_cache_get_ranges_error`) this
    /// call emits on validation failure or a mid-stream fetch error, so
    /// `get_range`/`get_ranges` (and the two HTTP handlers, which call this
    /// directly) keep emitting the metric they always have.
    #[span_fn]
    pub async fn stream_ranges(
        &self,
        key: &str,
        ranges: Vec<Range<u64>>,
        caller: StreamRangesCaller,
    ) -> Result<impl Stream<Item = Result<Bytes>> + Send + 'static> {
        let file_size = match self.size(key).await {
            Ok(s) => s,
            Err(e) => {
                caller.emit_error_metric();
                return Err(e);
            }
        };
        self.stream_ranges_inner(key, ranges, file_size, caller)
            .await
    }

    /// Like `stream_ranges`, but for a caller that already resolved
    /// `file_size` itself (e.g. `get_range_handler`, which needs it up front
    /// for range validation): skips the redundant `self.size()` call, so a
    /// cache hit doesn't fire `range_cache_size_backend_hit` a second time
    /// per call.
    #[span_fn]
    pub async fn stream_ranges_with_size(
        &self,
        key: &str,
        ranges: Vec<Range<u64>>,
        file_size: u64,
        caller: StreamRangesCaller,
    ) -> Result<impl Stream<Item = Result<Bytes>> + Send + 'static> {
        self.stream_ranges_inner(key, ranges, file_size, caller)
            .await
    }

    async fn stream_ranges_inner(
        &self,
        key: &str,
        ranges: Vec<Range<u64>>,
        file_size: u64,
        caller: StreamRangesCaller,
    ) -> Result<impl Stream<Item = Result<Bytes>> + Send + 'static> {
        for r in &ranges {
            if r.end > file_size {
                caller.emit_error_metric();
                return Err(RangeError::OutOfBounds {
                    requested_end: r.end,
                    file_size,
                }
                .into());
            }
        }

        let cache = self.clone();
        let key = key.to_string();
        Ok(try_stream! {
            for r in ranges {
                if r.start >= r.end {
                    continue;
                }
                let blk_range = blocks_for_range(r.start, r.end, cache.block_size);
                let mut windows = cache.stream_demand_windows(&key, file_size, blk_range);
                while let Some((w, result)) = windows.next().await {
                    let block_map = result.inspect_err(|_| caller.emit_error_metric())?;
                    yield cache.assemble_window(&block_map, &w, r.start, r.end, file_size);
                }
            }
        })
    }

    /// Build the ordered, bounded stream of demand-priority window fetches for
    /// one requested range's block span `blk_range`: chunk it into
    /// `DEMAND_WINDOW_BLOCKS`-sized windows and fetch each (at most two in
    /// flight via `buffered(2)`), yielding `(window_indices, fetch_result)`.
    /// Extracted from `stream_ranges_inner`'s `try_stream!` body to keep that
    /// generator small; the returned stream owns its own `RangeCache`/key
    /// clones so it is `'static`.
    fn stream_demand_windows(
        &self,
        key: &str,
        file_size: u64,
        blk_range: Range<u64>,
    ) -> impl Stream<Item = (Vec<u64>, Result<HashMap<u64, Bytes>>)> + Send + 'static {
        let window_indices: Vec<Vec<u64>> = (blk_range.start..blk_range.end)
            .collect::<Vec<u64>>()
            .chunks(DEMAND_WINDOW_BLOCKS as usize)
            .map(|w| w.to_vec())
            .collect();
        let cache = self.clone();
        let key = key.to_string();
        stream::iter(window_indices)
            .map(move |w| {
                let cache = cache.clone();
                let key = key.clone();
                async move {
                    let result = cache
                        .fetch_blocks(&key, file_size, &w, Priority::Demand)
                        .await;
                    (w, result)
                }
            })
            .buffered(2)
    }

    /// Assemble the bytes for one window: gather its blocks from `block_map`,
    /// then clamp to the window's own byte span intersected with the outer
    /// requested range `[req_start, req_end)` before assembling. Clamping to
    /// the window (not the whole range) matters because `block_map` holds only
    /// this window's data and `assemble_range` pre-sizes its output buffer from
    /// `req_end - req_start`, so passing the full range's bounds on every
    /// window would over-allocate by up to the entire range's size each
    /// iteration.
    fn assemble_window(
        &self,
        block_map: &HashMap<u64, Bytes>,
        window: &[u64],
        req_start: u64,
        req_end: u64,
        file_size: u64,
    ) -> Bytes {
        let blocks: Vec<(u64, Bytes)> = window
            .iter()
            .map(|idx| {
                let data = block_map
                    .get(idx)
                    .cloned()
                    .expect("fetch_blocks returns every requested index");
                (*idx, data)
            })
            .collect();
        let win_start = window[0] * self.block_size;
        let win_end = block_byte_range(
            *window.last().expect("window is non-empty"),
            self.block_size,
            file_size,
        )
        .end;
        let local_start = req_start.max(win_start);
        let local_end = req_end.min(win_end);
        assemble_range(&blocks, self.block_size, local_start, local_end)
    }

    #[span_fn]
    pub async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes> {
        let mut stream = Box::pin(
            self.stream_ranges(key, vec![range], StreamRangesCaller::Range)
                .await?,
        );
        let mut buf = BytesMut::new();
        while let Some(chunk) = stream.next().await {
            buf.put_slice(&chunk?);
        }
        Ok(buf.freeze())
    }

    /// Like `get_range`, but for a caller that already resolved `file_size`
    /// itself, skipping the redundant `self.size()` call inside
    /// `stream_ranges` (see `stream_ranges_with_size`).
    #[span_fn]
    pub async fn get_range_with_size(
        &self,
        key: &str,
        file_size: u64,
        range: Range<u64>,
    ) -> Result<Bytes> {
        let mut stream = Box::pin(
            self.stream_ranges_with_size(key, vec![range], file_size, StreamRangesCaller::Range)
                .await?,
        );
        let mut buf = BytesMut::new();
        while let Some(chunk) = stream.next().await {
            buf.put_slice(&chunk?);
        }
        Ok(buf.freeze())
    }

    #[span_fn]
    pub async fn get_ranges(&self, key: &str, ranges: &[Range<u64>]) -> Result<Vec<Bytes>> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }
        let owned_ranges: Vec<Range<u64>> = ranges.to_vec();
        let stream = Box::pin(
            self.stream_ranges(key, owned_ranges, StreamRangesCaller::Ranges)
                .await?,
        );
        collect_ranges_from_stream(ranges, stream).await
    }

    /// Like `get_ranges`, but for a caller that already resolved `file_size`
    /// itself (see `stream_ranges_with_size`).
    #[span_fn]
    pub async fn get_ranges_with_size(
        &self,
        key: &str,
        file_size: u64,
        ranges: &[Range<u64>],
    ) -> Result<Vec<Bytes>> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }
        let owned_ranges: Vec<Range<u64>> = ranges.to_vec();
        let stream = Box::pin(
            self.stream_ranges_with_size(key, owned_ranges, file_size, StreamRangesCaller::Ranges)
                .await?,
        );
        collect_ranges_from_stream(ranges, stream).await
    }

    /// Warm the cache for `ranges` at `Prefetch` priority without returning
    /// any bytes. The HTTP surface for this (endpoint + client method) is
    /// #1198; this is the priority-carrying core it builds on. Public (rather
    /// than crate-private) so integration tests under `tests/` — which
    /// compile as a separate crate — can exercise the promotion behavior
    /// described in the fetch-rework plan.
    pub async fn prefetch_ranges(&self, key: &str, ranges: &[Range<u64>]) -> Result<()> {
        if ranges.is_empty() {
            return Ok(());
        }
        let file_size = self.size(key).await?;
        let mut all_block_indices = BTreeSet::new();
        for r in ranges {
            let start = r.start;
            let end = r.end;
            if end > file_size {
                return Err(RangeError::OutOfBounds {
                    requested_end: end,
                    file_size,
                }
                .into());
            }
            if start < end {
                let blk = blocks_for_range(start, end, self.block_size);
                all_block_indices.extend(blk.start..blk.end);
            }
        }
        self.prefetch_blocks(
            key,
            file_size,
            &all_block_indices.into_iter().collect::<Vec<_>>(),
        )
        .await
    }

    /// Warm the cache for the given block indices at `Prefetch` priority.
    pub async fn prefetch_blocks(&self, key: &str, file_size: u64, indices: &[u64]) -> Result<()> {
        self.fetch_blocks(key, file_size, indices, Priority::Prefetch)
            .await?;
        Ok(())
    }
}

/// Reassemble the flat, ordered chunk sequence a `stream_ranges*` stream
/// yields back into one `Bytes` per requested range, using each range's
/// known length rather than relying on a chunk boundary lining up with a
/// range boundary. Shared by `get_ranges` and `get_ranges_with_size`.
async fn collect_ranges_from_stream(
    ranges: &[Range<u64>],
    mut stream: impl Stream<Item = Result<Bytes>> + Unpin,
) -> Result<Vec<Bytes>> {
    let mut result = Vec::with_capacity(ranges.len());
    let mut pending: Option<Bytes> = None;
    for r in ranges {
        let start = r.start;
        let end = r.end;
        if start >= end {
            // `stream_ranges` yields nothing for a degenerate range, so
            // reinsert the empty chunk at its position ourselves.
            result.push(Bytes::new());
            continue;
        }
        let need = (end - start) as usize;
        let mut collected = BytesMut::with_capacity(need);
        while collected.len() < need {
            let chunk = match pending.take() {
                Some(c) => c,
                None => stream
                    .next()
                    .await
                    .expect("stream_ranges under-yielded for a non-degenerate range")?,
            };
            let remaining = need - collected.len();
            if chunk.len() > remaining {
                collected.put_slice(&chunk[..remaining]);
                pending = Some(chunk.slice(remaining..));
            } else {
                collected.put_slice(&chunk);
            }
        }
        result.push(collected.freeze());
    }
    Ok(result)
}
