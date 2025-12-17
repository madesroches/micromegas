use datafusion::parquet::file::metadata::ParquetMetaData;
use moka::future::Cache;
use std::sync::Arc;

/// Default cache size for batch operations (10 MB)
const DEFAULT_CACHE_SIZE_BYTES: u64 = 10 * 1024 * 1024;

/// Cache entry storing metadata and its serialized size for weight calculation
#[derive(Clone)]
struct CacheEntry {
    metadata: Arc<ParquetMetaData>,
    serialized_size: u32,
}

/// Global LRU cache for partition metadata, shared across all readers and queries.
///
/// Memory budget is based on serialized metadata size.
pub struct MetadataCache {
    cache: Cache<String, CacheEntry>,
}

impl Default for MetadataCache {
    fn default() -> Self {
        Self::new(DEFAULT_CACHE_SIZE_BYTES)
    }
}

impl MetadataCache {
    /// Creates a new metadata cache with the specified memory budget in bytes.
    pub fn new(max_capacity_bytes: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity_bytes)
            .weigher(|_key: &String, entry: &CacheEntry| -> u32 { entry.serialized_size })
            .build();
        Self { cache }
    }

    /// Gets cached metadata for the given file path, if present.
    pub async fn get(&self, file_path: &str) -> Option<Arc<ParquetMetaData>> {
        self.cache.get(file_path).await.map(|e| e.metadata.clone())
    }

    /// Inserts metadata into the cache with its serialized size for weight calculation.
    pub async fn insert(
        &self,
        file_path: String,
        metadata: Arc<ParquetMetaData>,
        serialized_size: u32,
    ) {
        self.cache
            .insert(
                file_path,
                CacheEntry {
                    metadata,
                    serialized_size,
                },
            )
            .await;
    }

    /// Returns cache statistics (entry_count, weighted_size_bytes).
    pub fn stats(&self) -> (u64, u64) {
        (self.cache.entry_count(), self.cache.weighted_size())
    }
}

impl std::fmt::Debug for MetadataCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (entries, size) = self.stats();
        f.debug_struct("MetadataCache")
            .field("entries", &entries)
            .field("weighted_size_bytes", &size)
            .finish()
    }
}
