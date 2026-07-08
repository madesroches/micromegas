# In-Process L1 Cache Plan (#1205)

Tracking issue: [#1205](https://github.com/madesroches/micromegas/issues/1205) — "object-cache:
in-process L1 cache to eliminate the network hop on hot reads". Follow-up to the range-aware read
cache (#1188) and the read-path rework (#1203/#1216).

## Overview

Add an **in-process L1 cache** for query-side reads, built from the existing `RangeCache` core
(`rust/object-cache/src/range_cache.rs`) with a **bounded RAM backend**. Rather than filtering by
key prefix or file extension, L1 is installed **only on the object stores DataFusion reads through**:
the store handed to the parquet `ReaderFactory` and the static-tables store. Every raw-blob read
(ETL materialization, and the `get_payload`/`parse_block` UDFs) goes through `BlobStorage` on a
different store reference and bypasses L1 automatically — so no path/prefix/suffix filter is needed;
the caller distinction *is* the filter.

The same change **removes the old whole-file `FileCache`** (`file_cache.rs` / `caching_reader.rs`),
which today only caches files ≤10 MB at the parquet-reader layer. L1 subsumes it: it caches hot
row-group bytes for files of all sizes, closing the >10 MB gap that motivates this issue.

Hot reads short-circuit at L1 with zero network; an L1 miss falls through to whatever store it wraps
— which already contains L2 (`CacheClientStore`) when configured, else object storage directly. So
the L1 → L2 → origin tiering the issue describes still holds; L1 just lives at the reader layer
rather than as a standalone `ObjectStore` in `make_cache`.

## Current State

### Object stores and read paths (why caller-based works)

`DataLakeConnection.blob_storage` (`rust/telemetry/src/blob_storage.rs`) wraps one
`Arc<dyn ObjectStore>` stack, built in `connect_to_data_lake`
(`rust/ingestion/src/data_lake_connection.rs:111-131`):

```
BlobStorage.inner() = PrefixStore(root) -> CacheClientStore (L2, if configured) -> raw store (S3/GCS)
```

The lake object store has exactly **two** top-level key prefixes:
- `blobs/{process_id}/{stream_id}/{block_id}` — raw telemetry payloads, **read-once** (parsed into
  parquet during ETL). Written at `web_ingestion_service.rs:155`, read via `payload.rs:25`
  (`read_blob`).
- `views/{view_set}/{view_instance}/{date}/{time}_{file_id}.parquet` — materialized partitions,
  **read-repeatedly** on the query hot path. Written at `write_partition.rs:546-547`.

Two facts (confirmed by tracing) make the caller-based install clean:

1. **Parquet partition scans read exclusively through `ReaderFactory.object_store`.** The scan is
   wired by `ParquetSource::with_parquet_file_reader_factory`
   (`partitioned_execution_plan.rs:56-60`); when a reader factory is set, DataFusion does **not**
   consult `runtime_env().object_store(url)`. `ReaderFactory::new` is called at exactly two sites,
   both `lake.blob_storage.inner()`: `lakehouse_context.rs:106` and `:127`.

2. **Every raw-blob read goes through `BlobStorage`, never through the reader factory store.** ETL
   processors call `fetch_block_payload` → `BlobStorage::read_blob` (`payload.rs:19-38`,
   `blob_storage.rs:72-75`). The `get_payload` UDF (`get_payload_function.rs:110`) and `parse_block`
   UDTF (`parse_block_table_function.rs:301`) also use `BlobStorage`. `BlobStorage.inner()` and
   `read_blob` share the same internal `Arc`, but that Arc is a *different reference* from whatever we
   wrap and hand to `ReaderFactory` — so wrapping the reader-factory store leaves all blob reads
   uncached even when they are triggered from SQL.

**Consequence:** wrap the store *passed to* `ReaderFactory::new` (and the static-tables store); do
**not** wrap inside `BlobStorage` (`connect_with_layer`) or in `make_cache`, which would cache blob
reads too.

### Static tables (separate store, also read-repeatedly)

`StaticTablesConfigurator` (`static_tables_configurator.rs`) discovers `*.json`/`*.csv` under
`MICROMEGAS_STATIC_TABLES_URL` (`public/src/servers/flight_sql_server.rs:206`), parses its **own**
object store (`static_tables_configurator.rs:70-76`), registers it with `ctx.register_object_store`,
and exposes each file as a DataFusion `ListingTable`. Reads are lazy per query through that
registered store (`csv_table_provider.rs:34`, `json_table_provider.rs:96`), so they benefit from
caching and are wrapped the same way.

### The old file cache (to be removed)

- `ReaderFactory` (`reader_factory.rs:29-88`) holds `object_store`, `MetadataCache`, `FileCache`, and
  builds a `ParquetReader` wrapping a `CachingReader` per file.
- `CachingReader` (`caching_reader.rs`) caches whole files ≤10 MB in `FileCache` and slices ranges;
  >10 MB files bypass and read `object_store.get_range`/`get_ranges` directly.
- `FileCache` (`file_cache.rs`) is a moka LRU, default 200 MB / 10 MB max-file, coalescing via
  `try_get_with`. Config: `MICROMEGAS_FILE_CACHE_MB`, `MICROMEGAS_FILE_CACHE_MAX_FILE_MB`
  (`lakehouse_context.rs:17-20,79-103`).
- `load_partition_metadata` (`partition_metadata.rs:102-147`) reads the footer through a
  `CachingReaderFetch` adapter over the same `CachingReader`.

### RangeCache core (reused as-is)

- `RangeCache::new(origin, backend, block_size, ns, total_fetch_permits,
  demand_reserved_fetch_permits, max_coalesced_get_bytes, promote_whole_batch)`
  (`range_cache.rs:533-559`). Block model (default 1 MiB), single-flight, run coalescing, write-once
  keys (no invalidation). `#[derive(Clone)]`.
- Read API: `get_range(key, range)` (`:1141`), `get_ranges(key, ranges)` (`:1175`), `size(key)`
  (`:625`), plus `*_with_size` / `stream_ranges_with_size`.
- Takes a `backend: Arc<dyn RangeCacheBackend>` (`backend.rs`), so **multiple `RangeCache` instances
  can share one backend** (hence one RAM budget) as long as their `ns` differ (ns is part of every
  key).
  - `MemoryBackend` (`memory_backend.rs`) is a plain unbounded `Mutex<HashMap>` — **no eviction**,
    not usable for a budgeted L1.
  - `FoyerBackend` (`foyer_backend.rs`) has a bounded byte-weighted LRU RAM tier but is feature-gated
    and always builds an on-disk device.
- Reference config (`object_cache_srv.rs:153-162`, defaults `range_cache.rs:64-72`):
  `total_fetch_permits = 32`, `demand_reserved = 8`, `max_coalesced_get_bytes = 8 MiB`,
  `promote_whole_batch = false`, block 1 MiB.

## Design

### Target layering

```
DataFusion parquet scan  --> ReaderFactory.object_store = L1CacheStore
                                                            (RangeCache ns="lakehouse",
                                                             origin = blob_storage.inner())
                                                              --> PrefixStore(root) -> L2 -> origin

DataFusion static scan   --> registered store             = L1CacheStore
                                                            (RangeCache ns="static",
                                                             origin = static-tables store)

ETL / get_payload / parse_block --> BlobStorage.read_blob --> (unwrapped) PrefixStore(root) -> L2 -> origin
```

Both `L1CacheStore` instances share **one** `Arc<BoundedMemoryBackend>` (a single RAM budget),
distinguished by `ns`. `make_cache` / `BlobStorage` are unchanged.

### Crate placement (keep `object-cache` untouched)

All new code lives in `analytics`; `object-cache` is not modified. `object-cache` deliberately
dropped its moka dependency in #1203, and `analytics` already depends on moka (`file_cache.rs`,
`metadata_cache.rs`) — so the moka-based backend belongs in `analytics`, not next to the other
backends in `object-cache`. The only `object-cache` surface used is the already-`pub` `RangeCache`
and `RangeCacheBackend` trait. Both wrap sites (`lakehouse_context.rs`, `static_tables_configurator.rs`)
are in `analytics`, so the shared-budget backend is a simple analytics-local global — no cross-crate
threading.

### New component: `L1CacheStore` (an `ObjectStore` over a `RangeCache`)

`RangeCache` is an internal API, not an `ObjectStore`. Add a thin adapter in `analytics` exposing a
RAM-backed `RangeCache` as an `ObjectStore`, so it drops into either wrap site:

```rust
// rust/analytics/src/lakehouse/l1_store.rs (new)
pub struct L1CacheStore {
    cache: RangeCache,             // backend = shared bounded RAM; origin = the wrapped store
    origin: Arc<dyn ObjectStore>,  // for pass-through ops (put/list/delete/head/preconditions)
}
```

`ObjectStore` impl (mirrors `CacheClientStore`'s shape):
- **`get_opts`** — no conditional/version preconditions and not `head`:
  - `GetRange::Bounded(r)` → `cache.get_range(key, r)` returned as a single-chunk streaming
    `GetResult`.
  - full / open-ended / suffix → resolve size via `cache.size(key)` and serve the requested slice
    (suffix computes `start = size - suffix`).
  - preconditions / `head` → delegate to `origin`.
- **`get_ranges`** → `cache.get_ranges(key, ranges)`.
- **`put_*`, `list`, `delete`, `copy`, `head`** → delegate to `origin`. On any cache error, fall back
  to `origin` for the same request (same graceful-degradation contract as `CacheClientStore`).

**No prefix/suffix filter inside `L1CacheStore`** — it caches whatever it is asked for, and it is only
ever handed parquet-partition reads and static-table reads, both cacheable. Full-object reads (CSV/JSON
static tables read whole files) are served by fetching all covering blocks, which is exactly the
caching we want.

### New component: `BoundedMemoryBackend` (shared RAM budget)

`MemoryBackend` (`object-cache`) is unbounded. Rather than hand-roll an LRU, reuse **moka** — the same
`moka::future::Cache` + byte-weigher the current `FileCache` already uses (`file_cache.rs:43-45`), so
this is a small, proven wrapper, not a new eviction algorithm. It lives in `analytics` (which already
depends on moka), implementing the `object-cache` `RangeCacheBackend` trait:

```rust
// rust/analytics/src/lakehouse/bounded_memory_backend.rs (new)
pub struct BoundedMemoryBackend { cache: moka::future::Cache<String, Bytes> } // weigher = |_, v| v.len()
#[async_trait] impl RangeCacheBackend for BoundedMemoryBackend { get; put; } // disk_stats -> None (default)
```

- Byte-weighted `max_capacity`; moka handles eviction and concurrency. Simpler than `FileCache`: the
  backend is pure `get`/`put` (no `try_get_with`) because `RangeCache` already owns single-flight via
  its in-flight map (#1203).
- `FillHint::Prefetch` should not evict hot `Demand` data. L1 is read-driven (no write-warming), so
  prefetch fills are rare here — a v1 may ignore the hint and refine later.
- `disk_stats()` returns `None` (trait default).

### Shared backend across both wrap sites

Both wrap sites are in `analytics`, so a single analytics-local lazily-initialized backend, sized once
from `MICROMEGAS_L1_CACHE_MB`, gives one shared budget with no cross-crate threading:

```rust
// rust/analytics/src/lakehouse/l1_store.rs
fn shared_l1_backend() -> Option<Arc<BoundedMemoryBackend>>; // None when MICROMEGAS_L1_CACHE_MB == 0
pub fn l1_wrap(origin: Arc<dyn ObjectStore>, ns: &str) -> Arc<dyn ObjectStore>; // wraps iff enabled
```

`l1_wrap` returns `origin` unchanged when L1 is disabled. A shared global is safe because keys are
write-once/content-addressed and namespaced per instance, so cross-context reuse (e.g. in tests) can
only ever return identical bytes.

### Wiring

- `lakehouse_context.rs:106` and `:127`: wrap before constructing the factory —
  `L1: ReaderFactory::new(l1_wrap(lake.blob_storage.inner(), "lakehouse"), metadata_cache)`. Drop the
  `file_cache` argument.
- `static_tables_configurator.rs:76`: `let object_store = l1_wrap(Arc::new(object_store), "static");`
  before `register_object_store`.
- `analytics/Cargo.toml`: add a dependency on `micromegas-object-cache` (analytics does not depend on
  it today).

### Removing the old file cache (analytics)

- Delete `file_cache.rs`, `caching_reader.rs`, `tests/file_cache_tests.rs`; drop their `mod` lines in
  `lakehouse/mod.rs`.
- `ReaderFactory` / `ParquetReader`: drop the `file_cache` field; `ParquetReader` holds
  `object_store: Arc<dyn ObjectStore>` + `path` and implements `AsyncFileReader::get_bytes` /
  `get_byte_ranges` via `object_store.get_range` / `get_ranges` (L1 now lives in that store). Keep the
  `parquet_read ... duration_ms` debug log; the per-read `cache_hit` flag goes away (L1 emits its own
  hit/miss metrics — see Testing).
- `load_partition_metadata` (`partition_metadata.rs`): replace `CachingReaderFetch` with a
  `MetadataFetch` reading `object_store.get_range(path, range)`; change the signature from
  `reader: &mut CachingReader` to `object_store: &Arc<dyn ObjectStore>, path: &Path`. Footer reads now
  benefit from L1 too (the #1121 footer tie-in the issue mentions).
- `lakehouse_context.rs`: drop `FileCache` construction and the `MICROMEGAS_FILE_CACHE_MB` /
  `MICROMEGAS_FILE_CACHE_MAX_FILE_MB` knobs.

## Implementation Steps

**Phase 1 — building blocks (all in `analytics`)**
1. Add `micromegas-object-cache` to `analytics/Cargo.toml` (for `RangeCache` + `RangeCacheBackend`).
2. `BoundedMemoryBackend` (`bounded_memory_backend.rs`), moka-based, implementing `RangeCacheBackend`.
   Tests: byte-weighted eviction at budget, get/put round-trip.
3. `L1CacheStore` (`l1_store.rs`) implementing `ObjectStore` over a `RangeCache`, with origin
   pass-through/fallback; plus `shared_l1_backend()` / `l1_wrap(origin, ns)` reading
   `MICROMEGAS_L1_CACHE_MB` (0 = disabled).

**Phase 2 — wiring (analytics)**
4. Wrap the reader-factory store at `lakehouse_context.rs:106` and `:127` via `l1_wrap(..., "lakehouse")`.
5. Wrap the static-tables store at `static_tables_configurator.rs:76` via `l1_wrap(..., "static")`.

**Phase 3 — remove old file cache (analytics)**
7. Refactor `load_partition_metadata` to read via `object_store` + `path`.
8. Refactor `ReaderFactory` / `ParquetReader` to read directly from `object_store`; drop `file_cache`.
9. Delete `file_cache.rs`, `caching_reader.rs`, `file_cache_tests.rs`; update `mod.rs`.
10. Drop `FileCache` + its env knobs from `lakehouse_context.rs`.

**Phase 4 — verify**
11. `cd rust && cargo fmt && cargo clippy --workspace -- -D warnings && cargo test`.
12. Manual: run flight-sql with and without `MICROMEGAS_OBJECT_CACHE_URL`; confirm repeat queries hit
    L1 (metrics), a `get_payload`/`parse_block` query does not populate L1, and a repeat static-table
    query hits L1.

## Files to Modify

**New (all in `analytics`; `object-cache` is untouched)**
- `rust/analytics/src/lakehouse/bounded_memory_backend.rs`
- `rust/analytics/src/lakehouse/l1_store.rs` (`L1CacheStore` + `shared_l1_backend`/`l1_wrap`)
- `rust/analytics/tests/l1_store_tests.rs`

**Modified**
- `rust/analytics/Cargo.toml` (add `micromegas-object-cache`)
- `rust/analytics/src/lakehouse/lakehouse_context.rs`
- `rust/analytics/src/lakehouse/static_tables_configurator.rs`
- `rust/analytics/src/lakehouse/reader_factory.rs`
- `rust/analytics/src/lakehouse/partition_metadata.rs`
- `rust/analytics/src/lakehouse/mod.rs`

**Deleted**
- `rust/analytics/src/lakehouse/file_cache.rs`
- `rust/analytics/src/lakehouse/caching_reader.rs`
- `rust/analytics/tests/file_cache_tests.rs`

## Trade-offs

- **Caller-based install vs. prefix/suffix filter.** Wrapping per object-store instance needs no
  filtering logic and correctly excludes blobs even when read through DataFusion UDFs
  (`get_payload`/`parse_block`), because those go through `BlobStorage`, not the reader-factory store.
  A prefix filter at `make_cache` would need root-aware segment matching and still wouldn't cover
  static tables (a separate store); a `.parquet` suffix filter wouldn't cover static tables either.
  Cost: L1 wiring moves into the analytics layer and `analytics` gains an `object-cache` dependency,
  and it is no longer literally an `ObjectStore` in front of `CacheClientStore` as #1205's text
  describes (functionally the L1→L2→origin tiering is preserved, since L1's origin already contains
  L2).
- **moka-based backend in `analytics` vs. hand-rolled LRU vs. `FoyerBackend` in `object-cache`.** moka
  is the workspace LRU and already backs `FileCache` with a byte-weigher, so the backend is a proven
  wrapper rather than a hand-rolled eviction algorithm. Placing it in `analytics` (which already
  depends on moka) keeps `object-cache` free of the moka dependency #1203 removed, at the cost of the
  backend living apart from `MemoryBackend`/`FoyerBackend`. Foyer's RAM tier was rejected: feature-gated
  and always builds an on-disk device — wrong footprint for a RAM-only L1.
- **Shared analytics-local global backend.** Both wrap sites are in `analytics`, so one lazily-initialized
  global gives a single RAM budget with no cross-crate plumbing; write-once/namespaced keys make the
  shared instance safe (identical bytes for identical keys).
- **Block granularity vs. whole-file.** L1 caches 1 MiB blocks instead of whole ≤10 MB files. For large
  files this is the point (cache only touched row-group blocks); for a cold one-shot small-file read it
  can pull marginally more bytes, but repeat reads are served from RAM.

## Documentation

- Update the admin/object-cache docs (`mkdocs/docs/`, page from #1188) to describe the L1 tier, the
  `MICROMEGAS_L1_CACHE_MB` knob, that it covers parquet partitions **and** static tables, and that it
  excludes raw blobs.
- Note removal of `MICROMEGAS_FILE_CACHE_MB` / `MICROMEGAS_FILE_CACHE_MAX_FILE_MB` wherever service env
  vars are listed.

## Testing Strategy

- **Unit (`object-cache`)**: `BoundedMemoryBackend` eviction/weight/hint; `L1CacheStore` over a counting
  mock origin — repeat reads issue no origin request; ranged/full/suffix reads return correct bytes;
  origin errors fall back; `list`/`put`/`delete` pass through.
- **Analytics integration**: repeat parquet-partition query keeps the origin call count flat while a
  `get_payload`/`parse_block` query does not populate L1 (proves the caller-based split); repeat
  static-table query hits L1.
- **Regression**: existing lakehouse/query tests pass after `FileCache` removal; port still-relevant
  assertions from `file_cache_tests.rs` into the new L1 tests rather than dropping coverage.
- **Telemetry**: confirm L1 hit/miss is observable. `RangeCache` emits per-prefix hit/miss (#1206);
  with plain `new` these tag `"other"` — decide whether to attach `with_prefix_labels` for dimensioned
  L1 metrics (Open Questions).
- **CI**: `python3 build/rust_ci.py`.

## Open Questions

1. **Default `MICROMEGAS_L1_CACHE_MB`.** Old `FileCache` defaulted to 200 MB. Keep 200, or size up now
   that L1 covers all file sizes + static tables? (Proposed: keep 200.)
2. **Dimensioned L1 metrics.** Attach `with_prefix_labels(["views", "blobs"])` (and/or per-ns tags) so
   the L1 win is measurable per the #1206 sequencing note, or leave as `"other"`?
3. **`blocks` metadata-view reads.** `query_partitions(..., lake.blob_storage.inner(), ...)`
   (`partition_source_data.rs:273`, `jit_partitions.rs`) reads the `blocks` metadata *view* (parquet)
   through DataFusion. Confirm whether those reads use the context's wrapped `ReaderFactory` (cached)
   or a fresh unwrapped factory; if unwrapped, decide whether to route them through L1 too.
4. **`FillHint` handling in `BoundedMemoryBackend`.** Full demand/prefetch admission policy in v1, or
   equal-treatment LRU first? (L1 is read-driven, so prefetch fills are rare — equal treatment likely
   fine initially.)
5. **Fetch-permit sizing for L1.** Reuse server defaults (32 / 8), or smaller for an in-process cache?
