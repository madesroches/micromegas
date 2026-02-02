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
            └── CachingReader (file content caching)
                    └── ParquetObjectReader (object store I/O)
```

**Rationale:**
- Separation of concerns - each layer has single responsibility
- Testability - cache layer testable with mock readers
- Composability - can add other layers (retries, metrics, rate limiting)
- No Mutex needed - `&mut self` on `AsyncFileReader` methods already guarantees exclusive access
- Concrete types avoid dynamic dispatch overhead and simplify the code

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
/// Memory budget is based on file size.
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

    /// Gets cached file contents for the given file path, if present.
    pub async fn get(&self, file_path: &str) -> Option<Bytes> {
        self.cache.get(file_path).await.map(|e| e.data.clone())
    }

    /// Inserts file contents into the cache.
    pub async fn insert(&self, file_path: String, data: Bytes, file_size: u64) {
        if !self.should_cache(file_size) {
            return;
        }
        self.cache
            .insert(
                file_path,
                CacheEntry {
                    data,
                    file_size: file_size as u32,
                    inserted_at: now(),
                },
            )
            .await;
        imetric!("file_cache_entry_count", "count", self.cache.entry_count());
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
    file::metadata::ParquetMetaData,
};
use futures::future::BoxFuture;
use micromegas_tracing::prelude::*;
use std::ops::Range;
use std::sync::Arc;

use super::file_cache::FileCache;

/// Wrapper that adds file content caching to a ParquetObjectReader.
/// No Mutex needed - &mut self already guarantees exclusive access.
pub struct CachingReader {
    inner: ParquetObjectReader,
    filename: String,
    file_size: u64,
    file_cache: Arc<FileCache>,
    /// Local cache of file data for this reader instance
    cached_data: Option<Bytes>,
}

impl CachingReader {
    pub fn new(
        inner: ParquetObjectReader,
        filename: String,
        file_size: u64,
        file_cache: Arc<FileCache>,
    ) -> Self {
        Self {
            inner,
            filename,
            file_size,
            file_cache,
            cached_data: None,
        }
    }
}

impl AsyncFileReader for CachingReader {
    fn get_bytes(
        &mut self,
        range: Range<u64>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Bytes>> {
        Box::pin(async move {
            if self.file_cache.should_cache(self.file_size) {
                // Check local cache first (avoids global cache lookup)
                if let Some(data) = &self.cached_data {
                    return Ok(data.slice(range.start as usize..range.end as usize));
                }

                // Check global cache
                if let Some(data) = self.file_cache.get(&self.filename).await {
                    debug!("file_cache_hit file={}", self.filename);
                    self.cached_data = Some(data.clone());
                    return Ok(data.slice(range.start as usize..range.end as usize));
                }

                // Cache miss - fetch whole file
                let full_data = self.inner.get_bytes(0..self.file_size).await?;
                debug!("file_cache_miss file={} file_size={}", self.filename, self.file_size);
                self.file_cache
                    .insert(self.filename.clone(), full_data.clone(), self.file_size)
                    .await;
                self.cached_data = Some(full_data.clone());
                Ok(full_data.slice(range.start as usize..range.end as usize))
            } else {
                // Large file - pass through to inner reader
                debug!("file_cache_skip file={} file_size={}", self.filename, self.file_size);
                self.inner.get_bytes(range).await
            }
        })
    }

    fn get_byte_ranges(
        &mut self,
        ranges: Vec<Range<u64>>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Vec<Bytes>>> {
        Box::pin(async move {
            if self.file_cache.should_cache(self.file_size) {
                // Check local cache first
                if let Some(data) = &self.cached_data {
                    return Ok(ranges
                        .into_iter()
                        .map(|r| data.slice(r.start as usize..r.end as usize))
                        .collect());
                }

                // Check global cache
                if let Some(data) = self.file_cache.get(&self.filename).await {
                    debug!("file_cache_hit file={}", self.filename);
                    self.cached_data = Some(data.clone());
                    return Ok(ranges
                        .into_iter()
                        .map(|r| data.slice(r.start as usize..r.end as usize))
                        .collect());
                }

                // Cache miss - fetch whole file
                let full_data = self.inner.get_bytes(0..self.file_size).await?;
                debug!("file_cache_miss file={} file_size={}", self.filename, self.file_size);
                self.file_cache
                    .insert(self.filename.clone(), full_data.clone(), self.file_size)
                    .await;
                self.cached_data = Some(full_data.clone());
                Ok(ranges
                    .into_iter()
                    .map(|r| full_data.slice(r.start as usize..r.end as usize))
                    .collect())
            } else {
                self.inner.get_byte_ranges(ranges).await
            }
        })
    }

    fn get_metadata(
        &mut self,
        options: Option<&ArrowReaderOptions>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Arc<ParquetMetaData>>> {
        // Delegate to inner - metadata caching handled separately by ParquetReader
        self.inner.get_metadata(options)
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
        metadata_size_hint: Option<usize>,
        _metrics: &ExecutionPlanMetricsSet,
    ) -> datafusion::error::Result<Box<dyn AsyncFileReader + Send>> {
        let filename = partitioned_file.path().to_string();
        let file_size = partitioned_file.object_meta.size;

        // Layer 1: Object store reader (with footer hint for large file optimization)
        let mut object_reader = ParquetObjectReader::new(
            Arc::clone(&self.object_store),
            partitioned_file.path().clone(),
        );
        if let Some(hint) = metadata_size_hint {
            object_reader = object_reader.with_footer_size_hint(hint);
        }

        // Layer 2: Caching wrapper
        let caching_reader = CachingReader::new(
            object_reader,
            filename.clone(),
            file_size,
            Arc::clone(&self.file_cache),
        );

        // Layer 3: Metadata-aware reader
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

- `file_cache_hit file={path}` - Cache hit
- `file_cache_miss file={path} file_size={size}` - Cache miss, file loaded
- `file_cache_skip file={path} file_size={size}` - Large file bypassed cache

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
