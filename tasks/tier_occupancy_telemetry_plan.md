# Object-Cache Tier Occupancy Telemetry Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1322

## Overview

Add the RAM-tier **entry-count** gauge that #1322 asks for alongside the existing
`object_cache_ram_tier_usage_bytes` byte gauge, giving occupancy in both bytes and entry
count for the RAM tier. The issue also asks for a disk-tier bytes gauge; research below
shows the pinned `foyer` 0.22.3 does not expose live disk-tier residency through any public
API reachable from `FoyerBackend` — only cumulative write/read byte+IO counters (already
consumed for the `object_cache_foyer_disk_*_bytes_per_sec` rates) and the disk engine's
one-time partition allocation (effectively == fixed configured capacity, not live
occupancy). This plan ships the RAM entry-count gauge and documents the disk-tier gap as an
explicit, upstream-blocked limitation rather than emitting an approximate or synthetic
number.

## Current State

`object_cache_ram_tier_usage_bytes` already exists (added for #1276/#1277) and is emitted every
5s by the saturation sampler:

- `RangeCacheBackend::ram_usage_bytes(&self) -> Option<usize>` (`rust/object-cache/src/backend.rs:43`)
  — trait default `None`, for backends with no RAM-tier accounting.
- `FoyerBackend::ram_usage_bytes` (`rust/object-cache/src/foyer_backend.rs:562`) returns
  `Some(self.ram_usage())`, and `ram_usage()` (`:350`) is `self.cache.memory().usage()` —
  foyer's own weigher-total byte accounting for the RAM tier.
- `saturation_monitor::sample_once` (`rust/object-cache-srv/src/saturation_monitor.rs:78-84`)
  emits it as `imetric!("object_cache_ram_tier_usage_bytes", "bytes", ram_bytes as u64)`,
  gated on `cache.backend_ram_usage()` (which threads through to `ram_usage_bytes()`)
  returning `Some`.
- Documented in `mkdocs/docs/admin/object-cache.md:256` (Saturation table).

There is **no entry-count equivalent**. `foyer_memory::Cache` (the type behind
`cache.memory()`) exposes both `usage()` (bytes, already used) and a sibling
`entries() -> usize` (`foyer-memory-0.22.3/src/cache.rs:843`) that is not currently called
anywhere in the codebase — a one-line addition next to the existing `usage()` call.

**Disk-tier bytes: not exposed.** Investigated the pinned `foyer = "0.22.3"`
(`rust/Cargo.lock`) public API reachable from `HybridCache`/`Store`/`Device`:

- `HybridCache::statistics() -> &Arc<Statistics>` (`foyer-0.22.3/src/hybrid/cache.rs:629`),
  already consumed by `FoyerBackend::disk_stats` (`foyer_backend.rs:552-560`) for
  `BackendDiskStats`. `Statistics` (`foyer-storage-0.22.3/src/io/device/statistics.rs:84-169`)
  tracks only four cumulative counters since process start —
  `disk_write_bytes`/`disk_read_bytes`/`disk_write_ios`/`disk_read_ios` — used today for the
  `object_cache_foyer_disk_*_per_sec` rate gauges
  (`saturation_monitor.rs:130-153`). There is no eviction/reclaim counter to net against
  writes, so "cumulative bytes written" cannot be turned into "bytes currently resident"
  by subtraction.
- `Store::device() -> &Arc<dyn Device>` (`foyer-storage-0.22.3/src/store.rs:268`) exposes
  `Device::capacity()`, `allocated()`, and `free()` (`foyer-storage-0.22.3/src/io/device/mod.rs:65-72`).
  These look promising but aren't: at `build()` time, `BlockManager::open` loops
  `while device.free() >= block_size { device.create_partition(block_size) }`
  (`foyer-storage-0.22.3/src/engine/block/manager.rs:186-196`, invoked from
  `foyer-storage-0.22.3/src/engine/block/engine.rs:376`), carving the entire device into
  block-sized partitions up front until no free space remains — so `allocated()`/`free()`
  reflect this one-time, startup-only partitioning of the configured `disk_bytes` capacity,
  not live cached-block occupancy. They read a constant (== configured capacity)
  for the life of the process regardless of how full the disk tier actually is.
- `engine/block/manager.rs` has internal `size()`/`blocks()` methods that likely track real
  block occupancy, but `BlockManager` is not reachable from `Store`'s public surface (no
  `engine()`/`manager()` accessor) — it is a private implementation detail of the block
  engine.

