# Object-Cache Tier Occupancy Telemetry Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1322

## Overview

Add the RAM-tier **entry-count** gauge that #1322 asks for alongside the existing
`object_cache_ram_tier_usage_bytes` byte gauge, giving occupancy in both bytes and entry
count for the RAM tier. The issue also asks for a disk-tier bytes gauge; research below
shows that none of the disk-tier accessors reachable from the `HybridCache`/`Store`/`Device`
surface that `FoyerBackend` already uses expose live disk-tier residency — only cumulative
write/read byte+IO counters (already consumed for the `object_cache_foyer_disk_*_bytes_per_sec`
rates) and the disk engine's one-time partition allocation (effectively == fixed configured
capacity, not live occupancy). Separately, foyer's `HybridCacheBuilder::with_metrics_registry()`
*does* provide a genuine, live disk-tier block-occupancy signal — including reclaim — via its
internal `mixtrics`-based metrics facade; this is a real, technically feasible path, not a
dead end, but this plan deliberately defers it (see Trade-offs) rather than adding a new
direct dependency and a custom metrics-registry adapter to what is otherwise a minimal,
one-line RAM-entries change. This plan ships the RAM entry-count gauge only, and documents
the disk-tier bytes/entries gap as a deliberate scope decision with a recommended follow-up
issue, not as something foyer makes impossible.

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

**Disk-tier bytes: not exposed via the accessors `FoyerBackend` already uses — but a
separate, feasible path exists via foyer's metrics registry (see below).** Investigated the
pinned `foyer = "0.22.3"` (`rust/Cargo.lock`) public API reachable from
`HybridCache`/`Store`/`Device`:

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
- `engine/block/manager.rs` has `size()`/`blocks()` methods, but neither tracks live
  occupancy: `Block::size()` (`manager.rs:100`) returns `self.inner.partition.size()`, a
  fixed constant equal to the configured `block_size`; `BlockManager::blocks()`
  (`manager.rs:285`) returns `self.inner.blocks.len()`, the total count of partitions
  carved out once by the startup loop above — a fixed `capacity/block_size` figure, not a
  live count. The actual live-occupancy state (`clean_blocks`/`evictable_blocks`/
  `writing_blocks`/`reclaiming_blocks`) lives in a private `State`/`Inner` struct with no
  public accessor at all, and `BlockManager` itself is not reachable from `Store`'s public
  surface (no `engine()`/`manager()` accessor) — it is a private implementation detail of
  the block engine.

None of `Statistics`, `Device`, or `BlockManager`'s public methods give live disk-tier
occupancy — but that is not the end of the story: foyer *does* already track live
block-level occupancy internally, and exposes it through a mechanism this investigation
initially missed.

**`HybridCacheBuilder::with_metrics_registry()` — a real, unused path to live disk-tier
occupancy.** `HybridCacheBuilder::with_metrics_registry(self, registry: BoxedRegistry)`
(`foyer-0.22.3/src/hybrid/builder.rs:109`) is a public builder method, settable at exactly
the point `FoyerBackend::new` already builds the cache (`foyer_backend.rs:316`,
`HybridCacheBuilder::new()...with_event_listener(listener).memory(ram_bytes)...`) — it must
be called before `.memory()` consumes `self.registry` to construct the internal `Metrics`
struct (`builder.rs:120`). `FoyerBackend` never calls it, so the cache runs with the default
`NoopMetricsRegistry` and none of this data is currently collected.

`foyer_common::metrics::Metrics` (`foyer-common-0.22.3/src/metrics.rs:66-71`) defines exactly
the gauges needed: `storage_block_engine_block_clean`, `_writing`, `_evictable`,
`_reclaiming` (all `BoxedGauge`), plus `storage_block_engine_block_size_bytes`. These are not
inert — `foyer-storage-0.22.3/src/engine/block/manager.rs` updates them at every real
block-state transition, including the transition the plan originally claimed had no signal:
disk-side reclaim completion. `Drop for ReclaimingBlock` (`manager.rs:477-480`) calls
`on_reclaim_finish()` (`manager.rs:356-373`), which decrements `storage_block_engine_block_reclaiming`
and increments `_writing`/`_clean` depending on outcome — i.e. reclaim *does* have a listener
hook, foyer just drives it through its own metrics facade (the `mixtrics` crate's plain,
implementable `RegistryOps`/`GaugeVecOps`/`GaugeOps` traits) rather than a Rust callback
shaped like `EventListener`. A small custom `Registry` adapter that stores these gauge writes
in readable atomics would receive live, drift-free block counts (foyer itself keeps them in
sync — there is no self-tracked-counter drift risk here, unlike the rejected approach in
Trade-offs) and could derive:

