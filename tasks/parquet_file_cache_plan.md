# Plan: Parquet File Cache Implementation

## Goal
Implement an in-memory cache for parquet file contents to reduce object storage reads and improve query performance.

## Analysis Summary (24h local dev data)

> **Note**: Data collected on dev laptop with local SSD storage. Production environments using remote object storage (S3/GCS) will have significantly higher latency, making the cache impact more substantial.

- **15,343 reads** of **1,618 unique files** (9.48 reads/file avg)
- **89.5% potential cache hit rate** (files read multiple times)
- **2.93x read amplification** (44 MB transferred for 15 MB unique data)
- **97.8% of reads complete in 0ms** (local SSD + OS cache; remote storage would be 10-100ms+)
- **Cache size needed**: 15 MB (all files) or 3.3 MB (top 20% = 71% of reads)

## Design Decisions

### Whole Files vs Segments

**Recommendation: Cache whole files** with a size threshold.

**Rationale:**
1. Files are small (avg ~10KB, most under 100KB)
2. Multiple byte ranges of the same file are read per query
3. Simpler implementation, follows existing MetadataCache pattern
4. moka's weight-based eviction handles memory efficiently

### Handling Large Files

**Approach: Skip caching files above threshold (default 10MB)**

- Files > threshold: read directly from object store (pass-through)
- Files <= threshold: cache entire file on first read
- Threshold configurable via environment variable

**Future enhancement**: Segment-based caching for large files if needed.

### Layered Architecture

**Use `Arc<dyn AsyncFileReader>` for composable reader layers.**

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
- No generics - `Arc<dyn>` is simpler, perf difference negligible for I/O-bound work

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

/// Entry stored in the file cache
struct CacheEntry {
    data: Bytes,
    file_size: u64,
    inserted_at: i64,
}

/// In-memory cache for parquet file contents
pub struct FileCache {
    cache: Cache<String, Arc<CacheEntry>>,
    max_file_size: u64,
}

impl FileCache {
    /// Create a new FileCache with the given capacity in bytes
    pub fn new(max_capacity_bytes: u64, max_file_size_bytes: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity_bytes)
            .weigher(|_key: &String, entry: &Arc<CacheEntry>| -> u32 {
                entry.file_size.min(u32::MAX as u64) as u32
            })
            .eviction_listener(|_key: Arc<String>, entry: Arc<CacheEntry>, cause: RemovalCause| {
                if cause == RemovalCause::Size {
                    let eviction_delay = micromegas_tracing::now() - entry.inserted_at;
                    imetric!("file_cache_eviction_delay", "ticks", eviction_delay as u64);
                }
            })
            .build();

        Self { cache, max_file_size: max_file_size_bytes }
    }

    /// Check if a file should be cached based on its size
    pub fn should_cache(&self, file_size: u64) -> bool {
        file_size <= self.max_file_size
    }

    /// Get cached file contents
    pub async fn get(&self, file_path: &str) -> Option<Bytes> {
        self.cache.get(file_path).await.map(|e| e.data.clone())
    }

    /// Insert file contents into cache
    pub async fn insert(&self, file_path: String, data: Bytes, file_size: u64) {
        if !self.should_cache(file_size) {
            return;
        }
        let entry = Arc::new(CacheEntry {
            data,
            file_size,
            inserted_at: micromegas_tracing::now(),
        });
        self.cache.insert(file_path, entry).await;
    }

    /// Get cache statistics (entry_count, weighted_size_bytes)
    pub fn stats(&self) -> (u64, u64) {
        (self.cache.entry_count(), self.cache.weighted_size())
    }
}
```

### 2. Create CachingReader (`caching_reader.rs`)

A wrapper that adds caching to any `AsyncFileReader`:

```rust
use bytes::Bytes;
use datafusion::parquet::arrow::async_reader::AsyncFileReader;
use futures::future::BoxFuture;
use micromegas_tracing::prelude::*;
use std::ops::Range;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::file_cache::FileCache;

