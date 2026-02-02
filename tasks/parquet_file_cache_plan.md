# Plan: Parquet File Cache Implementation

## Goal
Implement an in-memory cache for parquet file contents to reduce object storage reads and improve query performance.

## Analysis Summary (real-world data from remote object storage)

> **Note**: Data represents ~8 minutes of activity. Longer time windows will have larger working sets, but recent files are accessed most frequently, so LRU eviction remains effective.

- **29,059 reads** of **6,084 unique files** (4.78 reads/file avg)
- **79.1% potential cache hit rate** (files read multiple times)
- **2.90x read amplification** (1,411 MB transferred for 486 MB unique data)
- **Latency**: 14-266ms per read (median 24ms, P99 80ms)
- **76% of total latency** would be eliminated by caching
- **Largest file**: 5.8 MB (100% of files under 10MB threshold)

### File Size Distribution
| Bucket | Count | Percentage |
|--------|-------|------------|
| <10KB | 2,164 | 35.6% |
| 10KB-100KB | 3,147 | 51.7% |
| 100KB-1MB | 675 | 11.1% |
| 1MB-10MB | 98 | 1.6% |

### LRU Cache Sizing
| Cache Size | Hit Rate |
|------------|----------|
| 50 MB | 61.7% |
| 100 MB | 68.7% |
| 200 MB | 75.0% |
| 500 MB | 79.1% |

## Design Decisions

### Whole Files vs Segments

**Recommendation: Cache whole files** with a size threshold.

**Rationale:**
1. Files are small (median 16KB, 87% under 100KB, largest 5.8MB)
2. Multiple byte ranges of the same file are read per query (avg 7.7 ranges/read)
3. Simpler implementation, follows existing MetadataCache pattern
4. moka's weight-based eviction handles memory efficiently

### Handling Large Files

**Approach: Skip caching files above threshold (default 10MB)**

- Files > threshold: read directly from object store (pass-through)
- Files <= threshold: cache entire file on first read
- Threshold configurable via environment variable

**Future enhancement**: Segment-based caching for large files if needed.

### Layered Architecture

**Use concrete types with direct ownership for composable reader layers.**

Rather than embedding cache logic directly in `ParquetReader`, create a separate `CachingReader` wrapper:

```
ReaderFactory
    └── ParquetReader (metadata handling)
            └── CachingReader (file content caching + object store I/O)
                    └── ObjectStore (via Arc, for uncached/large file reads)
```

**Rationale:**
- Separation of concerns - each layer has single responsibility
- Testability - cache layer testable with mock object stores
- Composability - can add other layers (retries, metrics, rate limiting)
- No Mutex needed - `&mut self` on `AsyncFileReader` methods already guarantees exclusive access
- Concrete types avoid dynamic dispatch overhead and simplify the code

### Thundering Herd Protection

**Problem:** Without protection, N concurrent requests for the same uncached file would all miss the cache and each fetch the entire file from object storage, wasting bandwidth and increasing latency.

**Solution:** Use moka's `try_get_with()` API which provides built-in request coalescing:
- First request initiates the load and holds a lock on that key
- Concurrent requests for the same key wait on the first request
- When the load completes, all waiters receive the same cached data
- Failed loads propagate the error to all waiters (no negative caching)

This is critical for query performance since DataFusion may spawn multiple parallel tasks that read the same files.

## Implementation

### Files to Create/Modify

| File | Action |
|------|--------|
| `rust/analytics/src/lakehouse/file_cache.rs` | **Create** - Cache storage (moka-based) |
| `rust/analytics/src/lakehouse/caching_reader.rs` | **Create** - `CachingReader` wrapper implementing `AsyncFileReader` |
| `rust/analytics/src/lakehouse/mod.rs` | **Modify** - Add module exports |
| `rust/analytics/src/lakehouse/reader_factory.rs` | **Modify** - Compose reader layers |

### 1. Create FileCache (`file_cache.rs`)

