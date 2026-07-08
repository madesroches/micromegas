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

Separately, `query_partitions_context` (`query.rs:60,67`) also calls
`ctx.register_object_store("obj://lakehouse/", object_store)` with the store passed by every
caller — `partition_source_data.rs:269-273` and all four sites in `jit_partitions.rs` — as
`blob_storage.inner()`, **unwrapped**. This is intentional and does not need L1 wrapping: parquet
byte reads are served by the `ReaderFactory` (fact 1 above), never by this registered store;
`register_object_store` here only lets DataFusion resolve `obj://lakehouse/...` URLs, it isn't on
the byte-read path.

**Consequence:** wrap the store *passed to* `ReaderFactory::new` (and the static-tables store); do
**not** wrap inside `BlobStorage` (`connect_with_layer`), in `make_cache`, or the store registered by
`query_partitions_context`, which would cache blob reads too (or wrap a store that never serves
bytes).

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
  `promote_whole_batch = false`, block 1 MiB. For L1 only `total_fetch_permits` is meaningful:
  reads are demand-only (no prefetch path), so `demand_reserved` is inert here — see Open Questions.

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

### Crate placement (all in `object-cache`)

The bounded backend, `L1CacheStore`, and `l1_wrap` all live in `object-cache`, next to the existing
`MemoryBackend`/`FoyerBackend` and the `RangeCache` they pair with — generic, reusable, and testable
in that crate's suite. `analytics` gains a dependency on `object-cache` and simply calls
`object_cache::l1_wrap(...)` at the two wrap sites. This is possible because the bounded backend uses
the standalone `foyer-memory` crate's in-memory `Cache` type (`foyer_memory::Cache`, see below) — **not**
the umbrella `foyer` crate, which would transitively pull in `foyer-storage`/`io-uring`/`libc` — so no
moka is introduced into `object-cache` (respecting the #1203 removal). The shared-budget backend is an
`object-cache`-owned global.

### New component: `L1CacheStore` (an `ObjectStore` over a `RangeCache`)

`RangeCache` is an internal API, not an `ObjectStore`. Add a thin adapter in `object-cache` exposing a
RAM-backed `RangeCache` as an `ObjectStore`, so it drops into either wrap site:

```rust
// rust/object-cache/src/l1_store.rs (new)
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

`MemoryBackend` (`object-cache`) is unbounded. Back the new bounded variant with the standalone
`foyer-memory` crate's in-memory `Cache` — not hand-rolled, not `HybridCache`, and **not** the umbrella
`foyer` crate (which transitively pulls in `foyer-storage`/`io-uring`/`libc`, the exact deps this
component exists to avoid; the existing `FoyerBackend` depends on the umbrella `foyer::` path and uses
`HybridCache`/`LruConfig`). `foyer_memory::Cache` is a pure in-memory, byte-weighted, sharded cache with
configurable eviction (`LfuConfig`/`FifoConfig`); it has **no disk-IO dependencies** (verified:
`foyer-memory` 0.22 pulls no `foyer-storage` or `io_uring`; `libc` still comes in transitively via
`tokio`/`parking_lot`, but that is unrelated to disk). It is the same in-memory engine the
umbrella `foyer` crate's `HybridCache` uses internally for its RAM tier, so L1 reuses the exact eviction
implementation `FoyerBackend` already relies on — via a different eviction config (`LfuConfig` here vs.
`FoyerBackend`'s `LruConfig`) and a lighter, disk-free crate.

```rust
// rust/object-cache/src/bounded_memory_backend.rs (new)
pub struct BoundedMemoryBackend { cache: foyer_memory::Cache<String, Bytes> }
// built as: foyer_memory::CacheBuilder::new(budget_bytes).with_weighter(|_k, v| v.len())
//   .with_eviction_config(foyer_memory::LfuConfig::default())
//   .build() // terminal call, required to get a `Cache`; the `Properties` generic on
//            // `build::<P>()` defaults so the `Cache<String, Bytes>` field type resolves
#[async_trait] impl RangeCacheBackend for BoundedMemoryBackend { get; put; } // disk_stats -> None (default)
```

- Byte-weighted capacity; foyer handles eviction and sharded concurrency. Pure `get`/`put` (no
  single-flight in the backend) because `RangeCache` already owns single-flight via its in-flight map
  (#1203).
- `BoundedMemoryBackend` ignores `FillHint` (treats `Demand` and `Prefetch` identically): with no disk
  tier, demand/prefetch admission is meaningless here. `FoyerBackend` only honors the hint by writing
  SSD-only via `storage_writer(...).force()` (`foyer_backend.rs:121`), which has no equivalent without
  a disk tier, and `foyer_memory::Cache` exposes only a plain `insert`. `MemoryBackend` already ignores
  the hint too. Revisit only if a future disk-backed L1 variant is added.
- `disk_stats()` returns `None` (trait default).

`foyer-memory` 0.22.3 is already present in `rust/Cargo.lock` transitively (pulled by the umbrella
`foyer` 0.22.3 and by `foyer-storage`), so adding it as a (non-optional) dependency of `object-cache`
promotes an existing transitive crate to a direct one — it costs nothing new in the dependency tree,
only a new manifest entry. It **must** be pinned to the same `0.22.x` line as the umbrella `foyer`
dependency (lockstep-versioned sub-crates); pinning to a different line (e.g. `0.23`) would put two
`foyer-memory` versions in the tree. The existing `foyer` **feature** (which pulls the disk-backed
`HybridCache` for `object-cache-srv`) is unchanged and still optional.

### Shared backend across both wrap sites

An `object-cache`-owned lazily-initialized global backend, sized once from `MICROMEGAS_L1_CACHE_MB`
(default 200 MB, matching the old `FileCache`'s `DEFAULT_FILE_CACHE_SIZE_MB` at
`lakehouse_context.rs:17`), gives one shared budget for both wrap sites:

```rust
// rust/object-cache/src/l1_store.rs
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
- `static_tables_configurator.rs:76`: `let object_store = object_cache::l1_wrap(Arc::new(object_store), "static");`
  before `register_object_store`.
- `analytics/Cargo.toml`: add a dependency on `micromegas-object-cache` (analytics does not depend on
  it today).
- `blocks`-view reads are covered automatically: `query_partitions` (`query.rs:80-91`) takes
  `reader_factory: Arc<ReaderFactory>` as a parameter and never builds its own. Every caller —
  `partition_source_data.rs:269-273` and all four sites in `jit_partitions.rs` (75-79, 131-135, 277-281,
  415-419) — passes `lakehouse.reader_factory().clone()`, i.e. the single shared `ReaderFactory` from
  `LakehouseContext`. So wrapping at `lakehouse_context.rs:106`/`:127` (above) automatically L1-caches
  `blocks`-view reads too; no extra routing is needed.

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

**Phase 1 — building blocks (in `object-cache`)**
1. Add `foyer-memory` to `object-cache/Cargo.toml` (non-optional) and the workspace root, pinned to
   the same `0.22.x` line as the umbrella `foyer` dependency (already in `Cargo.lock` transitively at
   0.22.3 — this promotes it to a direct dep without adding a new crate to the tree).
2. `BoundedMemoryBackend` (`bounded_memory_backend.rs`), foyer-in-memory-based, implementing
   `RangeCacheBackend` + export. Tests: byte-weighted eviction at budget, get/put round-trip.
3. `L1CacheStore` (`l1_store.rs`) implementing `ObjectStore` over a `RangeCache`, with origin
   pass-through/fallback; plus `shared_l1_backend()` / `l1_wrap(origin, ns)` reading
   `MICROMEGAS_L1_CACHE_MB` (0 = disabled) + export.

**Phase 2 — wiring (analytics)**
4. Add `micromegas-object-cache` to `analytics/Cargo.toml`.
5. Wrap the reader-factory store at `lakehouse_context.rs:106` and `:127` via
   `object_cache::l1_wrap(..., "lakehouse")`.
6. Wrap the static-tables store at `static_tables_configurator.rs:76` via
   `object_cache::l1_wrap(..., "static")`.

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

**New (in `object-cache`)**
- `rust/object-cache/src/bounded_memory_backend.rs`
- `rust/object-cache/src/l1_store.rs` (`L1CacheStore` + `shared_l1_backend`/`l1_wrap`)
- `rust/object-cache/tests/l1_store_tests.rs`

**Modified**
- `rust/Cargo.toml` (workspace: add `foyer-memory`)
- `rust/object-cache/Cargo.toml` (add `foyer-memory`); `object-cache/src/lib.rs` (exports)
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
- **`foyer_memory::Cache` vs. moka vs. hand-rolled LRU.** The standalone `foyer-memory` crate's `Cache`
  is a pure in-memory, byte-weighted, sharded cache with **no disk/IO deps** — unlike the umbrella
  `foyer` crate (already a dependency, via the `foyer` feature, for `FoyerBackend`'s `HybridCache`/
  `LruConfig`, and which transitively pulls in `foyer-storage`/`io-uring`/`libc`). `foyer-memory` is the
  same in-memory engine the umbrella crate's `HybridCache` uses internally for its RAM tier, so L1 reuses
  the exact eviction implementation `FoyerBackend` already relies on (with `LfuConfig` instead of
  `FoyerBackend`'s `LruConfig`), and the backend lives in `object-cache` next to
  `MemoryBackend`/`FoyerBackend`. (An earlier draft wrongly assumed foyer forces a disk tier; that is
  only true of the umbrella `foyer` crate used via `HybridCache`, not of `foyer-memory` used directly.)
  moka — the workspace LRU already backing `FileCache` — is an alternative that adds no *new* crate to
  the tree either (moka is already a workspace dependency elsewhere), but it would either re-introduce
  moka into `object-cache` (reversing the #1203 removal) or split the backend off into `analytics`.
  Chosen: `foyer-memory`, for stack consistency and clean placement; since it is already in
  `Cargo.lock` transitively at 0.22.3 (pulled by the umbrella `foyer` crate), promoting it to a direct
  `object-cache` dependency (pinned to the same `0.22.x` line) costs a manifest entry, not a new crate
  in the tree — and it stays light and disk-free (distinct from the existing optional `foyer` feature).
  Hand-rolling an LRU was rejected as needless risk.
- **Shared `object-cache`-owned global backend.** One lazily-initialized global gives a single RAM
  budget for both wrap sites; write-once/namespaced keys make the shared instance safe (identical bytes
  for identical keys).
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
  with plain `new` these tag `"other"`. L1 does **not** attach `with_prefix_labels`: the lakehouse ns
  only ever sees `views/...` (a single bucket, redundant with the per-ns total) and static-ns keys are
  not under `views/` (they would fall to `"other"`), so a prefix label adds no separating power over the
  two-ns split. Rely on the per-ns metrics (`"lakehouse"` vs `"static"`); revisit only if a second
  reachable prefix is ever introduced.
- **CI**: `python3 build/rust_ci.py`.

## Open Questions

1. **Fetch-permit sizing for L1 (`total` only).** L1 is demand-only — there is no prefetch path
   in-process (parquet reads come through `ReaderFactory → CachingReader` as `Priority::Demand`, and
   nothing issues a `Prefetch` run), so `demand_reserved_fetch_permits` is inert here: the
   `prefetch_permits` semaphore is never acquired and the reservation never gates anything. Pass a small
   placeholder (must be `< total`) and ignore it. The only meaningful knob is `total_fetch_permits`, which
   caps concurrent origin GETs and therefore transient fetch-buffer memory at roughly
   `total × max_coalesced_get_bytes` (≈ `total × 8 MiB`) — memory that is *separate from* the
   `MICROMEGAS_L1_CACHE_MB` budget and lives inside a query process alongside DataFusion execution. The
   server's `32` was sized for a dedicated caching service; in-process, prefer a smaller `total` (e.g.
   `16`, ≈128 MiB transient cap) to avoid over-parallelizing object I/O and stacking transient buffers on
   top of the cache and DataFusion's working set. Final value is a memory-vs-miss-fill-throughput tuning
   call for the target deployment.
