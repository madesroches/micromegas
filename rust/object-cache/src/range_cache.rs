use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use bytes::Bytes;
use futures::stream::{self, StreamExt, TryStreamExt};
use micromegas_tracing::prelude::*;
use moka::future::Cache;
use object_store::{ObjectStore, ObjectStoreExt, path::Path};

use super::backend::RangeCacheBackend;
use super::blocks::{assemble_range, block_byte_range, blocks_for_range};

/// Errors returned by [`RangeCache`] that callers may want to handle distinctly.
#[derive(Debug, thiserror::Error)]
pub enum RangeError {
    /// The requested range extends past the end of the object.
    #[error("requested range end {requested_end} exceeds object size {file_size}")]
    OutOfBounds { requested_end: u64, file_size: u64 },
}

pub const DEFAULT_BLOCK_SIZE: u64 = 1024 * 1024;
const MOKA_BLOCK_CAPACITY_BYTES: u64 = 128 * 1024 * 1024;
const MOKA_SIZE_CAPACITY: u64 = 100_000;
/// Upper bound on the number of origin block fetches issued concurrently for a
/// single `get_range`/`get_ranges` call. Without this cap a large read (e.g.
/// 512 MiB at 1 MiB blocks) would fan out to hundreds of simultaneous origin
/// GETs, overwhelming the origin store.
const MAX_CONCURRENT_BLOCK_FETCHES: usize = 16;

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
#[derive(Clone)]
pub struct RangeCache {
    origin: Arc<dyn ObjectStore>,
    backend: Arc<dyn RangeCacheBackend>,
    block_size: u64,
    ns: String,
    block_cache: Cache<String, Bytes>,
    size_cache: Cache<String, u64>,
}

impl RangeCache {
    pub fn new(
        origin: Arc<dyn ObjectStore>,
        backend: Arc<dyn RangeCacheBackend>,
        block_size: u64,
        ns: String,
    ) -> Self {
        let block_cache = Cache::builder()
            .max_capacity(MOKA_BLOCK_CAPACITY_BYTES)
            .weigher(|_k: &String, v: &Bytes| v.len().min(u32::MAX as usize) as u32)
            .build();
        let size_cache = Cache::builder().max_capacity(MOKA_SIZE_CAPACITY).build();
        Self {
            origin,
            backend,
            block_size,
            ns,
            block_cache,
            size_cache,
        }
    }

    #[span_fn]
    pub async fn size(&self, key: &str) -> Result<u64> {
        // The cache key carries no etag/version: see `RangeCache` docs — keys
        // are assumed write-once and content-addressed, so a cached size is
        // never invalidated.
        let meta_key = format!("meta:{}:{key}", self.ns);

        if let Some(size) = self.size_cache.get(&meta_key).await {
            imetric!("range_cache_size_mem_hit", "count", 1_u64);
            return Ok(size);
        }

        if let Some(data) = self.backend.get(&meta_key).await
            && data.len() == 8
        {
            imetric!("range_cache_size_backend_hit", "count", 1_u64);
            let size = u64::from_le_bytes(data[..8].try_into().expect("8-byte size slice"));
            self.size_cache.insert(meta_key, size).await;
            return Ok(size);
        }

        // Size miss: resolve from origin (one `head` per object, then cached
        // forever since objects are write-once).
        imetric!("range_cache_origin_head", "count", 1_u64);
        let object_meta = self.origin.head(&Path::from(key)).await?;
        let size = object_meta.size;
        debug!("range_cache origin head key={key} size={size}");

        let size_bytes = Bytes::from(size.to_le_bytes().to_vec());
        self.backend.put(meta_key.clone(), size_bytes).await;
        self.size_cache.insert(meta_key, size).await;

        Ok(size)
    }

    async fn get_block(&self, key: &str, block_idx: u64, file_size: u64) -> Result<Bytes> {
        // The block key carries no etag/version: see `RangeCache` docs — keys
        // are assumed write-once and content-addressed, so a cached block is
        // never invalidated.
        //
        // Every block request is counted here; the origin-fetch counter below
        // counts only the misses, so the hit rate is derivable as
        // `1 - range_cache_origin_block_fetch / range_cache_block_request`.
        imetric!("range_cache_block_request", "count", 1_u64);
        let block_key = format!("blk:{}:{key}:{block_idx}", self.ns);
        let backend = self.backend.clone();
        let origin = self.origin.clone();
        let block_size = self.block_size;
        let key_owned = key.to_string();
        let block_key_clone = block_key.clone();

        self.block_cache
            .try_get_with(block_key, async move {
                if let Some(data) = backend.get(&block_key_clone).await {
                    imetric!("range_cache_block_backend_hit", "count", 1_u64);
                    return Ok(data);
                }
                // Backend miss: fetch the block from origin, the expensive path
                // this cache exists to avoid. Count it and the bytes pulled.
                let blk_range = block_byte_range(block_idx, block_size, file_size);
                let path = Path::from(key_owned.as_str());
                let data = origin
                    .get_range(&path, blk_range)
                    .await
                    .map_err(|e| anyhow!("origin fetch block {block_idx}: {e}"))?;
                imetric!("range_cache_origin_block_fetch", "count", 1_u64);
                imetric!("range_cache_origin_block_bytes", "bytes", data.len() as u64);
                debug!(
                    "range_cache origin fetch key={key_owned} block={block_idx} bytes={}",
                    data.len()
                );
                backend.put(block_key_clone, data.clone()).await;
                Ok::<Bytes, anyhow::Error>(data)
            })
            .await
            .map_err(|e: Arc<anyhow::Error>| anyhow!("{e}"))
    }