/// Wrapper that adds file content caching to any AsyncFileReader.
/// Uses Arc<dyn> for simple composition without generic type pollution.
pub struct CachingReader {
    inner: Arc<Mutex<Box<dyn AsyncFileReader + Send>>>,
    filename: String,
    file_size: u64,
    file_cache: Arc<FileCache>,
    /// Local cache of file data for this reader instance
    cached_data: Option<Bytes>,
}

impl CachingReader {
    pub fn new(
        inner: Box<dyn AsyncFileReader + Send>,
        filename: String,
        file_size: u64,
        file_cache: Arc<FileCache>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
            filename,
            file_size,
            file_cache,
            cached_data: None,
        }
    }

    /// Load entire file into cache, or retrieve from cache if present
    async fn ensure_cached(&mut self) -> datafusion::parquet::errors::Result<Bytes> {
        // Already have local copy
        if let Some(data) = &self.cached_data {
            return Ok(data.clone());
        }

        // Check global cache
        if let Some(data) = self.file_cache.get(&self.filename).await {
            debug!("file_cache_hit file={}", self.filename);
            self.cached_data = Some(data.clone());
            return Ok(data);
        }

        // Load from object store
        let full_data = {
            let mut inner = self.inner.lock().await;
            inner.get_bytes(0..self.file_size).await?
        };

        debug!("file_cache_miss file={} file_size={}", self.filename, self.file_size);
        self.file_cache
            .insert(self.filename.clone(), full_data.clone(), self.file_size)
            .await;
        self.cached_data = Some(full_data.clone());

        Ok(full_data)
    }
}

impl AsyncFileReader for CachingReader {
    fn get_bytes(
        &mut self,
        range: Range<u64>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Bytes>> {
        Box::pin(async move {
            if self.file_cache.should_cache(self.file_size) {
                let data = self.ensure_cached().await?;
                Ok(data.slice(range.start as usize..range.end as usize))
            } else {
                // Large file - pass through to inner reader
                debug!("file_cache_skip file={} file_size={}", self.filename, self.file_size);
                let mut inner = self.inner.lock().await;
                inner.get_bytes(range).await
            }
        })
    }

    fn get_byte_ranges(
        &mut self,
        ranges: Vec<Range<u64>>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Vec<Bytes>>> {
        Box::pin(async move {
            if self.file_cache.should_cache(self.file_size) {
                let data = self.ensure_cached().await?;
                Ok(ranges
                    .into_iter()
                    .map(|r| data.slice(r.start as usize..r.end as usize))
                    .collect())
            } else {
                let mut inner = self.inner.lock().await;
                inner.get_byte_ranges(ranges).await
            }
        })
    }

    fn get_metadata(
        &mut self,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Arc<ParquetMetaData>>> {
        // Delegate to inner - metadata caching handled separately by ParquetReader
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let mut inner = inner.lock().await;
            inner.get_metadata().await
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
    fn create_reader(&self, ...) -> Result<Box<dyn AsyncFileReader + Send>> {
        let filename = partitioned_file.path().to_string();
        let file_size = partitioned_file.object_meta.size;

        // Layer 1: Object store reader
        let object_reader = ParquetObjectReader::new(
            Arc::clone(&self.object_store),
            partitioned_file.path().clone(),
        );

        // Layer 2: Caching wrapper
        let caching_reader = CachingReader::new(
            Box::new(object_reader),
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
| `MICROMEGAS_FILE_CACHE_MB` | `100` | Total cache size in MB |
| `MICROMEGAS_FILE_CACHE_MAX_FILE_MB` | `10` | Max file size to cache in MB |

### 5. Metrics

Add instrumentation using existing `imetric!` macro:

- `file_cache_hit` - Count of cache hits
- `file_cache_miss` - Count of cache misses
- `file_cache_skip` - Count of large files skipped
- `file_cache_eviction_delay` - Time entries spent in cache before eviction
- `file_cache_entry_count` - Current number of cached files
- `file_cache_size_bytes` - Current cache size

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
