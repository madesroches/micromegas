# In-Process L1 Cache Plan (#1205)

Tracking issue: [#1205](https://github.com/madesroches/micromegas/issues/1205) — "object-cache:
in-process L1 cache to eliminate the network hop on hot reads". Follow-up to the range-aware read
cache (#1188) and the read-path rework (#1203/#1216).

## Overview

Add an **in-process L1 cache** for parquet row-group bytes, built from the existing `RangeCache`
core (`rust/object-cache/src/range_cache.rs`) with a **bounded RAM backend**, composed as an
`ObjectStore` layer in front of `CacheClientStore` (the L2 client). Hot reads short-circuit at L1
with zero network; an L1 miss falls through to L2 (or directly to origin object storage when L2 is
not configured).

The same change **removes the old whole-file `FileCache`** (`file_cache.rs` / `caching_reader.rs`),
which today only caches files ≤10 MB at the parquet-reader layer. L1 subsumes it: it caches hot
bytes for files of all sizes at a lower layer, closing the >10 MB gap that motivates this issue and
eliminating a redundant second copy of small-file bytes.

Two scope constraints (from the issue discussion):
1. **L1 caches parquet files only, not blobs.** Raw blobs (`.../blobs/...`) are written once at
   ingestion and read once during ETL — caching them would only churn the RAM budget and evict
   genuinely hot parquet bytes.
2. **L1 replaces `FileCache`**, it does not sit alongside it.

## Current State

### Layering today

`connect_to_data_lake` (`rust/ingestion/src/data_lake_connection.rs:111-131`) builds the object-store
stack:

```
BlobStorage.inner() = PrefixStore(root) -> CacheClientStore (L2, if configured) -> raw store (S3/GCS)
```

- `make_cache` (`data_lake_connection.rs:86-108`) wraps the raw full-bucket store in
  `CacheClientStore` **only when `MICROMEGAS_OBJECT_CACHE_URL` + `..._API_KEY` are set**; otherwise
  it returns the raw store unwrapped.
- `BlobStorage::new` (`rust/telemetry/src/blob_storage.rs:19-23`) wraps that layer in
  `PrefixStore(root)`. Because the prefix store is outermost, the cache layer sees **bucket-relative
  keys that already include the lake root**, e.g. `{root}/views/.../x.parquet` and `{root}/blobs/...`
  (confirmed: `blob_storage.rs:47-56`, and the server forbids a non-empty origin prefix for the same
  reason).

### The read path and the old file cache

- `ReaderFactory` (`rust/analytics/src/lakehouse/reader_factory.rs:29-88`) is DataFusion's
  `ParquetFileReaderFactory`. It holds `object_store` (= `BlobStorage.inner()`), a `MetadataCache`,
  and a `FileCache`, and builds a `ParquetReader` per file wrapping a `CachingReader`.
- `CachingReader` (`rust/analytics/src/lakehouse/caching_reader.rs`) caches **whole files ≤10 MB** in
  the shared `FileCache` and slices ranges out of the cached `Bytes`; files >10 MB bypass the cache
  and call `object_store.get_range`/`get_ranges` directly — which, when L2 is configured, cross the
  network on **every** repeat read.
- `FileCache` (`rust/analytics/src/lakehouse/file_cache.rs`) is a moka LRU keyed by path, byte-weighted,
  default 200 MB budget / 10 MB max-file, with thundering-herd coalescing via `try_get_with`.
- `load_partition_metadata` (`rust/analytics/src/lakehouse/partition_metadata.rs:102-147`) reads the
  parquet footer through a `CachingReaderFetch` adapter (`partition_metadata.rs:78-94`) over the same
  `CachingReader`, so footer reads for >10 MB files also cross the network on cold in-process reads.
- Config knobs today (`rust/analytics/src/lakehouse/lakehouse_context.rs:17-20,79-103`):
  `MICROMEGAS_FILE_CACHE_MB` (default 200), `MICROMEGAS_FILE_CACHE_MAX_FILE_MB` (default 10).

### The RangeCache core (reused as-is)

- `RangeCache` (`rust/object-cache/src/range_cache.rs:511-559`) — block model (default block
  1 MiB), single-flight in-flight map, run coalescing, priority fetch budget, write-once keys (no
  invalidation). `#[derive(Clone)]`, cheap to clone.
- Public read API: `get_range(key, range)` (`:1141`), `get_ranges(key, ranges)` (`:1175`),
  `size(key)` (`:625`), plus `*_with_size` and `stream_ranges_with_size` variants.
- Plain `new(origin, backend, block_size, ns, total_fetch_permits, demand_reserved_fetch_permits,
  max_coalesced_get_bytes, promote_whole_batch)` works without prefix labels (every key classifies
  as `"other"`); `with_prefix_labels` is an optional builder for dimensioned metrics.
- Backends implement `RangeCacheBackend { get, put, disk_stats }` (`backend.rs`).
  - `MemoryBackend` (`memory_backend.rs`) is a plain `Mutex<HashMap<String, Bytes>>` — **unbounded,
    no eviction, ignores `FillHint`.** Not usable as-is for a budgeted L1.
  - `FoyerBackend` (`foyer_backend.rs`) has a bounded, byte-weighted LRU RAM tier but is
    **feature-gated (`foyer`, off by default)** and always constructs an on-disk device — there is no
    clean RAM-only path.
- Reference construction (`rust/object-cache-srv/src/object_cache_srv.rs:153-162`), defaults from
  `range_cache.rs:64-72`: `total_fetch_permits = 32`, `demand_reserved = 8`,
  `max_coalesced_get_bytes = 8 MiB`, `promote_whole_batch = false`, block size 1 MiB.

## Design

### Target layering

```
BlobStorage.inner() = PrefixStore(root)
                        -> L1CacheStore (NEW: in-process RangeCache + bounded RAM backend)
                          -> CacheClientStore (L2, if configured)
                            -> raw store (S3/GCS)
```

L1 is inserted inside `make_cache`, wrapping whatever origin it is given: `CacheClientStore` when L2
is configured, otherwise the raw store. This keeps in-process caching **always on** (matching the old
`FileCache`, which was unconditional), so a monolith with no object-cache-srv does not regress — it
just gets a byte-range RAM cache instead of a whole-file one.

### New component: `L1CacheStore` (an `ObjectStore` over a `RangeCache`)

`RangeCache` is an internal API, not an `ObjectStore`; `CacheClientStore` is the only
`impl ObjectStore` wrapper today. Add a second adapter in the `object-cache` crate that exposes a
RAM-backed `RangeCache` as an `ObjectStore`, so it slots into the layer chain exactly like
`CacheClientStore`:

```rust
// rust/object-cache/src/l1_store.rs (new)
pub struct L1CacheStore {
    cache: RangeCache,            // backend = bounded RAM; origin = the wrapped store
    origin: Arc<dyn ObjectStore>, // same store RangeCache fetches from; used for pass-through ops
}
```

Behavior of the `ObjectStore` impl (mirrors `CacheClientStore`'s structure):

- **`get_opts`** — cache only when the key is parquet (see filter below) and the request has no
  conditional/version preconditions:
  - `GetRange::Bounded(r)` → `cache.get_range(key, r)`, returned as a single-chunk streaming
    `GetResult` (reuse the existing `build_get_result` pattern).
  - full / open-ended / suffix → resolve size via `cache.size(key)` (or `get_ranges_with_size`) and
    serve from the cache; suffix computes `start = size - suffix`.
  - non-parquet key, precondition set, or `head` → delegate straight to `origin`.
- **`get_ranges`** — parquet key → `cache.get_ranges(key, ranges)`; else delegate to `origin`.
- **`put_*`, `list`, `delete`, `copy`, `head`, `get` (unranged full for non-parquet)** — delegate to
  `origin` unchanged. L1 never caches writes; write-once keys mean no invalidation is needed.

Because `RangeCache` already does single-flight, coalescing, and its own error handling, the adapter
is thin. On any cache error the adapter falls back to `origin` for the same request (same graceful
degradation contract as `CacheClientStore`).

### Parquet-only filter

L1 sees bucket-relative keys with the lake root prefix, so a bare `views/` prefix match is not
root-robust. **Cache iff the key ends in `.parquet`.** Blobs (`{root}/blobs/{process}/{stream}/{block}`)
never carry that suffix, so this cleanly excludes them and is independent of the lake root. Everything
else falls straight through to `origin`. (A `helper is_cacheable(key: &str) -> bool` keeps this in one
place.)

### Bounded RAM backend

`MemoryBackend` is unbounded, so L1 needs a budgeted backend. Add a byte-bounded LRU backend in the
`object-cache` crate:

```rust
// rust/object-cache/src/bounded_memory_backend.rs (new)
pub struct BoundedMemoryBackend { /* LRU keyed by String, weighted by value.len(), byte cap */ }
impl RangeCacheBackend for BoundedMemoryBackend { get; put (evicts LRU while over budget); }
```

- Weight = `value.len()`; evict least-recently-used entries on `put` until within the byte cap.
- `FillHint::Prefetch` fills should not evict hot `Demand` data — at minimum, honor the hint by not
  admitting a prefetch fill that would evict demand entries (foyer's RAM tier has an equivalent
  policy). A first cut may treat both equally and refine later.
- `disk_stats()` returns `None` (no disk tier) — already the trait default.

Rationale for a new backend over the two existing options is in **Trade-offs**.

### Wiring

`make_cache` (`data_lake_connection.rs`) gains the L1 layer:

```rust
fn make_cache(direct) -> (Arc<dyn ObjectStore>, Option<Arc<dyn ObjectPrefetch>>) {
    let (l2_or_direct, prefetch) = /* existing CacheClientStore logic */;
    let l1 = build_l1(l2_or_direct.clone());     // reads MICROMEGAS_L1_CACHE_MB etc.
    (l1, prefetch)                                // prefetch face unchanged (still the L2 client)
}
```

- `build_l1` constructs `BoundedMemoryBackend`, a `RangeCache::new(origin, backend, block_size, ns,
  ...)` with the block/permit/coalesce defaults, and returns `Arc::new(L1CacheStore { ... })`.
- When `MICROMEGAS_L1_CACHE_MB == 0`, `build_l1` returns the origin unwrapped (L1 disabled).
- `ns` for L1 can be derived from the origin URI like the server does, or a fixed `"l1"` — L1 is a
  single process-local namespace, so a constant is fine.
- The prefetch face returned for write-time warming is still the **L2** client (`CacheClientStore`);
  L1 is a read-side cache and is not warmed on write.

### Removing the old file cache (analytics)

- Delete `rust/analytics/src/lakehouse/file_cache.rs`, `caching_reader.rs`, and
  `rust/analytics/tests/file_cache_tests.rs`; drop their `mod` lines in `lakehouse/mod.rs`.
- `ReaderFactory` (`reader_factory.rs`): drop the `file_cache` field; `ParquetReader` holds
  `object_store: Arc<dyn ObjectStore>` + `path` and implements `AsyncFileReader::get_bytes` /
  `get_byte_ranges` by calling `object_store.get_range` / `get_ranges` directly (L1 now lives in that
  store). Keep the `parquet_read ... duration_ms` debug log; the per-read `cache_hit` flag goes away
  (L1 emits its own hit/miss metrics — see Testing/Telemetry).
- `load_partition_metadata` (`partition_metadata.rs`): replace the `CachingReaderFetch` adapter with a
  small `MetadataFetch` that reads `object_store.get_range(path, range)`. Change the signature from
  `reader: &mut CachingReader` to `object_store: &Arc<dyn ObjectStore>, path: &Path` (footer reads now
  benefit from L1 too — this addresses the #1121 footer-read tie-in the issue mentions).
- `lakehouse_context.rs`: drop `FileCache` construction and the `MICROMEGAS_FILE_CACHE_MB` /
  `MICROMEGAS_FILE_CACHE_MAX_FILE_MB` knobs; `ReaderFactory::new` no longer takes a file cache.

## Implementation Steps

**Phase 1 — object-cache crate (new building blocks)**
1. Add `BoundedMemoryBackend` (`bounded_memory_backend.rs`) implementing `RangeCacheBackend` with a
   byte-weighted LRU and a configurable cap; export from `lib.rs`. Unit tests: eviction at budget,
   weight accounting, `get`/`put` round-trip, hint handling.
2. Add `L1CacheStore` (`l1_store.rs`) implementing `ObjectStore` over a `RangeCache`, with the
   parquet-only filter and origin pass-through/fallback; export from `lib.rs`.

**Phase 2 — wiring (ingestion)**
3. Add `build_l1` + the `MICROMEGAS_L1_CACHE_MB` (and any block/permit) env knobs; insert L1 in
   `make_cache` in front of the existing L2/direct store. `0` disables.

**Phase 3 — remove the old file cache (analytics)**
4. Refactor `load_partition_metadata` to read via `object_store` + `path`.
5. Refactor `ReaderFactory` / `ParquetReader` to read directly from `object_store`; drop the
   `file_cache` field.
6. Delete `file_cache.rs`, `caching_reader.rs`, `file_cache_tests.rs`; update `mod.rs`.
7. Update `lakehouse_context.rs` to drop `FileCache` and its env knobs.

**Phase 4 — verify**
8. `cd rust && cargo fmt && cargo clippy --workspace -- -D warnings && cargo test`.
9. Manual: run flight-sql with and without `MICROMEGAS_OBJECT_CACHE_URL`, confirm repeat queries hit
   L1 (metrics) and blobs are not cached.

## Files to Modify

**New**
- `rust/object-cache/src/bounded_memory_backend.rs`
- `rust/object-cache/src/l1_store.rs`
- `rust/object-cache/tests/l1_store_tests.rs`, `.../bounded_memory_backend_tests.rs`

**Modified**
- `rust/object-cache/src/lib.rs` (exports)
- `rust/ingestion/src/data_lake_connection.rs` (`make_cache`, `build_l1`, env knobs)
- `rust/analytics/src/lakehouse/reader_factory.rs`
- `rust/analytics/src/lakehouse/partition_metadata.rs`
- `rust/analytics/src/lakehouse/lakehouse_context.rs`
- `rust/analytics/src/lakehouse/mod.rs`

**Deleted**
- `rust/analytics/src/lakehouse/file_cache.rs`
- `rust/analytics/src/lakehouse/caching_reader.rs`
- `rust/analytics/tests/file_cache_tests.rs`

## Trade-offs

- **Bounded RAM backend: new `BoundedMemoryBackend` vs. `FoyerBackend` RAM-only vs. re-adding moka.**
  Foyer's RAM tier is already a bounded weighted LRU, but it is feature-gated and always builds an
  on-disk device (no clean RAM-only path) — pulling foyer's disk machinery into every flight-sql /
  monolith process just to bound RAM is the wrong footprint for a "zero-network, RAM-only" L1. moka
  was deliberately removed from `object-cache` in #1203; re-adding it would regress that cleanup. A
  small byte-weighted LRU backend is the least-surprising fit, keeps L1 free of disk deps, and is
  reusable. Cost: a modest amount of new, well-scoped code.
- **Block granularity vs. whole-file.** The old `FileCache` cached whole ≤10 MB files; L1 caches
  1 MiB blocks. For large files this is the whole point (cache only touched row-group blocks instead
  of nothing). For a cold one-shot read of a small file, block-aligned fetching can pull marginally
  more bytes than exact ranges, but repeat reads are served from RAM. Net win on the hot path.
- **L1 always-on vs. only-with-L2.** The issue frames L1 "in front of `CacheClientStore`", but the
  old `FileCache` ran unconditionally. Gating L1 on L2 would regress no-L2 deployments (monolith), so
  L1 wraps whatever origin it is given and is controlled by its own `MICROMEGAS_L1_CACHE_MB` knob.
- **Parquet-only by `.parquet` suffix vs. prefix classification.** `RangeCache::classify` exists but
  is metrics-only and prefix-based; the suffix check is simpler, root-agnostic, and the correct gate
  for *whether to cache at all*. Prefix labels remain available if we later want dimensioned L1
  metrics.

## Documentation

- Update the admin/object-cache docs (the page added in #1188/`1117316ca`, under `mkdocs/docs/`) to
  describe the L1 tier and the `MICROMEGAS_L1_CACHE_MB` knob, and to note that `MICROMEGAS_FILE_CACHE_*`
  are removed.
- Note the removal of `MICROMEGAS_FILE_CACHE_MB` / `MICROMEGAS_FILE_CACHE_MAX_FILE_MB` wherever env
  vars are listed for services.

## Testing Strategy

- **Unit (`object-cache`)**: `BoundedMemoryBackend` eviction/weight/hint; `L1CacheStore` — parquet
  keys are cached (second read issues no origin request), blob/non-parquet keys always pass through,
  ranged/full/suffix gets return correct bytes, origin errors fall back.
- **Integration**: a `RangeCache`-over-`L1CacheStore` with a counting mock origin `ObjectStore`
  proving repeat parquet reads hit RAM (origin call count stays flat) while blob reads always reach
  origin.
- **Analytics regression**: existing lakehouse/query tests must pass unchanged after the `FileCache`
  removal; port any still-relevant assertions from `file_cache_tests.rs` (e.g. large-file reads
  succeed) into the new L1 tests rather than dropping coverage.
- **Telemetry**: confirm L1 hit/miss is observable. `RangeCache` emits per-prefix hit/miss metrics
  (#1206); with plain `new` these tag as `"other"`. Decide whether to call `with_prefix_labels` on L1
  so the L1 win is measurable per the #1206 sequencing note (Open Questions).
- **CI**: `python3 build/rust_ci.py`.

## Open Questions

1. **Default `MICROMEGAS_L1_CACHE_MB`.** Old `FileCache` defaulted to 200 MB. Keep 200, or size L1
   larger now that it covers all file sizes? (Proposed: keep 200 as a safe default.)
2. **Prefix-labeled L1 metrics.** Attach `with_prefix_labels(["views", "blobs"])` to L1 for
   dimensioned hit-rate metrics, or leave everything as `"other"`? Attaching makes the L1 win
   measurable (ties into #1206) at the cost of a little wiring.
3. **`FillHint` handling in `BoundedMemoryBackend`.** Full demand/prefetch admission policy in v1, or
   ship a simple equal-treatment LRU first and refine? L1 is read-driven (no write-warming), so
   prefetch fills are rare here — equal treatment is likely fine initially.
4. **Fetch-permit sizing for L1.** Reuse the server defaults (32 / 8), or smaller for an in-process
   RAM cache whose "origin" is often another in-process/LAN hop? (Proposed: reuse defaults; revisit
   if it over-fans-out to S3 in the no-L2 case.)
