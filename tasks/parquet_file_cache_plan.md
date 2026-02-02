# Plan: Parquet File Cache Implementation

## Status: IMPLEMENTED

All components have been implemented and tested:
- `file_cache.rs` - FileCache with moka-based LRU cache and thundering herd protection
- `caching_reader.rs` - CachingReader wrapper with two-level caching strategy
- `reader_factory.rs` - Updated to use CachingReader
- `lakehouse_context.rs` - Updated to create and manage FileCache
- `mod.rs` - Module exports added
- `file_cache_tests.rs` - 11 unit tests passing

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

### Files Created/Modified

| File | Action | Status |
|------|--------|--------|
| `rust/analytics/src/lakehouse/file_cache.rs` | **Create** - Cache storage (moka-based) | DONE |
| `rust/analytics/src/lakehouse/caching_reader.rs` | **Create** - `CachingReader` wrapper with inherent async methods | DONE |
| `rust/analytics/src/lakehouse/mod.rs` | **Modify** - Add module exports | DONE |
| `rust/analytics/src/lakehouse/reader_factory.rs` | **Modify** - Compose reader layers | DONE |
| `rust/analytics/src/lakehouse/lakehouse_context.rs` | **Modify** - Create and manage FileCache | DONE |
| `rust/analytics/tests/file_cache_tests.rs` | **Create** - Unit tests | DONE |

### 1. Create FileCache (`file_cache.rs`) - DONE

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

### 2. Create CachingReader (`caching_reader.rs`) - DONE

A wrapper that adds file content caching, used internally by `ParquetReader`.

**Implementation note:** Uses inherent async methods (`get_bytes`, `get_byte_ranges`) rather than implementing `AsyncFileReader` trait. This is simpler and sufficient since `ParquetReader` delegates to these methods directly. Also added `run_pending_tasks()` for test scenarios where cache stats need to be immediately consistent:

```rust
use bytes::Bytes;
use datafusion::parquet::errors::ParquetError;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::ops::Range;
use std::sync::Arc;

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

impl CachingReader {
    pub async fn get_bytes(
        &mut self,
        range: Range<u64>,
    ) -> datafusion::parquet::errors::Result<Bytes> {
        if self.file_cache.should_cache(self.file_size) {
            let data = self.load_file_data().await?;
            Ok(data.slice(range.start as usize..range.end as usize))
        } else {
            // Large file - read directly from object store (bypass cache)
            debug!("file_cache_skip file={} file_size={}", self.filename, self.file_size);
            let opts = object_store::GetOptions::default();
            let get_range = object_store::GetRange::Bounded(range.clone());
            let result = self.object_store
                .get_opts(&self.path, opts.with_range(get_range))
                .await
                .map_err(|e| ParquetError::External(Box::new(e)))?;
            result.bytes().await.map_err(|e| ParquetError::External(Box::new(e)))
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
            debug!("file_cache_skip file={} file_size={}", self.filename, self.file_size);
            let results = self.object_store
                .get_ranges(&self.path, &ranges)
                .await
                .map_err(|e| ParquetError::External(Box::new(e)))?;
            Ok(results)
        }
    }
}
```

### 3. Update ReaderFactory - DONE

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

The existing `ParquetReader` impl requires no changes - calls like `inner.get_bytes(range).await`
work identically whether `inner` implements `AsyncFileReader` or has inherent async methods.

### 4. Configuration - DONE

Environment variables (following existing pattern):

| Variable | Default | Description |
|----------|---------|-------------|
| `MICROMEGAS_FILE_CACHE_MB` | `200` | Total cache size in MB |
| `MICROMEGAS_FILE_CACHE_MAX_FILE_MB` | `10` | Max file size to cache in MB |

> **Note**: Real-world analysis shows 200MB achieves 75% hit rate. For high-throughput deployments, consider 500MB for maximum 79% hit rate.

### 5. Metrics - DONE

Metrics using `imetric!` macro (mirrors MetadataCache pattern):

- `file_cache_eviction_delay` - Time entries spent in cache before eviction (ticks)
- `file_cache_entry_count` - Current number of cached files (emitted on insert)

Debug logs for observability:

- `file_cache_load file={path} file_size={size}` - Cache miss, file being loaded from object store
- `file_cache_skip file={path} file_size={size}` - Large file bypassed cache

