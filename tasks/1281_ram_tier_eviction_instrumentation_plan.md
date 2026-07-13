# Object-Cache Eviction Instrumentation Plan (RAM + disk tiers)

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1281

## Overview

The object cache emits only aggregate occupancy (`object_cache_ram_tier_usage_bytes`) — nothing
about *what* gets evicted or *how long it lived* before leaving a tier. That makes it impossible to
distinguish **thrashing** (entries leaving moments after they arrived, i.e. the tier is undersized)
from **normal working-set turnover** (entries that lived a reasonable while). This plan adds
event-driven eviction/age signals for **both** the RAM tier and the disk tier:

1. `object_cache_ram_tier_eviction_count` — counter tagged by eviction `reason`
   (`evict`/`replace`/`remove`/`clear`) and `prefix`; makes RAM evictions visible at all.
2. `object_cache_ram_tier_eviction_age_ms` — float/timer tagged by `prefix`, emitted on
   capacity-driven RAM eviction (`Event::Evict`); measures RAM residency.
3. `object_cache_disk_tier_read_age_ms` — float/timer tagged by `prefix`, emitted when an entry is
   served **from the disk tier** (`Source::Disk`); measures how long an entry sat on disk before it
   left the disk tier by promotion. See **Disk-tier limitation** for why this (not a reclaim-time
   age) is the observable disk-exit signal foyer 0.22 exposes.

All three are dogfooded through the standard tracing sink, queryable like every other cache metric.

## Current State

### RAM tier construction (`rust/object-cache/src/foyer_backend.rs`)

- `FoyerBackend` wraps a `HybridCache<String, Bytes>` (`:40`).
- `new_with_shards` (`:48`) builds the cache with `.with_weighter(|_key, value| value.len())`
  (`:71`) and `.with_eviction_config(LruConfig::default())` (`:77`). It never calls
  `with_event_listener`, so RAM-tier evictions are invisible.
- `get` (`:105`) returns `entry.value().clone()`; `put` inserts either via
  `storage_writer(key).force().insert(value)` (prefetch, disk-only, `:129`) or
  `self.cache.insert(key, owned)` (demand, `:147`).
- `ram_usage()` reads `self.cache.memory().usage()` (`:99`) — unchanged by this work.

### foyer's eviction/tier hooks (foyer 0.22 / foyer-memory 0.22.3 / foyer-storage 0.22.3)

- **RAM tier listener**: `HybridCacheBuilder::with_event_listener(Arc<dyn EventListener<K, V>>)`
  (`hybrid/builder.rs:82`), registered on the in-memory `CacheBuilder` inside `.memory()` (`:124`).
  `EventListener::on_leave(&self, reason: Event, key, value)` (`foyer-common/src/event.rs:38`);
  `Event` ∈ `Evict`/`Replace`/`Remove`/`Clear` (`:19`). Confirmed: `on_leave` is invoked **only**
  from `foyer-memory` (`raw.rs:399,510,614,643,668`) — it is a RAM-tier event exclusively.
- **Hybrid policy**: `HybridCachePolicy::default() == WriteOnEviction` (`hybrid/cache.rs:180`) — an
  entry is written to disk **when it is evicted from RAM**. So under the current default, the RAM
  `Event::Evict` moment *is* the disk-write moment.
