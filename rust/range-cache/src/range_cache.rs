use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use bytes::Bytes;
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

    pub async fn size(&self, key: &str) -> Result<u64> {
        let meta_key = format!("meta:{}:{key}", self.ns);

        if let Some(size) = self.size_cache.get(&meta_key).await {
            return Ok(size);
        }

        if let Some(data) = self.backend.get(&meta_key).await
            && data.len() == 8
        {
            let size = u64::from_le_bytes(data[..8].try_into().expect("8-byte size slice"));
            self.size_cache.insert(meta_key, size).await;
            return Ok(size);
        }

        let object_meta = self.origin.head(&Path::from(key)).await?;
        let size = object_meta.size;

        let size_bytes = Bytes::from(size.to_le_bytes().to_vec());
        self.backend.put(meta_key.clone(), size_bytes).await;
        self.size_cache.insert(meta_key, size).await;

        Ok(size)
    }

    async fn get_block(&self, key: &str, block_idx: u64, file_size: u64) -> Result<Bytes> {
        let block_key = format!("blk:{}:{key}:{block_idx}", self.ns);
        let backend = self.backend.clone();
        let origin = self.origin.clone();
        let block_size = self.block_size;
        let key_owned = key.to_string();
        let block_key_clone = block_key.clone();

        self.block_cache
            .try_get_with(block_key, async move {
                if let Some(data) = backend.get(&block_key_clone).await {
                    return Ok(data);
                }
                let blk_range = block_byte_range(block_idx, block_size, file_size);
                let path = Path::from(key_owned.as_str());
                let data = origin
                    .get_range(&path, blk_range)
                    .await
                    .map_err(|e| anyhow!("origin fetch block {block_idx}: {e}"))?;
                backend.put(block_key_clone, data.clone()).await;
                Ok::<Bytes, anyhow::Error>(data)
            })
            .await
            .map_err(|e: Arc<anyhow::Error>| anyhow!("{e}"))
    }

    pub async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes> {
        let file_size = match self.size(key).await {
            Ok(s) => s,
            Err(e) => {
                imetric!("range_cache_miss", "count", 1_u64);
                return Err(e);
            }
        };

        let start = range.start;
        let end = range.end;

        if end > file_size {
            imetric!("range_cache_miss", "count", 1_u64);
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
        let mut block_futures = Vec::new();

        for block_idx in blk_indices.start..blk_indices.end {
            let cache = self.clone();
            let key_owned = key.to_string();
            block_futures.push(tokio::spawn(async move {
                let data = cache.get_block(&key_owned, block_idx, file_size).await?;
                Ok::<(u64, Bytes), anyhow::Error>((block_idx, data))
            }));
        }

        let mut blocks = Vec::with_capacity(block_futures.len());
        for fut in block_futures {
            let (block_idx, data) = fut.await.map_err(|e| anyhow!("join error: {e}"))??;
            blocks.push((block_idx, data));
        }

        blocks.sort_by_key(|(idx, _)| *idx);
        Ok(assemble_range(&blocks, self.block_size, start, end))
    }

    pub async fn get_ranges(&self, key: &str, ranges: &[Range<u64>]) -> Result<Vec<Bytes>> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }

        let file_size = match self.size(key).await {
            Ok(s) => s,
            Err(e) => {
                return Err(anyhow!("size lookup failed: {e}"));
            }
        };

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
                for idx in blk.start..blk.end {
                    all_block_indices.insert(idx);
                }
            }
        }

        let mut block_futures = Vec::new();
        for block_idx in &all_block_indices {
            let cache = self.clone();
            let key_owned = key.to_string();
            let block_idx = *block_idx;
            block_futures.push(tokio::spawn(async move {
                let data = cache.get_block(&key_owned, block_idx, file_size).await?;
                Ok::<(u64, Bytes), anyhow::Error>((block_idx, data))
            }));
        }

        let mut block_map: HashMap<u64, Bytes> = HashMap::new();
        for fut in block_futures {
            let (idx, data) = fut.await.map_err(|e| anyhow!("join error: {e}"))??;
            block_map.insert(idx, data);
        }

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