> **Note**: Cache hits are silent (no log). With thundering herd protection, a single `file_cache_load` log may serve multiple concurrent requests.

### 6. Unit Tests - DONE

Created `rust/analytics/tests/file_cache_tests.rs` (per project convention: tests under `tests/` folder).

**11 tests implemented:**
- `test_should_cache_threshold` - Validates size threshold logic
- `test_cache_hit_skips_loader` - Verifies cache hits don't re-fetch
- `test_different_keys_both_load` - Different files both load
- `test_loader_error_propagation` - Errors propagate correctly
- `test_stats_accuracy` - Cache statistics are accurate
- `test_thundering_herd_single_load` - Concurrent requests coalesce
- `test_get_bytes_returns_correct_range` - Range slicing works
- `test_get_byte_ranges_multiple` - Multi-range reads work
- `test_large_file_bypasses_cache` - Large files bypass cache
- `test_cached_read_populates_cache` - Reads populate cache
- `test_multiple_readers_share_cache` - Readers share global cache

#### FileCache Tests

```rust
use bytes::Bytes;
use micromegas_analytics::lakehouse::file_cache::FileCache;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[tokio::test]
async fn test_should_cache_threshold() {
    let cache = FileCache::new(100 * 1024, 10 * 1024); // 100KB cache, 10KB max file

    assert!(cache.should_cache(10 * 1024));      // exactly at threshold
    assert!(cache.should_cache(1024));           // below threshold
    assert!(!cache.should_cache(10 * 1024 + 1)); // above threshold
}

#[tokio::test]
async fn test_cache_hit_skips_loader() {
    let cache = FileCache::new(1024 * 1024, 100 * 1024);
    let load_count = Arc::new(AtomicUsize::new(0));

    let data = Bytes::from_static(b"test data");

    // First load
    let load_count_clone = Arc::clone(&load_count);
    let data_clone = data.clone();
    let result = cache
        .get_or_load("file1", 9, move || {
            load_count_clone.fetch_add(1, Ordering::SeqCst);
            let d = data_clone.clone();
            async move { Ok::<_, std::io::Error>(d) }
        })
        .await
        .unwrap();
    assert_eq!(result, data);
    assert_eq!(load_count.load(Ordering::SeqCst), 1);

    // Second load - should hit cache
    let load_count_clone = Arc::clone(&load_count);
    let result = cache
        .get_or_load("file1", 9, move || {
            load_count_clone.fetch_add(1, Ordering::SeqCst);
            async move { Ok::<_, std::io::Error>(Bytes::new()) }
        })
        .await
        .unwrap();
    assert_eq!(result, data);
    assert_eq!(load_count.load(Ordering::SeqCst), 1); // loader not called again
}

#[tokio::test]
async fn test_different_keys_both_load() {
    let cache = FileCache::new(1024 * 1024, 100 * 1024);
    let load_count = Arc::new(AtomicUsize::new(0));

    for key in ["file1", "file2"] {
        let load_count_clone = Arc::clone(&load_count);
        cache
            .get_or_load(key, 5, move || {
                load_count_clone.fetch_add(1, Ordering::SeqCst);
                async move { Ok::<_, std::io::Error>(Bytes::from_static(b"data")) }
            })
            .await
            .unwrap();
    }

    assert_eq!(load_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_loader_error_propagation() {
    let cache = FileCache::new(1024 * 1024, 100 * 1024);

    let result: Result<Bytes, _> = cache
        .get_or_load("file1", 5, || async {
            Err::<Bytes, _>(std::io::Error::new(std::io::ErrorKind::NotFound, "not found"))
        })
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_stats_accuracy() {
    let cache = FileCache::new(1024 * 1024, 100 * 1024);

    assert_eq!(cache.stats(), (0, 0));

    cache
        .get_or_load("file1", 100, || async {
            Ok::<_, std::io::Error>(Bytes::from(vec![0u8; 100]))
        })
        .await
        .unwrap();

    // Note: moka may need sync() for immediate stats accuracy
    let (count, size) = cache.stats();
    assert_eq!(count, 1);
    assert_eq!(size, 100);
}

#[tokio::test]
async fn test_thundering_herd_single_load() {
    let cache = Arc::new(FileCache::new(1024 * 1024, 100 * 1024));
    let load_count = Arc::new(AtomicUsize::new(0));

    // Spawn 10 concurrent requests for the same key
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let load_count = Arc::clone(&load_count);
            tokio::spawn(async move {
                cache
                    .get_or_load("same_key", 5, || {
                        let lc = Arc::clone(&load_count);
                        async move {
                            lc.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                            Ok::<_, std::io::Error>(Bytes::from_static(b"data"))
                        }
                    })
                    .await
            })
        })
        .collect();

    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // With thundering herd protection, loader should be called exactly once
    assert_eq!(load_count.load(Ordering::SeqCst), 1);
}
```