```rust
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
                        imetric!(
                            "file_cache_eviction_delay",
                            "ticks",
                            eviction_delay as u64
                        );
                    }
                },
            )
            .build();

        Self { cache, max_file_size: max_file_size_bytes }
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
    pub async fn get_or_load<F, Fut, E>(
        &self,
        file_path: &str,
        file_size: u64,
        loader: F,
    ) -> Result<Bytes, Arc<E>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Bytes, E>>,
        E: Send + Sync + 'static,
    {
        let file_size_u32 = file_size as u32;
        let result = self
            .cache
            .try_get_with(file_path.to_string(), async {
                let data = loader().await?;
                imetric!("file_cache_entry_count", "count", self.cache.entry_count() + 1);
                Ok(CacheEntry {
                    data,
                    file_size: file_size_u32,
                    inserted_at: now(),
                })
            })
            .await?;
        Ok(result.data.clone())
    }

    /// Returns cache statistics (entry_count, weighted_size_bytes).
    pub fn stats(&self) -> (u64, u64) {
        (self.cache.entry_count(), self.cache.weighted_size())
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
```

### 2. Create CachingReader (`caching_reader.rs`)

A wrapper that adds caching to any `AsyncFileReader`:

```rust
use bytes::Bytes;
use datafusion::parquet::{
    arrow::{
        arrow_reader::ArrowReaderOptions,
        async_reader::{AsyncFileReader, ParquetObjectReader},
    },
    errors::ParquetError,
    file::metadata::ParquetMetaData,
};
use futures::future::BoxFuture;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::ops::Range;
use std::sync::Arc;

use super::file_cache::FileCache;

/// Wrapper that adds file content caching to a ParquetObjectReader.
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
        }
    }

    /// Load file data, using cache with thundering herd protection.
    async fn load_file_data(&mut self) -> datafusion::parquet::errors::Result<Bytes> {
        // Check local cache first (avoids global cache lookup)
        if let Some(data) = &self.cached_data {
            return Ok(data.clone());
        }

        // Use get_or_load for thundering herd protection - concurrent requests
        // for the same file will coalesce into a single object store fetch
        let object_store = Arc::clone(&self.object_store);
        let path = self.path.clone();
        let filename = self.filename.clone();
        let file_size = self.file_size;

        let data = self
            .file_cache
            .get_or_load(&self.filename, self.file_size, || async move {
                debug!("file_cache_load file={filename} file_size={file_size}");
                let result = object_store.get(&path).await.map_err(|e| {
                    ParquetError::External(Box::new(e))
                })?;
                result.bytes().await.map_err(|e| {
                    ParquetError::External(Box::new(e))
                })
            })
            .await
            .map_err(|e| ParquetError::External(Box::new(e)))?;

        self.cached_data = Some(data.clone());
        Ok(data)
    }
}

impl AsyncFileReader for CachingReader {
    fn get_bytes(
        &mut self,
        range: Range<u64>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Bytes>> {
        Box::pin(async move {
            if self.file_cache.should_cache(self.file_size) {
                let data = self.load_file_data().await?;
                Ok(data.slice(range.start as usize..range.end as usize))
            } else {
                // Large file - create temporary reader for pass-through
                debug!("file_cache_skip file={} file_size={}", self.filename, self.file_size);
                let reader = ParquetObjectReader::new(
                    Arc::clone(&self.object_store),
                    self.path.clone(),
                );
                // ParquetObjectReader doesn't implement get_bytes directly in a way we can call,
                // so we use object_store directly for the range read
                let opts = object_store::GetOptions::default();
                let get_range = object_store::GetRange::Bounded(range.clone());
                let result = self.object_store
                    .get_opts(&self.path, opts.with_range(get_range))
                    .await
                    .map_err(|e| ParquetError::External(Box::new(e)))?;
                result.bytes().await.map_err(|e| ParquetError::External(Box::new(e)))
            }
        })
    }

    fn get_byte_ranges(
        &mut self,
        ranges: Vec<Range<u64>>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Vec<Bytes>>> {
        Box::pin(async move {
            if self.file_cache.should_cache(self.file_size) {
                let data = self.load_file_data().await?;
                Ok(ranges
                    .into_iter()
                    .map(|r| data.slice(r.start as usize..r.end as usize))
                    .collect())
            } else {
                // Large file - use object_store's get_ranges for efficient multi-range fetch
                debug!("file_cache_skip file={} file_size={}", self.filename, self.file_size);
                let results = self.object_store
                    .get_ranges(&self.path, &ranges)
                    .await
                    .map_err(|e| ParquetError::External(Box::new(e)))?;
                Ok(results)
            }
        })
    }

    fn get_metadata(
        &mut self,
        _options: Option<&ArrowReaderOptions>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Arc<ParquetMetaData>>> {
        // Delegate to ParquetReader layer which handles metadata caching
        // This method should not be called on CachingReader directly
        Box::pin(async move {
            Err(ParquetError::General(
                "get_metadata should be handled by ParquetReader layer".to_string(),
            ))
        })
    }
}
```