    #[span_fn]
    pub async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes> {
        let file_size = match self.size(key).await {
            Ok(s) => s,
            Err(e) => {
                imetric!("range_cache_get_range_error", "count", 1_u64);
                return Err(e);
            }
        };

        let start = range.start;
        let end = range.end;

        if end > file_size {
            imetric!("range_cache_get_range_error", "count", 1_u64);
            return Err(RangeError::OutOfBounds {
                requested_end: end,
                file_size,
            }
            .into());
        }

        if start >= end {
            return Ok(Bytes::new());
        }

        let blk_indices = blocks_for_range(start, end, self.block_size);

        // Fetch missing blocks with bounded concurrency to avoid fanning out to
        // hundreds of simultaneous origin GETs on a large read. Any block
        // failure aborts the whole call (`try_collect`); ordering is restored by
        // the sort below since `buffer_unordered` yields out of order.
        let mut blocks: Vec<(u64, Bytes)> = stream::iter(blk_indices.start..blk_indices.end)
            .map(|block_idx| {
                let cache = self.clone();
                let key_owned = key.to_string();
                async move {
                    let data = cache.get_block(&key_owned, block_idx, file_size).await?;
                    Ok::<(u64, Bytes), anyhow::Error>((block_idx, data))
                }
            })
            .buffer_unordered(MAX_CONCURRENT_BLOCK_FETCHES)
            .try_collect()
            .await?;

        blocks.sort_by_key(|(idx, _)| *idx);
        Ok(assemble_range(&blocks, self.block_size, start, end))
    }

    #[span_fn]
    pub async fn get_ranges(&self, key: &str, ranges: &[Range<u64>]) -> Result<Vec<Bytes>> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }

        // Propagate the size-lookup error unwrapped so the underlying
        // `object_store::Error` (notably `NotFound`) survives the downcast in
        // callers, matching `get_range` and the single-GET endpoint.
        let file_size = match self.size(key).await {
            Ok(s) => s,
            Err(e) => {
                imetric!("range_cache_get_ranges_error", "count", 1_u64);
                return Err(e);
            }
        };

        let mut all_block_indices = BTreeSet::new();
        for r in ranges {
            let start = r.start;
            let end = r.end;
            if end > file_size {
                imetric!("range_cache_get_ranges_error", "count", 1_u64);
                return Err(RangeError::OutOfBounds {
                    requested_end: end,
                    file_size,
                }
                .into());
            }
            if start < end {
                let blk = blocks_for_range(start, end, self.block_size);
                for idx in blk.start..blk.end {
                    all_block_indices.insert(idx);
                }
            }
        }

        // Fetch all distinct blocks with bounded concurrency (same cap as
        // `get_range`). Any block failure aborts the whole call; results are
        // keyed by block index so out-of-order completion is fine.
        let fetched: Vec<(u64, Bytes)> = stream::iter(all_block_indices)
            .map(|block_idx| {
                let cache = self.clone();
                let key_owned = key.to_string();
                async move {
                    let data = cache.get_block(&key_owned, block_idx, file_size).await?;
                    Ok::<(u64, Bytes), anyhow::Error>((block_idx, data))
                }
            })
            .buffer_unordered(MAX_CONCURRENT_BLOCK_FETCHES)
            .try_collect()
            .await?;
        let block_map: HashMap<u64, Bytes> = fetched.into_iter().collect();

        let mut result = Vec::with_capacity(ranges.len());
        for r in ranges {
            let start = r.start;
            let end = r.end;
            if start >= end {
                result.push(Bytes::new());
                continue;
            }
            let blk = blocks_for_range(start, end, self.block_size);
            let blocks: Vec<(u64, Bytes)> = (blk.start..blk.end)
                .filter_map(|idx| block_map.get(&idx).map(|d| (idx, d.clone())))
                .collect();
            result.push(assemble_range(&blocks, self.block_size, start, end));
        }

        Ok(result)
    }
}
