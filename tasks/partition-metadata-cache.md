# Partition Metadata Cache

## Problem

The same partition metadata is being loaded repeatedly from the database. Analysis of a flight-sql-srv session showed:

| Metric | Value |
|--------|-------|
| Queries executed | 67 |
| Metadata fetches | 10,908 |
| Unique partitions | 1,206 |
| Most loaded file | 156 times |
| Total bytes fetched | 66 MB |
| Unique bytes needed | 6.6 MB |

Each query creates new `ParquetReader` instances via `ReaderFactory::create_reader()`. Each reader has its own local cache that starts empty, so the same metadata is fetched from the database repeatedly across queries.

## Solution

Add a global LRU cache in `ReaderFactory` using `moka` with a configurable memory budget. The cache will be shared across all readers and queries within a service instance.

### Memory Budget

- Average serialized metadata size: ~5.5 KB
- In-memory size: ~3x serialized (~16 KB)
- Total partitions in system: ~6,000
- Full cache memory: ~100 MB

A 50 MB budget would cache most hot partitions and provide >90% hit rate for typical workloads.

## Implementation Plan

### 1. Create single ReaderFactory at service startup

**Key architectural change:** Currently `make_partitioned_execution_plan` creates a new `ReaderFactory` per query. Instead, create one `ReaderFactory` at service startup and pass `Arc<ReaderFactory>` through the query path.

In `flight-sql-srv`, create once at startup:

```rust
let metadata_cache = Arc::new(MetadataCache::new(
    std::env::var("MICROMEGAS_METADATA_CACHE_MB")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(50) * 1024 * 1024
));

let reader_factory = Arc::new(ReaderFactory::new(
    object_store.clone(),
    pool.clone(),
    metadata_cache,
));
```

### 2. Thread ReaderFactory through query path

Replace `object_store: Arc<dyn ObjectStore>` + `pool: PgPool` parameters with `reader_factory: Arc<ReaderFactory>`:

**partitioned_execution_plan.rs:**
```rust
pub fn make_partitioned_execution_plan(
    schema: SchemaRef,
    reader_factory: Arc<ReaderFactory>,  // replaces object_store + pool
    state: &dyn Session,
    projection: Option<&Vec<usize>>,
    filters: &[Expr],
    limit: Option<usize>,
    partitions: Arc<Vec<Partition>>,
) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
    // ...
    let source = Arc::new(
        ParquetSource::default()
            .with_predicate(predicate)
            .with_parquet_file_reader_factory(reader_factory),  // use directly
    );
    // ...
}
```

**partitioned_table_provider.rs:**
```rust
pub struct PartitionedTableProvider {
    schema: SchemaRef,
    reader_factory: Arc<ReaderFactory>,  // replaces object_store + pool
    partitions: Arc<Vec<Partition>>,
}
```

**materialized_view.rs** - similar changes

**query.rs** - pass reader_factory through `query_lakehouse()` and related functions

### 3. Add moka dependency

In `rust/analytics/Cargo.toml`:

```toml
moka = { version = "0.12", features = ["future"] }
```

### 4. Create MetadataCache struct

New file `rust/analytics/src/lakehouse/metadata_cache.rs`:

```rust
use datafusion::parquet::file::metadata::ParquetMetaData;
use moka::future::Cache;
use std::sync::Arc;

/// Cache entry storing metadata and its serialized size for weight calculation
struct CacheEntry {
    metadata: Arc<ParquetMetaData>,
    serialized_size: u32,
}

pub struct MetadataCache {
    cache: Cache<String, CacheEntry>,
}

impl MetadataCache {
    pub fn new(max_capacity_bytes: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity_bytes)
            .weigher(|_key: &String, entry: &CacheEntry| -> u32 {
                // Use serialized size * 3 as estimate for in-memory size
                entry.serialized_size.saturating_mul(3)
            })
            .build();
        Self { cache }
    }

    pub async fn get(&self, file_path: &str) -> Option<Arc<ParquetMetaData>> {
        self.cache.get(file_path).await.map(|e| e.metadata.clone())
    }

    pub async fn insert(
        &self,
        file_path: String,
        metadata: Arc<ParquetMetaData>,
        serialized_size: u32,
    ) {
        self.cache
            .insert(file_path, CacheEntry { metadata, serialized_size })
            .await;
    }
}
```