- **Entry source**: `HybridCacheEntry::source() -> Source` (`foyer-memory/src/cache.rs:351`) returns
  `Source::Memory` / `Source::Disk` / `Source::Outer`. A disk-tier hit yields `Source::Disk`
  (verified in foyer's own test, `hybrid/cache.rs:1571`). This lets `FoyerBackend::get` detect,
  per read, whether the value came off disk.
- **Disk reclaim has no per-entry hook**: the block reclaimer
  (`foyer-storage/src/engine/block/reclaimer.rs:103`) scans a victim block, **copies raw byte
  slices** for reinserted entries and drops the rest via `indexer.remove_batch` — it never decodes
  `V` and fires no listener. So entries reclaimed from disk **without ever being read back are not
  observable per-entry**. Reinsertion during reclaim is a raw byte copy (no re-encode), so any
  timestamp embedded in the payload survives compaction unchanged.

### Disk-tier limitation, and estimating reclaim age from read age

The only *observable* way an entry leaves the disk tier is by promotion on a disk hit
(`Source::Disk`), where `FoyerBackend::get` sees it and has the key for prefix tagging. Reclaim —
the capacity-driven disk eviction — is invisible per-entry (raw-byte block recycling, no decode, no
callback). `object_cache_disk_tier_read_age_ms` therefore measures *disk residency at read*.

**Reclaim age is estimable from read age.** An entry can only be *read* from disk while it is still
resident, so the **maximum observed disk read-age is a lower bound on the disk residency horizon** —
i.e. roughly how old entries get before their block is reclaimed. Entries reclaimed unread are
simply ≥ that age. So `max()` / high quantiles (p95/p99) of `object_cache_disk_tier_read_age_ms`
estimate the reclaim age directly, **without a foyer patch**: a small max ("disk items are pulled
back almost immediately, then recycled") signals disk thrashing; a large max ("items sit for a long
time before either being read or recycled") signals healthy turnover — the same thrashing-vs-turnover
distinction the issue asks for, now on the disk tier.

For that estimate to work, the metric must be emitted as a **per-read measurement**, not a
pre-aggregated mean — micromegas stores each `fmetric` sample as a point, so query-time `max()`/
quantiles recover the tail. (This plan emits one `fmetric` per disk read; §4.) Aggregate reclaim
*volume* remains covered by the foyer disk `Statistics` already surfaced through
`disk_stats`/`object_cache_foyer_disk_*` (`saturation_monitor.rs`). An *exact* per-entry reclaim-age
would still require a foyer hook, but the read-age tail makes that unnecessary for the operator
question (see Open Questions).

### Value serialization constraint

`HybridCache<K, V>` requires `V: StorageValue = Value + Code` (`foyer-common/src/code.rs:43`). The
`serde` feature is **not** enabled, so `Bytes` gets a hand-written `Code` impl (`code.rs:208`). Any
wrapper value type must implement `Code`. `std::time::Instant` is not serializable and is
process-local, so **RAM age uses an in-memory `Instant`**, while **disk age uses a wall-clock
millis timestamp embedded in the encoded payload** (`chrono` is already an object-cache dep).

### Metric tagging (`rust/object-cache/src/metric_tags.rs`)

- `PrefixTags` (`:31`) precomputes interned `&'static PropertySet`s per prefix label at construction
  so hot sites do an array lookup instead of allocating + intern-locking per call.
- `longest_prefix_match(labels, key)` (`:84`) classifies a key to a prefix index (equal-or-`/`
  boundary rule); unmatched keys fall back to `PREFIX_OTHER` (`"other"`, `:22`).
- Classification lives in `RangeCache` today (`range_cache/mod.rs:142`), **not** in the backend.

### Wiring (`rust/object-cache-srv/src/object_cache_srv.rs`)

- `FoyerBackend::new_with_shards(...)` is built at `:143`, **before** `allowed_prefixes` is resolved
  (`:168`) and the `'static` `prefix_labels` are leaked + attached via `with_prefix_labels`
  (`:196`–`201`). To give the backend prefix labels, this ordering must change (Implementation).

## Design

### 1. Value wrapper carrying both a RAM `Instant` and a disk wall-clock

```rust
/// RAM-tier cache value carrying the timing needed for eviction/age telemetry.
/// - `ram_inserted_at`: when the entry (re-)entered the RAM tier. Set on `new()`
///   and refreshed on `Code::decode` (a disk->RAM promotion is a *new* RAM
///   residency), so RAM age always measures time resident in RAM. Not serialized.
/// - `disk_write_ms`: wall-clock ms (epoch) when the entry was written to disk.
///   Serialized into the payload by `Code::encode` (which stamps "now" — under
///   the default `WriteOnEviction` policy encode runs at disk-write time), and
///   preserved verbatim through disk reclaim (raw-byte reinsertion, no re-encode).
///   `DISK_WRITE_NONE` for a RAM-only entry that has never been persisted.
/// - `is_prefetch`: true only for the ephemeral phantom record created by the
///   prefetch `put` arm. Not serialized (always `false` on decode). foyer 0.22.3
///   fires `on_leave` *twice* for that phantom record — `Event::Remove`
///   synchronously during `insert`, then `Event::Evict` when the ephemeral
///   handle is dropped (the disk-write dispatch) — both at age ≈ 0 ms. The
///   listener (§3) uses this marker to exclude that noise from both signals.
#[derive(Clone)]
struct CachedBlock {
    bytes: Bytes,
    ram_inserted_at: Instant,
    disk_write_ms: i64,
    is_prefetch: bool,
}

const DISK_WRITE_NONE: i64 = i64::MIN;

impl CachedBlock {
    fn new(bytes: Bytes) -> Self {
        Self {
            bytes,
            ram_inserted_at: Instant::now(),
            disk_write_ms: DISK_WRITE_NONE,
            is_prefetch: false,
        }
    }
    /// Ephemeral disk-only phantom record for the prefetch path (see field doc).
    fn new_prefetch(bytes: Bytes) -> Self {
        Self { is_prefetch: true, ..Self::new(bytes) }
    }
}

impl foyer::Code for CachedBlock {
    fn encode(&self, writer: &mut impl std::io::Write) -> foyer::Result<()> {
        // Stamp the disk-write instant here: encode == disk write under
        // WriteOnEviction. Leading i64 LE, then the payload.
        let now_ms = chrono::Utc::now().timestamp_millis();
        now_ms.encode(writer)?;             // foyer's numeric Code impl (LE)
        self.bytes.encode(writer)
    }
    fn decode(reader: &mut impl std::io::Read) -> foyer::Result<Self> {
        let disk_write_ms = i64::decode(reader)?;
        let bytes = Bytes::decode(reader)?;
        Ok(Self { bytes, ram_inserted_at: Instant::now(), disk_write_ms, is_prefetch: false })
    }
    fn estimated_size(&self) -> usize {
        std::mem::size_of::<i64>() + self.bytes.estimated_size()
    }
}
```

Ripple changes in `foyer_backend.rs`:
- `cache: HybridCache<String, CachedBlock>`.
- weighter: `|_key, value: &CachedBlock| value.bytes.len()` (RAM budget still counts payload bytes;
  the extra 8 disk bytes are disk-tier only and don't affect RAM accounting).
- demand `put`: `self.cache.insert(key, CachedBlock::new(owned))`.
- prefetch `put`: `storage_writer(key).force().insert(CachedBlock::new_prefetch(value))`
  (disk-only path; its encode stamps the disk-write time, exactly what we want for later disk
  read-age; `is_prefetch: true` lets the listener exclude the phantom `on_leave` firings this path
  triggers — see §3).
- `get` (see §4).

### 2. Shared prefix-tag classifier (`metric_tags.rs`)

Both the RAM eviction listener and `FoyerBackend::get` need to classify a key to a prefix and reach
precomputed tag sets, so factor the table out (mirrors how `RangeCache` holds `PrefixTags`):

```rust
pub const REASON_EVICT: &str = "evict";
pub const REASON_REPLACE: &str = "replace";
pub const REASON_REMOVE: &str = "remove";
pub const REASON_CLEAR: &str = "clear";

#[derive(Debug, Clone, Copy)]
pub struct EvictionTags {
    pub label: &'static str,
    pub prefix: &'static PropertySet,        // {prefix} — used by both age metrics
    count_evict: &'static PropertySet,       // {prefix, reason=evict}
    count_replace: &'static PropertySet,
    count_remove: &'static PropertySet,
    count_clear: &'static PropertySet,
}
impl EvictionTags {
    pub fn new(label: &'static str) -> Self { /* find_or_create each */ }
    /// `count_for(REASON_*)`; unknown value falls back to the evict set.
    pub fn count_for(&self, reason: &'static str) -> &'static PropertySet { /* match */ }
}

/// Precomputed table shared (via `Arc`) between the eviction listener and the
/// backend's `get`. `classify` reuses `longest_prefix_match`, so the matching
/// rule is not duplicated.
pub struct EvictionTagTable {
    labels: Arc<[&'static str]>,
    tags: Arc<[EvictionTags]>,   // parallel to `labels`
    other: EvictionTags,         // PREFIX_OTHER fallback
}
impl EvictionTagTable {
    pub fn new(labels: Arc<[&'static str]>) -> Self { /* map EvictionTags::new + other */ }
    pub fn classify(&self, key: &str) -> &EvictionTags { /* longest_prefix_match or &other */ }
}
```

`metric_tags` stays free of any `foyer` import (it compiles without the `foyer` feature). Mapping
`Event -> REASON_*` lives in `foyer_backend.rs`.

### 3. RAM eviction listener (`foyer_backend.rs`)

```rust
struct EvictionListener {
    tags: Arc<EvictionTagTable>,
}
impl foyer::EventListener for EvictionListener {
    type Key = String;
    type Value = CachedBlock;
    fn on_leave(&self, reason: Event, key: &String, value: &CachedBlock) {
        if value.is_prefetch {
            // Phantom prefetch record: foyer fires Remove (synchronously during
            // `insert`) then Evict (when the ephemeral handle is dropped, i.e.
            // the disk-write dispatch) for the *same* disk-only write, both at
            // age ≈ 0 ms — indistinguishable from real thrashing if counted.
            // Exclude from both the count and the age signal.
            return;
        }
        let t = self.tags.classify(key);
        imetric!("object_cache_ram_tier_eviction_count", "count",
                 t.count_for(reason_str(reason)), 1_u64);
        if reason == Event::Evict {  // capacity-driven — the thrashing signal
            let age_ms = value.ram_inserted_at.elapsed().as_secs_f64() * 1000.0;
            fmetric!("object_cache_ram_tier_eviction_age_ms", "ms", t.prefix, age_ms);
        }
    }
}
```

**Hot-path constraint**: `on_leave` runs synchronously inside foyer's insert path, possibly under a
shard lock. It must be allocation-free and non-blocking — satisfied by precomputed
`&'static PropertySet`s (array lookup, no intern-lock), `Instant::elapsed`, and the thread-local
`imetric!`/`fmetric!` queues. No async/IO.

### 4. Disk read-age in `get` (`foyer_backend.rs`)

```rust
async fn get(&self, key: &str) -> Option<Bytes> {
    match self.cache.get(key).await {
        Ok(Some(entry)) => {
            if entry.source() == foyer::Source::Disk {
                let v = entry.value();
                if v.disk_write_ms != DISK_WRITE_NONE {
                    let age_ms = (chrono::Utc::now().timestamp_millis() - v.disk_write_ms) as f64;
                    let t = self.tags.classify(key);
                    fmetric!("object_cache_disk_tier_read_age_ms", "ms", t.prefix, age_ms.max(0.0));
                }
            }
            Some(entry.value().bytes.clone())
        }
        Ok(None) => None,
        Err(e) => { /* unchanged: metric + warn + treat as miss */ }
    }
}
```

`source() == Disk` fires exactly once per disk read (a subsequent hit on the now-promoted entry
reports `Source::Memory`), so no double counting. `FoyerBackend` gains a `tags: Arc<EvictionTagTable>`
field (the same `Arc` handed to the listener).

**One sample per read (not aggregated).** Emitting a distinct `fmetric` per disk read is deliberate:
it preserves the full read-age distribution so a query-time `max()`/`quantile()` recovers the disk
residency horizon (the reclaim-age estimate; see "Disk-tier limitation" above). Do not average or
downsample at emission time.

### 5. Wiring prefix labels into the backend (`object_cache_srv.rs`)

The listener/table needs the `'static` prefix labels at build time. Reorder so labels are resolved
first and shared:

```
resolve allowed_prefixes ─┐
leak prefix_labels ───────┼─► FoyerBackend::new_with_shards(.., prefix_labels.clone())
                          └─► RangeCache::new(..).with_prefix_labels(prefix_labels)
```

`FoyerBackend::new_with_shards` gains a `prefix_labels: Arc<[&'static str]>` param, builds
`Arc<EvictionTagTable>`, gives one clone to the `EvictionListener` (registered via
`with_event_listener` before `.memory()`) and keeps one on the backend for `get`. `FoyerBackend::new`
passes an empty slice → everything classifies as `"other"` (matching `RangeCache::new`'s default).

## Implementation Steps

### Phase 1 — value wrapper (`foyer_backend.rs`)
1. Add `CachedBlock` + `DISK_WRITE_NONE` + `Code` impl (LE i64 disk-write ms then payload; decode
   refreshes `ram_inserted_at`, preserves `disk_write_ms`).
2. Swap the cache value type to `CachedBlock`; update weighter and both `put` arms.

### Phase 2 — tag table (`metric_tags.rs`)
3. Add `REASON_*` constants, `EvictionTags` (`new` + `count_for`), and `EvictionTagTable`
   (`new` + `classify`). Keep the module foyer-free.

### Phase 3 — listener + get (`foyer_backend.rs`)
4. Add `EvictionListener` + `reason_str(Event)`; add `tags: Arc<EvictionTagTable>` field on
   `FoyerBackend`; emit disk read-age in `get` on `Source::Disk`.
5. Add `prefix_labels` param to `new_with_shards`; build the shared `Arc<EvictionTagTable>`,
   register the listener before `.memory()`. Update `new` to pass an empty slice.

### Phase 4 — wiring (`object_cache_srv.rs`)
6. Move `allowed_prefixes`/`prefix_labels` resolution above the backend build; pass `prefix_labels`
   into `new_with_shards`; reuse the same `Arc` for `with_prefix_labels`.

### Phase 5 — call-site + test updates
7. Update `FoyerBackend::new_with_shards` call sites (tests) to pass a labels slice
   (`Arc::from(Vec::new())` where prefix tagging isn't exercised).

## Files to Modify

- `rust/object-cache/src/foyer_backend.rs` — `CachedBlock` + `Code`, value-type change,
  `EvictionListener`, disk read-age in `get`, `new_with_shards` signature + listener registration.
- `rust/object-cache/src/metric_tags.rs` — `REASON_*`, `EvictionTags`, `EvictionTagTable`.
- `rust/object-cache-srv/src/object_cache_srv.rs` — reorder prefix-label resolution, pass labels.
- `rust/object-cache/tests/foyer_backend_tests.rs` — new arg + RAM-eviction & disk-read-age asserts.
- `rust/object-cache-srv/tests/saturation_tests.rs`, `prefetch_tests.rs` — new constructor arg.

## Trade-offs

- **Two timestamps (`Instant` + wall-clock ms) vs. one**: RAM residency wants a monotonic
  process-local `Instant` (cheap, immune to clock skew) but that can't cross the disk boundary; disk
  residency wants a value that survives serialization and disk reclaim, so it must be a wall-clock
  millis stamp. Carrying both is 24 B/RAM-entry (an `Instant` ~16 B + `i64` 8 B) and 8 B/disk-entry.
  Using one wall-clock for both would make RAM age vulnerable to clock adjustment and lose the
  clean "fresh residency on decode" semantics. Chosen: both.
- **Disk read-age vs. true disk reclaim-age**: foyer 0.22 exposes no per-entry disk-eviction hook
  and recycles blocks as raw bytes without decoding, so an *exact* reclaim-age is not obtainable
  without patching foyer. But the reclaim age is *estimable*: since an entry is only readable while
  still on disk, the max/high-quantile of the disk read-age distribution is a lower bound on the
  disk residency horizon (the reclaim age). Emitting per-read samples of
  `object_cache_disk_tier_read_age_ms` thus answers the thrashing question for the disk tier without
  a foyer change. Chosen: per-read disk read-age; exact per-entry reclaim-age deferred as unnecessary
  unless the estimate proves insufficient (Open Questions).
- **Wrapper `CachedBlock` vs. side map of insertion times**: a side `HashMap` needs its own eviction
  lifecycle and hot-path locking — a race-prone shadow of foyer's own structure. The wrapper
  co-locates timing with the value so foyer manages its lifetime for free. Chosen: wrapper.
- **Age on `Evict` only vs. all reasons**: `Replace`/`Remove`/`Clear` don't speak to capacity
  pressure; emitting age for them would dilute the thrashing signal. The `count` metric still covers
  all four reasons (for real RAM residents) so total RAM churn stays visible. Chosen: RAM age on
  `Evict`, count on all — except the prefetch phantom record, which is excluded from *both* signals
  entirely via the `is_prefetch` marker (§3/§1): foyer 0.22.3 fires `on_leave` twice for that
  ephemeral disk-only write (`Remove` at insert, `Evict` at drop), both at age ≈ 0 ms, which is
  otherwise indistinguishable from genuine thrashing.
- **Listener classifies keys itself vs. calling back into `RangeCache`**: the listener/`get` live in
  the backend and can't reach `RangeCache`'s classifier; they reuse the shared
  `longest_prefix_match` + `EvictionTagTable`, so matching logic isn't duplicated — only a small
  precomputed table is held in a second place (as `RangeCache` already does). Consistent.
- **Dependency on foyer's default `WriteOnEviction` hybrid policy**: `Code::encode` stamps
  `disk_write_ms` as "now", which is only equal to the disk-write moment because
  `HybridCachePolicy::default() == WriteOnEviction` (`hybrid/cache.rs:180`); this plan pins no policy
  change. If the policy is ever switched to `WriteOnInsertion`, `disk_write_ms` would instead mark
  insertion time and disk read-age would become "age since insertion" rather than "disk residency" —
  revisit this design if `with_policy` is ever touched.

## Documentation

- No user-facing doc currently catalogs object-cache metrics (they're self-describing via the
  tracing sink). If a metrics reference is later added under `mkdocs/`, list the three new
  `object_cache_ram_tier_eviction_*` / `object_cache_disk_tier_read_age_ms` metrics with their
  `reason`/`prefix` dimensions. No existing page needs updating for this change.

## Testing Strategy

- **RAM eviction (`foyer_backend_tests.rs`)**: build a `FoyerBackend` with a tiny `ram_bytes` and
  real prefix labels; insert enough demand entries to force capacity eviction; assert
  `object_cache_ram_tier_eviction_count{reason=evict, prefix=…}` fired and
  `object_cache_ram_tier_eviction_age_ms{prefix=…}` fired with a plausible (> 0) value. Reuse
  `InMemorySink`/`init_in_memory_tracing` from `micromegas_tracing` (see `telemetry_tests.rs`) —
  these are library types and reachable from any crate. `integer_metric_values`/
  `float_metric_values`, however, are private free functions inside the `object-cache-srv`
  `saturation_tests` integration-test binary; `foyer_backend_tests.rs` lives in a different crate
  (`object-cache`) as a separate test binary with no import path to them, so duplicate both helpers
  into `foyer_backend_tests.rs` (or promote them to a shared library location) instead of importing
  them.
- **Disk read-age**: with a small RAM tier and a real disk tier, insert a key (force RAM eviction so
  it lands on disk), then `get` it back and assert `entry.source()` took the disk path and
  `object_cache_disk_tier_read_age_ms{prefix=…}` fired ≥ 0. (May need a brief settle after eviction
  for the disk write to complete; follow the existing disk-tier test patterns in
  `foyer_backend_tests.rs`.)
- **`Code` round-trip**: unit-test `CachedBlock` encode→decode preserves `bytes` and the
  `disk_write_ms` stamped at encode, and yields a fresh `ram_inserted_at`.
- **Regression**: existing `foyer_backend_tests` (demand/prefetch residency, RAM-usage) must pass
  unchanged after the value-type swap — confirms the weighter still counts payload bytes and the
  8-byte disk prefix doesn't leak into RAM accounting.
- Run `cargo test -p micromegas-object-cache -p micromegas-object-cache-srv --features foyer` and
  `cargo clippy --workspace -- -D warnings`.

## Open Questions

1. **Exact disk reclaim-age**: the reclaim age is estimated here from the max/high-quantile of the
   per-read disk read-age distribution (see "Disk-tier limitation"), which needs no foyer change.
   An *exact* per-entry reclaim-age would still require a foyer-side hook — a reclaimer/storage
   event listener, or decoding entries during reclaim — and is worth an upstream issue/PR only if
   the estimate proves insufficient (e.g. a workload that rarely re-reads disk entries, starving the
   read-age tail). **Recommendation**: ship the read-age estimate + aggregate reclaim stats; revisit
   only if the estimate is demonstrably blind.