### 3. Update ReaderFactory

Compose the reader layers:

```rust
pub struct ReaderFactory {
    object_store: Arc<dyn ObjectStore>,
    pool: PgPool,
    metadata_cache: Arc<MetadataCache>,
    file_cache: Arc<FileCache>,  // NEW
}

impl ParquetFileReaderFactory for ReaderFactory {
    fn create_reader(
        &self,
        _partition_index: usize,
        partitioned_file: PartitionedFile,
        _metadata_size_hint: Option<usize>,
        _metrics: &ExecutionPlanMetricsSet,
    ) -> datafusion::error::Result<Box<dyn AsyncFileReader + Send>> {
        let path = partitioned_file.path().clone();
        let filename = path.to_string();
        let file_size = partitioned_file.object_meta.size as u64;

        // Layer 1: Caching wrapper (handles file content caching with thundering herd protection)
        let caching_reader = CachingReader::new(
            Arc::clone(&self.object_store),
            path,
            filename.clone(),
            file_size,
            Arc::clone(&self.file_cache),
        );

        // Layer 2: Metadata-aware reader
        Ok(Box::new(ParquetReader {
            filename,
            file_size,
            pool: self.pool.clone(),
            metadata_cache: Arc::clone(&self.metadata_cache),
            inner: caching_reader,
        }))
    }
}
```

`ParquetReader` now wraps `CachingReader` instead of `ParquetObjectReader`:

```rust
pub struct ParquetReader {
    pub filename: String,
    pub file_size: u64,
    pub pool: PgPool,
    pub metadata_cache: Arc<MetadataCache>,
    pub inner: CachingReader,  // Changed from ParquetObjectReader
}
```

### 4. Configuration

Environment variables (following existing pattern):

| Variable | Default | Description |
|----------|---------|-------------|
| `MICROMEGAS_FILE_CACHE_MB` | `200` | Total cache size in MB |
| `MICROMEGAS_FILE_CACHE_MAX_FILE_MB` | `10` | Max file size to cache in MB |

> **Note**: Real-world analysis shows 200MB achieves 75% hit rate. For high-throughput deployments, consider 500MB for maximum 79% hit rate.

### 5. Metrics

Metrics using `imetric!` macro (mirrors MetadataCache pattern):

- `file_cache_eviction_delay` - Time entries spent in cache before eviction (ticks)
- `file_cache_entry_count` - Current number of cached files (emitted on insert)

Debug logs for observability:

- `file_cache_load file={path} file_size={size}` - Cache miss, file being loaded from object store
- `file_cache_skip file={path} file_size={size}` - Large file bypassed cache

> **Note**: Cache hits are silent (no log). With thundering herd protection, a single `file_cache_load` log may serve multiple concurrent requests.

## Verification

### Build & Lint
```bash
cd rust
cargo build
cargo clippy --workspace -- -D warnings
cargo test
```

### Manual Testing
1. Start services: `python3 local_test_env/ai_scripts/start_services.py`
2. Run some queries to populate cache
3. Check cache metrics in logs:
   ```sql
   SELECT time, msg FROM log_entries
   WHERE msg LIKE 'file_cache%'
   ORDER BY time DESC LIMIT 50;
   ```
4. Verify reduced `object_storage_read` entries for repeated queries

### Performance Validation
Compare before/after for same query sequence:
- Count of `object_storage_read` log entries
- Total bytes transferred
- Query latency (if measurable)