Conclusion: a genuine live disk-tier-bytes gauge would require either an upstream `foyer`
change (exposing per-partition live occupancy) or this crate maintaining its own running
counter. The latter was considered and rejected — see Trade-offs. The same applies to a
disk-tier entry count: `blocks()`, the count-like sibling to `size()`, lives on the same
unreachable `BlockManager`, so entry count is exactly as infeasible as bytes, for the
identical reason.

## Design

Extend the existing RAM-usage accessor path with an entry-count sibling, following the
exact shape `ram_usage_bytes` already established:

1. **`RangeCacheBackend` trait** (`rust/object-cache/src/backend.rs`): add
   `fn ram_entries(&self) -> Option<usize> { None }`, defaulted like `ram_usage_bytes`, doc
   comment noting it's the entry-count sibling to the byte gauge.
2. **`FoyerBackend`** (`rust/object-cache/src/foyer_backend.rs`): add a small
   `pub fn ram_entries(&self) -> usize { self.cache.memory().entries() }` next to
   `ram_usage()` (`:347-352`, same doc-comment shape: exposed for integration tests too),
   and implement the trait method as `Some(self.ram_entries())` next to the existing
   `ram_usage_bytes` impl (`:562-564`).
3. **`RangeCache`** (`rust/object-cache/src/range_cache/mod.rs`): add a
   `backend_ram_entries(&self) -> Option<usize>` passthrough next to the existing
   `backend_ram_usage` passthrough (same pattern — locate and mirror its signature/body).
4. **`saturation_monitor::sample_once`** (`rust/object-cache-srv/src/saturation_monitor.rs:78-84`):
   alongside the existing `ram_tier_usage_bytes` block, add:
   ```rust
   if let Some(ram_entries) = cache.backend_ram_entries() {
       imetric!(
           "object_cache_ram_tier_entries",
           "count",
           ram_entries as u64
       );
   }
   ```

No changes to disk-tier telemetry — the existing `object_cache_foyer_disk_*_bytes_per_sec`
/`_ios_per_sec` throughput gauges already cover what `Statistics` can support, and no new
disk-bytes gauge is added (see Current State / Trade-offs).

## Implementation Steps

1. `rust/object-cache/src/backend.rs` — add `ram_entries` to `RangeCacheBackend` with a
   `None` default, doc comment mirroring `ram_usage_bytes`.
2. `rust/object-cache/src/foyer_backend.rs` — add `FoyerBackend::ram_entries()` (public,
   mirrors `ram_usage()`) and the trait impl.
3. `rust/object-cache/src/range_cache/mod.rs` — add `RangeCache::backend_ram_entries()`
   passthrough, mirroring `backend_ram_usage`.
4. `rust/object-cache-srv/src/saturation_monitor.rs` — emit
   `object_cache_ram_tier_entries` in `sample_once`, gated the same way as the bytes gauge.
5. `rust/object-cache-srv/tests/saturation_tests.rs` — add a test asserting the new gauge
   fires with the expected count (see Testing Strategy).