- disk-tier bytes, approximately, as `(blocks() - clean) * block_size_bytes`, and
- a disk-tier "entry-like" count as the non-clean block count itself,

both of which are **block-granular, not byte-exact**: blocks default to 16 MiB
(`block_size`), so this is a coarse occupancy proxy, not per-entry accounting.

This path is genuinely feasible — see Trade-offs for why it is deliberately not implemented
in this plan.

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

No changes to disk-tier telemetry in this plan — the existing
`object_cache_foyer_disk_*_bytes_per_sec`/`_ios_per_sec` throughput gauges are unaffected,
and no new disk-tier occupancy gauge is added here. This is a scope decision, not a
technical dead end: a live disk-tier block-occupancy gauge is feasible via foyer's
`with_metrics_registry()` (see Current State / Trade-offs), but implementing it would mean
adding `mixtrics` as a new direct dependency and writing a custom metrics-registry adapter —
a larger, separate change from the one-line RAM-entries gauge this plan is scoped to. See
Trade-offs for the full reasoning and the recommended follow-up issue.

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
   (`:256`), and a short note under/near it that disk-tier bytes/entries are deferred as a
   follow-up rather than unavailable — foyer's `with_metrics_registry()` does expose live
   disk-tier block occupancy, but adopting it needs a new direct `mixtrics` dependency and a
   custom registry adapter, out of scope here (cross-reference this so a future reader
   doesn't file it again as "still missing," and instead finds the follow-up issue).
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

- **Self-tracked disk-bytes counter via our own listener hooks, rejected.** An alternative
  considered was maintaining our own running disk-tier byte counter: increment on each
  RAM→disk write-back (observable via `RamEvictionListener::on_leave`'s `Event::Evict`,
  which already fires per evicted block) and decrement on disk→RAM promotion (observable in
  `promote_if_valid`). Rejected because there is no corresponding *application-level* signal
  for a block being reclaimed *from the disk tier itself* — foyer's own disk-side LRU/region
  reclaim runs entirely inside the block engine with no `EventListener`-shaped callback
  exposed to `FoyerBackend` — so this counter would only ever grow via the
  decrement-on-promotion path and silently drift high of the true value over any run where
  disk-tier capacity pressure causes disk-side eviction without a matching RAM promotion. A
  gauge that quietly overstates occupancy is worse than no gauge. (Note: this is distinct
  from the `with_metrics_registry()` path below — foyer *does* internally observe disk-side
  reclaim completion via `Drop for ReclaimingBlock` → `on_reclaim_finish()`, it just isn't
  surfaced as a callback we can hook into directly; it only reaches us through foyer's own
  metrics facade.)
- **`with_metrics_registry()` + custom `mixtrics` adapter for live disk-tier block
  occupancy — feasible, deliberately deferred.** Unlike the self-tracked counter above, this
  path has no drift risk: foyer maintains `storage_block_engine_block_{clean,writing,
  evictable,reclaiming}` and `storage_block_engine_block_size_bytes` gauges itself, updated
  at every real block-state transition including disk-side reclaim completion (see Current
  State), so a custom registry adapter that just records the gauge writes would stay
  correct by construction. It is deliberately not implemented in this plan, for four
  reasons: (1) it requires adding `mixtrics` as a new *direct* dependency — today it's only
  pulled in transitively through `foyer` (confirmed: not listed in
  `rust/object-cache/Cargo.toml` or the workspace `rust/Cargo.toml`, only present in
  `rust/Cargo.lock` as a transitive entry); (2) it requires implementing a custom
  `RegistryOps`/`GaugeVecOps`/`GaugeOps` adapter to receive foyer's gauge writes — a real
  integration surface, not a one-line gauge emission like the RAM-entries change this plan is
  otherwise scoped to; (3) the result is block-granular (16 MiB default `block_size`), not
  byte-exact residency, so it's a coarser signal than `ram_tier_usage_bytes`; and (4) this
  plan's established scope (see the RAM-entries-per-prefix trade-off below) is deliberately
  kept to a minimal, low-risk, purely-additive change, and this would be neither minimal nor
  low-risk in comparison. Recommendation: file a separate follow-up GitHub issue —
  "object-cache: disk-tier block occupancy via foyer's metrics registry" — rather than
  expanding this plan to cover it.
- **Directory-size-on-disk approximation, rejected.** Periodically `stat`-ing
  `--disk-path` was considered as a filesystem-level proxy for occupancy. Rejected on the
  same evidence as the `Device::allocated()` finding above: `BlockManager::open` carves the
  full configured-capacity partition file up front at startup
  (`manager.rs:186-196`, invoked from `engine.rs:376`), so the region
  file's size is constant (== configured capacity) from first boot regardless of live
  utilization — a directory walk would report the same number whether the tier is empty or
  full.
- **RAM entries per-prefix, out of scope for this change.** The issue allows falling back
  to a global gauge "if per-prefix accounting is feasible, else global" — matching the
  existing global `ram_tier_usage_bytes`. Per-prefix accounting is technically feasible: the
  same insert sites (`FoyerBackend::put`'s Demand arm, `promote_if_valid`) and the same
  `on_leave` exit hook already used by `object_cache_promotion_count` and
  `object_cache_ram_tier_eviction_count` could drive an atomic per-prefix
  increment-on-insert/decrement-on-leave counter via the existing `EvictionTagTable::classify`
  lookup, with no cache scan required. It's left out of this change anyway: this design keeps
  the new gauge a minimal, one-line addition mirroring the existing global
  `ram_tier_usage_bytes`, rather than introducing a new stateful counter with its own
  insert/evict/replace bookkeeping. Kept global, consistent with `ram_tier_usage_bytes`.

## Documentation

- `mkdocs/docs/admin/object-cache.md` Saturation table (`:247-261`): add
  `object_cache_ram_tier_entries` next to `object_cache_ram_tier_usage_bytes`, and a short
  explanatory line that neither disk-tier bytes nor disk-tier entry count is emitted yet —
  not because foyer doesn't expose the data, but as a deliberate scope decision: foyer's
  `with_metrics_registry()` does track live disk-tier block occupancy (including reclaim),
  but surfacing it would require a new direct `mixtrics` dependency and a custom
  metrics-registry adapter, deferred to a follow-up (tracked separately from #1322) rather
  than folded into this change. This should read as "deferred, tracked separately," not "not
  possible."
- `CHANGELOG.md`: one entry for the new gauge.

## Testing Strategy

Add `ram_tier_entries_gauge_reflects_cached_block_count` to
`rust/object-cache-srv/tests/saturation_tests.rs`, modeled on the existing
`ram_tier_usage_gauge_reflects_demand_put` test (lines 167-230) — the harness/helper
boilerplate (a `FoyerBackend`-backed `RangeCache`, `init_in_memory_tracing` guard,
`flush_metrics_buffer`, `integer_metric_values` helper) is shared with
`foyer_disk_gauges_emit_only_after_a_second_tick`, but the fill path is not: this new test
must insert via `FillHint::Demand`, not `Prefetch`. Per `foyer_backend.rs:512-539`,
`Prefetch` fills use an ephemeral phantom record that's dropped immediately and never lands
in `cache.memory()`, so it would never increment `entries()` — `Demand` is what actually
guarantees RAM residency.

- Put N distinct keys into the cache via `FillHint::Demand` (small enough to stay resident
  in the RAM tier given the test's configured `ram_bytes`).
- Call `sample_once`.
- `flush_metrics_buffer()`, then assert `integer_metric_values(&sink,
  "object_cache_ram_tier_entries")` contains exactly one value equal to N.

Regression: existing `saturation_tests.rs` and `foyer_backend_tests.rs` must pass
unchanged — this only adds a new accessor and a new emission, no control-flow changes. Run
`cargo test -p micromegas-object-cache -p micromegas-object-cache-srv --features foyer` and
`cargo clippy --workspace -- -D warnings`.

## Open Questions

None — disk-tier bytes/entries are a deliberately deferred scope decision, with a feasible
path documented for a follow-up (see Current State / Trade-offs), not an open question to
resolve during implementation.
