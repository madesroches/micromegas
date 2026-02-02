use anyhow::bail;
use bytes::Bytes;
use micromegas_tracing::prelude::*;
use moka::future::Cache;
use moka::notification::RemovalCause;
use std::future::Future;
use std::sync::Arc;

/// Default cache size (200 MB)
const DEFAULT_CACHE_SIZE_BYTES: u64 = 200 * 1024 * 1024;

/// Default max file size to cache (10 MB)
const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 10 * 1024 * 1024;

/// Cache entry storing file data and metadata for weight calculation
#[derive(Clone)]
struct CacheEntry {
    data: Bytes,
    file_size: u32,
    /// Timestamp when the entry was inserted (in ticks from now())
    inserted_at: i64,
}

/// Global LRU cache for parquet file contents, shared across all readers and queries.
///
/// Memory budget is based on file size. Uses moka's `try_get_with` to prevent
/// thundering herd - concurrent requests for the same uncached file will coalesce
/// into a single load operation.
pub struct FileCache {
    cache: Cache<String, CacheEntry>,
    max_file_size: u64,
}

impl Default for FileCache {
    fn default() -> Self {
        Self::new(DEFAULT_CACHE_SIZE_BYTES, DEFAULT_MAX_FILE_SIZE_BYTES)
    }
}

impl FileCache {
    /// Creates a new file cache with the specified memory budget and max file size.
    pub fn new(max_capacity_bytes: u64, max_file_size_bytes: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity_bytes)
            .weigher(|_key: &String, entry: &CacheEntry| -> u32 { entry.file_size })
            .eviction_listener(
                |_key: Arc<String>, entry: CacheEntry, cause: RemovalCause| {
                    if cause == RemovalCause::Size {
                        // Track eviction delay: time between insertion and eviction due to size pressure
                        let eviction_delay = now() - entry.inserted_at;
                        imetric!("file_cache_eviction_delay", "ticks", eviction_delay as u64);
                    }
                },
            )
            .build();

        Self {
            cache,
            max_file_size: max_file_size_bytes,
        }
    }

    /// Check if a file should be cached based on its size
    pub fn should_cache(&self, file_size: u64) -> bool {
        file_size <= self.max_file_size
    }

    /// Gets file contents, loading from the provided async function on cache miss.
    ///
    /// Uses moka's `try_get_with` to coalesce concurrent requests - if multiple
    /// callers request the same uncached file simultaneously, only one will
    /// execute the loader while others wait for the result.
    ///
    /// Returns an error if file_size >= 4GB (moka weigher uses u32).
    pub async fn get_or_load<F, Fut, E>(
        &self,
        file_path: &str,
        file_size: u64,
        loader: F,
    ) -> anyhow::Result<Bytes>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Bytes, E>>,
        E: Send + Sync + std::error::Error + 'static,
    {
        if file_size > u32::MAX as u64 {
            bail!(
                "file too large to cache: {file_size} bytes (max {})",
                u32::MAX
            );
        }
        let file_size_u32 = file_size as u32;
        // Note: entry_count may be stale under concurrent loads of different files (approximate metric)
        let entry_count = self.cache.entry_count();
        let result = self
            .cache
            .try_get_with(file_path.to_string(), async {
                let data = loader().await.map_err(|e| anyhow::anyhow!(e))?;
                imetric!("file_cache_entry_count", "count", entry_count + 1);
                Ok::<_, anyhow::Error>(CacheEntry {
                    data,
                    file_size: file_size_u32,
                    inserted_at: now(),
                })
            })
            .await
            .map_err(|e: Arc<anyhow::Error>| anyhow::anyhow!("{e}"))?;
        Ok(result.data.clone())
    }

    /// Returns cache statistics (entry_count, weighted_size_bytes).
    pub fn stats(&self) -> (u64, u64) {
        (self.cache.entry_count(), self.cache.weighted_size())
    }

    /// Runs pending cache maintenance tasks.
    ///
    /// This should be called to ensure cache statistics are up-to-date,
    /// particularly useful in test scenarios.
    pub async fn run_pending_tasks(&self) {
        self.cache.run_pending_tasks().await;
    }
}

impl std::fmt::Debug for FileCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (entries, size) = self.stats();
        f.debug_struct("FileCache")
            .field("entries", &entries)
            .field("weighted_size_bytes", &size)
            .field("max_file_size", &self.max_file_size)
            .finish()
    }
}