6. `mkdocs/docs/admin/object-cache.md` — add a Saturation-table row for
   `object_cache_ram_tier_entries`, next to the `object_cache_ram_tier_usage_bytes` row
   (`:256`), and a short note under/near it that disk-tier bytes/entries are not emitted
   because foyer 0.22.3 does not expose live disk-tier residency (cross-reference this so a
   future reader doesn't file it again as "still missing").
7. `CHANGELOG.md` — add an entry under the appropriate unreleased section.
8. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and
   `cargo test -p micromegas-object-cache -p micromegas-object-cache-srv --features foyer`.

## Files to Modify

- `rust/object-cache/src/backend.rs` — new `ram_entries` trait method.
- `rust/object-cache/src/foyer_backend.rs` — new `ram_entries()` inherent method + trait impl.
- `rust/object-cache/src/range_cache/mod.rs` — new `backend_ram_entries()` passthrough.
- `rust/object-cache-srv/src/saturation_monitor.rs` — new gauge emission in `sample_once`.
- `rust/object-cache-srv/tests/saturation_tests.rs` — new test.
- `mkdocs/docs/admin/object-cache.md` — Saturation-table row + disk-tier limitation note.
- `CHANGELOG.md` — changelog entry.

## Trade-offs

- **Self-tracked disk-bytes counter, rejected.** An alternative to the "not exposed"
  conclusion is maintaining our own running disk-tier byte counter: increment on each
  RAM→disk write-back (observable via `RamEvictionListener::on_leave`'s `Event::Evict`,
  which already fires per evicted block) and decrement on disk→RAM promotion (observable in
  `promote_if_valid`). Rejected because there is no corresponding signal for a block being
  reclaimed *from the disk tier itself* (foyer's own disk-side LRU/region reclaim runs
  entirely inside the block engine with no listener hook) — so the counter would only ever
  grow via the decrement-on-promotion path and silently drift high of the true value over
  any run where disk-tier capacity pressure causes disk-side eviction without a matching
  RAM promotion. A gauge that quietly overstates occupancy is worse than no gauge; better to
  document the gap than ship a number that looks authoritative but isn't.
- **Directory-size-on-disk approximation, rejected.** Periodically `stat`-ing
  `--disk-path` was considered as a filesystem-level proxy for occupancy. Rejected on the
  same evidence as the `Device::allocated()` finding above: `BlockManager::open` carves the
  full configured-capacity partition file up front at startup
  (`manager.rs:186-196`, invoked from `engine.rs:376`), so the region
  file's size is constant (== configured capacity) from first boot regardless of live
  utilization — a directory walk would report the same number whether the tier is empty or
  full.
- **RAM entries per-prefix, not attempted.** The issue allows falling back to a global
  gauge "if per-prefix accounting is feasible, else global" — matching the existing global
  `ram_tier_usage_bytes`. `foyer_memory::Cache` has no per-prefix accounting API; producing
  one would mean iterating all cache entries under lock on every 5s tick, which is exactly
  the class of hot-path/lock-contention cost the existing per-event `imetric!` tagging
  approach (classify at insert/evict time, not scan at sample time) was designed to avoid.
  Kept global, consistent with `ram_tier_usage_bytes`.

## Documentation

- `mkdocs/docs/admin/object-cache.md` Saturation table (`:247-261`): add
  `object_cache_ram_tier_entries` next to `object_cache_ram_tier_usage_bytes`, and a short
  explanatory line that neither disk-tier bytes nor disk-tier entry count is emitted, with
  the reason (foyer 0.22.3 exposes no live disk-residency API for either, since both would
  require the same unreachable `BlockManager`), so this doesn't read as an oversight.
- `CHANGELOG.md`: one entry for the new gauge.

## Testing Strategy

Add `ram_tier_entries_gauge_reflects_cached_block_count` to
`rust/object-cache-srv/tests/saturation_tests.rs`, modeled on the existing
`foyer_disk_gauges_emit_only_after_a_second_tick` test's setup (a `FoyerBackend`-backed
`RangeCache`, `init_in_memory_tracing` guard, `flush_metrics_buffer`,
`integer_metric_values` helper already in that file):

- Put N distinct keys into the cache (small enough to stay resident in the RAM tier given
  the test's configured `ram_bytes`).
- Call `sample_once`.
- `flush_metrics_buffer()`, then assert `integer_metric_values(&sink,
  "object_cache_ram_tier_entries")` contains exactly one value equal to N.

Regression: existing `saturation_tests.rs` and `foyer_backend_tests.rs` must pass
unchanged — this only adds a new accessor and a new emission, no control-flow changes. Run
`cargo test -p micromegas-object-cache -p micromegas-object-cache-srv --features foyer` and
`cargo clippy --workspace -- -D warnings`.

## Open Questions

None — the disk-tier gap is a documented limitation (see Current State / Trade-offs), not
an open question to resolve during implementation.