#### CachingReader Tests

```rust
use bytes::Bytes;
use micromegas_analytics::lakehouse::caching_reader::CachingReader;
use micromegas_analytics::lakehouse::file_cache::FileCache;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::ObjectStore;
use std::sync::Arc;

async fn setup_test_store() -> (Arc<InMemory>, Path, Bytes) {
    let store = Arc::new(InMemory::new());
    let path = Path::from("test/file.parquet");
    let data = Bytes::from(vec![0u8; 1000]); // 1KB test file
    store.put(&path, data.clone().into()).await.unwrap();
    (store, path, data)
}

#[tokio::test]
async fn test_get_bytes_returns_correct_range() {
    let (store, path, data) = setup_test_store().await;
    let cache = Arc::new(FileCache::new(1024 * 1024, 100 * 1024));

    let mut reader = CachingReader::new(
        store,
        path.clone(),
        path.to_string(),
        1000,
        cache,
    );

    let result = reader.get_bytes(100..200).await.unwrap();
    assert_eq!(result, data.slice(100..200));
}

#[tokio::test]
async fn test_get_byte_ranges_multiple() {
    let (store, path, data) = setup_test_store().await;
    let cache = Arc::new(FileCache::new(1024 * 1024, 100 * 1024));

    let mut reader = CachingReader::new(
        store,
        path.clone(),
        path.to_string(),
        1000,
        cache,
    );

    let ranges = vec![0..100, 500..600, 900..1000];
    let results = reader.get_byte_ranges(ranges.clone()).await.unwrap();

    assert_eq!(results.len(), 3);
    for (result, range) in results.iter().zip(ranges.iter()) {
        assert_eq!(*result, data.slice(range.clone()));
    }
}

#[tokio::test]
async fn test_large_file_bypasses_cache() {
    let store = Arc::new(InMemory::new());
    let path = Path::from("test/large.parquet");
    let large_data = Bytes::from(vec![0u8; 20 * 1024 * 1024]); // 20MB
    store.put(&path, large_data.clone().into()).await.unwrap();

    let cache = Arc::new(FileCache::new(1024 * 1024, 10 * 1024 * 1024)); // 10MB threshold

    let mut reader = CachingReader::new(
        store,
        path.clone(),
        path.to_string(),
        20 * 1024 * 1024,
        cache.clone(),
    );

    // Read should succeed
    let result = reader.get_bytes(0..1000).await.unwrap();
    assert_eq!(result.len(), 1000);

    // Cache should remain empty (file too large)
    assert_eq!(cache.stats().0, 0);
}
```

## Future Improvements

### Rename `object_storage_read` log message - DONE

The `object_storage_read` debug log in `ParquetReader` was misleading - it logged every read request including cache hits.

**Implemented:** Option 2 - renamed to `parquet_read` with `cache_hit=true/false` field:
- `parquet_read file=... file_size=... bytes=... cache_hit=true duration_ms=0`
- `parquet_read file=... file_size=... bytes=... cache_hit=false duration_ms=25`

Implementation:
- Added `last_read_was_cache_hit` field to `CachingReader`
- Uses `AtomicBool` to track whether the loader closure was called (cache miss) or not (cache hit)
- `ParquetReader` reads this flag after each operation and includes it in the log

## Verification

### Build & Lint (PASSED)
```bash
cd rust
cargo build                                    # OK
cargo clippy -p micromegas-analytics -- -D warnings  # OK
cargo test -p micromegas-analytics             # 130+ tests passing
cargo test -p micromegas-analytics --test file_cache_tests  # 11 tests passing
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
