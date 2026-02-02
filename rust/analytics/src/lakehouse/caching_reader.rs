use bytes::Bytes;
use datafusion::parquet::errors::ParquetError;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::file_cache::FileCache;

/// Adds file content caching to object store reads.
///
/// This is an internal component used by `ParquetReader`, not a standalone `AsyncFileReader`.
/// It only provides `get_bytes` and `get_byte_ranges` methods - metadata handling remains
/// in the `ParquetReader` layer.
///
/// Uses a two-level caching strategy:
/// 1. Local `cached_data` - avoids global cache lookups within a single reader
/// 2. Global `FileCache` - shared across all readers, with thundering herd protection
pub struct CachingReader {
    /// Object store for loading uncached files (shared, cloneable)
    object_store: Arc<dyn ObjectStore>,
    /// Path to the file in object store
    path: object_store::path::Path,
    filename: String,
    file_size: u64,
    file_cache: Arc<FileCache>,
    /// Local cache of file data for this reader instance
    cached_data: Option<Bytes>,
    /// Whether the most recent read operation was served from cache
    last_read_was_cache_hit: bool,
}

impl CachingReader {
    pub fn new(
        object_store: Arc<dyn ObjectStore>,
        path: object_store::path::Path,
        filename: String,
        file_size: u64,
        file_cache: Arc<FileCache>,
    ) -> Self {
        Self {
            object_store,
            path,
            filename,
            file_size,
            file_cache,
            cached_data: None,
            last_read_was_cache_hit: false,
        }
    }

    /// Returns whether the most recent read operation was served from cache.
    pub fn last_read_was_cache_hit(&self) -> bool {
        self.last_read_was_cache_hit
    }

    /// Load file data, using cache with thundering herd protection.
    /// Returns the data and sets `last_read_was_cache_hit` accordingly.
    async fn load_file_data(&mut self) -> datafusion::parquet::errors::Result<Bytes> {
        // Check local cache first (avoids global cache lookup)
        if let Some(data) = &self.cached_data {
            self.last_read_was_cache_hit = true;
            return Ok(data.clone());
        }

        // Use get_or_load for thundering herd protection - concurrent requests
        // for the same file will coalesce into a single object store fetch.
        // Track whether the loader was called to determine cache hit/miss.
        let loader_was_called = Arc::new(AtomicBool::new(false));
        let loader_was_called_clone = Arc::clone(&loader_was_called);

        let object_store = Arc::clone(&self.object_store);
        let path = self.path.clone();
        let filename = self.filename.clone();
        let file_size = self.file_size;

        let data = self
            .file_cache
            .get_or_load(&self.filename, self.file_size, || {
                loader_was_called_clone.store(true, Ordering::SeqCst);
                async move {
                    debug!("file_cache_load file={filename} file_size={file_size}");
                    let result = object_store.get(&path).await?;
                    result.bytes().await
                }
            })
            .await
            .map_err(|e| ParquetError::General(e.to_string()))?;

        self.cached_data = Some(data.clone());
        self.last_read_was_cache_hit = !loader_was_called.load(Ordering::SeqCst);
        Ok(data)
    }

    pub async fn get_bytes(
        &mut self,
        range: Range<u64>,
    ) -> datafusion::parquet::errors::Result<Bytes> {
        if self.file_cache.should_cache(self.file_size) {
            let data = self.load_file_data().await?;
            Ok(data.slice(range.start as usize..range.end as usize))
        } else {
            // Large file - read directly from object store (bypass cache)
            self.last_read_was_cache_hit = false;
            debug!(
                "file_cache_skip file={} file_size={}",
                self.filename, self.file_size
            );
            self.object_store
                .get_range(&self.path, range)
                .await
                .map_err(|e| ParquetError::External(Box::new(e)))
        }
    }

    pub async fn get_byte_ranges(
        &mut self,
        ranges: Vec<Range<u64>>,
    ) -> datafusion::parquet::errors::Result<Vec<Bytes>> {
        if self.file_cache.should_cache(self.file_size) {
            let data = self.load_file_data().await?;
            Ok(ranges
                .into_iter()
                .map(|r| data.slice(r.start as usize..r.end as usize))
                .collect())
        } else {
            // Large file - use object_store's get_ranges for efficient multi-range fetch
            self.last_read_was_cache_hit = false;
            debug!(
                "file_cache_skip file={} file_size={}",
                self.filename, self.file_size
            );
            let results = self
                .object_store
                .get_ranges(&self.path, &ranges)
                .await
                .map_err(|e| ParquetError::External(Box::new(e)))?;
            Ok(results)
        }
    }
}