### 5. Modify ReaderFactory

Add cache to `ReaderFactory` and pass through to readers:

```rust
pub struct ReaderFactory {
    object_store: Arc<dyn ObjectStore>,
    pool: PgPool,
    metadata_cache: Arc<MetadataCache>,
}

impl ReaderFactory {
    pub fn new(
        object_store: Arc<dyn ObjectStore>,
        pool: PgPool,
        metadata_cache: Arc<MetadataCache>,
    ) -> Self {
        Self { object_store, pool, metadata_cache }
    }
}
```

Update `ParquetReader` to use the shared cache (remove per-reader cache):

```rust
pub struct ParquetReader {
    pub filename: String,
    pub pool: PgPool,
    pub metadata_cache: Arc<MetadataCache>,
    pub inner: ParquetObjectReader,
}
```

### 6. Modify partition_metadata.rs

Update `load_partition_metadata` to accept an optional cache:

```rust
#[span_fn]
pub async fn load_partition_metadata(
    pool: &PgPool,
    file_path: &str,
    cache: Option<&MetadataCache>,
) -> Result<Arc<ParquetMetaData>> {
    // Check cache first
    if let Some(cache) = cache {
        if let Some(metadata) = cache.get(file_path).await {
            debug!("cache hit for partition metadata path={file_path}");
            return Ok(metadata);
        }
    }

    // Existing database fetch logic...
    let metadata_bytes: Vec<u8> = row.try_get("metadata")?;
    let serialized_size = metadata_bytes.len() as u32;

    debug!("fetched partition metadata path={file_path} size={serialized_size}");

    // ... existing parsing logic ...

    let result = Arc::new(stripped);

    // Store in cache
    if let Some(cache) = cache {
        cache.insert(file_path.to_string(), result.clone(), serialized_size).await;
    }

    Ok(result)
}
```

### 7. Add cache metrics logging

Add periodic stats logging or expose via metrics:

```rust
impl MetadataCache {
    pub fn stats(&self) -> (u64, u64) {
        (self.cache.entry_count(), self.cache.weighted_size())
    }
}
```

## Files to Modify

- `rust/analytics/Cargo.toml` - add moka dependency
- `rust/analytics/src/lakehouse/mod.rs` - add metadata_cache module
- `rust/analytics/src/lakehouse/metadata_cache.rs` - new file
- `rust/analytics/src/lakehouse/partition_metadata.rs` - add cache parameter
- `rust/analytics/src/lakehouse/reader_factory.rs` - add cache field, remove per-reader cache
- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` - take `Arc<ReaderFactory>` instead of object_store + pool
- `rust/analytics/src/lakehouse/partitioned_table_provider.rs` - store `Arc<ReaderFactory>` instead of object_store + pool
- `rust/analytics/src/lakehouse/materialized_view.rs` - pass reader_factory
- `rust/analytics/src/lakehouse/query.rs` - create ReaderFactory once, pass through query chain
- `rust/flight-sql-srv/src/main.rs` - instantiate ReaderFactory at startup

## Configuration

New environment variable:

| Variable | Default | Description |
|----------|---------|-------------|
| `MICROMEGAS_METADATA_CACHE_MB` | 50 | Max memory for partition metadata cache |

## Testing

1. Run services and execute repeated queries
2. Check logs for "cache hit" vs "fetched partition metadata" messages
3. Verify cache reduces database fetches by >90%
4. Monitor memory usage stays within budget
5. Run existing tests to verify no regressions

## Expected Results

| Metric | Before | After |
|--------|--------|-------|
| DB fetches per repeated query | ~160 | ~0-5 (new partitions only) |
| Metadata load latency | ~2-3ms per file | <0.1ms cache hit |
| Memory overhead | 0 | ~50 MB (configurable) |
