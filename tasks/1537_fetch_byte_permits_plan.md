# Byte-Denominated Origin-Fetch Budget Plan

Issue: #1537

## Overview
The range cache's origin-fetch scheduler throttles by a **count** of permits, so the worst-case
transient memory of in-flight origin GETs is `max_concurrent_fetches * max_coalesced_get_bytes`.
Raising `MICROMEGAS_OBJECT_CACHE_MAX_CONCURRENT_FETCHES` for throughput — a knob whose help text
says nothing about memory — silently multiplies that ceiling (128 permits × 8 MiB ≈ 1 GiB). The
response-path `MICROMEGAS_OBJECT_CACHE_MEMORY_BUDGET_MB` does not cover these buffers, so it
reported ~100 of 256 MB occupied while a production process grew ~1.9 GB and was OOM-killed.
This plan adds a byte-denominated budget to the fetch scheduler, with the same demand/prefetch
reservation structure the count budget has, so the fetch path gets a hard MiB ceiling that does
not move when the concurrency knob moves. Concurrency stays as a parallelism cap.

## Current State
- `rust/object-cache/src/range_cache/scheduler.rs:151-196` — `FetchScheduler` holds two count
  semaphores: `shared_permits` (size `total`) and `prefetch_permits` (size
  `total - demand_reserved`). Every run takes one shared permit; an all-prefetch run also takes
  one prefetch permit.
- `scheduler.rs:445-483` — `acquire_run_permit`: promotion-aware loop. While every entry is
  prefetch it waits on `prefetch` then `shared`, each raced against `any_entry_promoted`; once any
  entry is demand it takes only `shared`. Returns `RunPermit { _shared, _prefetch }`
  (`scheduler.rs:417-420`).
- `scheduler.rs:238-245` — `fetch_budget_stats()` returns a 4-tuple of count occupancy, consumed by
  `rust/object-cache-srv/src/saturation_monitor.rs:48-68` as the
  `object_cache_fetch_{shared,prefetch}_{occupancy,available}` gauges.
- `rust/object-cache/src/range_cache/fetch.rs:261-358` — `spawn_run_fetch` computes the run's
  `byte_start..byte_end` only *after* acquiring the permit (`fetch.rs:316-317`), and drops the
  permit right after the origin GET returns (`fetch.rs:331`) — *before* `fulfill_run_success`
  (`fetch.rs:365-430`) slices the buffer and `backend.put`s each block. `FoyerBackend::put`
  (`rust/object-cache/src/foyer_backend.rs:549-586`) does `Bytes::copy_from_slice` per block, so
  the GET buffer and its copy coexist for the whole put loop with no permit held at all.
- `rust/object-cache/src/blocks.rs:21-41` — `coalesce_runs` caps a run at
  `max(max_coalesced_get_bytes / block_size, 1)` blocks, so the largest possible run is
  `max(floor(max_coalesced / block_size), 1) * block_size` bytes (≥ `block_size` even when
  `max_coalesced_get_bytes < block_size`).
- `rust/object-cache/src/range_cache/mod.rs:35-45` — defaults: `DEFAULT_TOTAL_FETCH_PERMITS = 32`,
  `DEFAULT_DEMAND_RESERVED_FETCH_PERMITS = 8`, `DEFAULT_MAX_COALESCED_GET_BYTES = 8 MiB`.
  `RangeCache::new` (`mod.rs:100-125`) forwards the counts to `FetchScheduler::new`.
- Callers of `RangeCache::new`: `object-cache-srv/src/object_cache_srv.rs:134-143`,
  `object-cache/src/l1_store.rs:100-109` (`L1_TOTAL_FETCH_PERMITS = 16`, demand-only), and many
  tests in `object-cache/tests/{range_cache,telemetry,metric_tags}_tests.rs` and
  `object-cache-srv/tests/{prefetch,memory_budget,shutdown_sequence,telemetry,saturation}_tests.rs`.
- `rust/object-cache-srv/src/cli.rs:73-108,192-207` — the concurrency knobs, their validation,
  and the response-path `memory_budget_mb` (1 MiB `mem_permits`, `handlers.rs:38-43`).

## Design

### Two budgets, one shape
Extract the shared/prefetch semaphore pair into a reusable type and instantiate it twice:

```rust
// scheduler.rs
struct PriorityBudget {
    shared: Arc<Semaphore>,
    shared_total: usize,
    prefetch: Arc<Semaphore>,   // shared_total - reserved
    prefetch_total: usize,
}

impl PriorityBudget {
    fn new(total: usize, demand_reserved: usize) -> Self;          // asserts reserved < total
    async fn acquire(&self, n: u32, entries: &[Arc<InFlight>]) -> BudgetPermit;
    fn stats(&self) -> BudgetStats;
}

pub struct BudgetStats { pub shared_available: usize, pub shared_total: usize,
                         pub prefetch_available: usize, pub prefetch_total: usize }
pub struct FetchBudgetStats { pub count: BudgetStats, pub bytes: BudgetStats }
```

`acquire` is today's `acquire_run_permit` loop verbatim, with `acquire_owned()` replaced by
`acquire_many_owned(n)`. `FetchScheduler` holds `count: PriorityBudget` (one permit per run) and
`bytes: PriorityBudget` (one permit per byte).

- **One permit = one byte.** A run charges exactly its length. tokio's semaphore capacity
  (`usize::MAX >> 3`) is effectively unbounded on 64-bit; the only constraint is that
  `acquire_many_owned` takes a `u32`, so a single run must be ≤ `u32::MAX` bytes (~4 GiB) —
  enforced at startup (see Constructor and validation). Coarser units (e.g. the 1 MiB
  `mem_permits` unit) would overcharge small runs (~43 KB observed average) for no benefit.
- **Byte reservation is derived, not a new knob:**
  `demand_reserved_bytes = demand_reserved_fetches * max_run_bytes`. Each reserved demand slot can
  hold one full-size run, which is exactly what the count reservation already promises. At
  defaults: 8 × 8 MiB = 64 MiB reserved, 192 MiB prefetch cap — identical to today's effective
  bound.
- **New knob:** `--fetch-memory-budget-mb` / `MICROMEGAS_OBJECT_CACHE_FETCH_MEMORY_BUDGET_MB`,
  default `256` (= 32 × 8 MiB, today's default worst case, so defaults change nothing *when every
  other fetch knob is also at its default*). A deployment that raised `DEMAND_RESERVED_FETCHES`,
  `MAX_COALESCED_GET_BYTES`, or `BLOCK_SIZE` (a larger block size can itself push `max_run_bytes`
  past the 256 MiB floor, per `coalesce_runs`'s per-block-size rounding) past what the 256 MiB
  floor allows will fail `Cli::validate` at startup; a deployment that raised
  `MAX_CONCURRENT_FETCHES` no longer gets more fetch memory for it and must set the new knob
  explicitly. See the CHANGELOG/admin-doc upgrade note.

### Acquisition in `acquire_run_permit`
```rust
pub(super) struct RunPermit { pub count: BudgetPermit, pub bytes: BudgetPermit }

pub(super) async fn acquire_run_permit(s: &FetchScheduler, entries, run_bytes: u64) -> RunPermit {
    let n = u32::try_from(run_bytes).expect("run size validated <= u32::MAX at construction");
    let bytes = s.bytes.acquire(n, entries).await;
    let count = s.count.acquire(1, entries).await;
    RunPermit { count, bytes }
}
```
- **Bytes first, then count.** Every run acquires in the same order, so there is no hold-and-wait
  cycle.
- A promotion that lands after the byte permit was taken with a prefetch component does not
  re-acquire it: the held prefetch-pool bytes are merely over-restrictive for that run's lifetime,
  and the count acquisition that follows sees the promotion normally.
- tokio's `Semaphore` is FIFO for `acquire_many`, so a large run at the head of the queue is not
  starved by a stream of small ones.

### Permit lifetime in `spawn_run_fetch`
- Hoist the `byte_start`/`byte_end` computation above the permit acquisition and pass
  `byte_end - byte_start` to `acquire_run_permit`.
- Release `permit.count` right after the origin GET (unchanged semantics: it bounds origin
  concurrency). Hold `permit.bytes` until after `fulfill_run_success` / the error fulfill loop, so
  the byte budget covers the GET buffer through the per-block `backend.put` copies. Once the task
  ends, remaining references to the buffer are either demand callers' block maps (covered by the
  response-path `mem_permits`) or prefetch entries that `join_prefetch` drops as they complete.
- Charge is 1× run bytes (see Trade-offs).

### Shared sizing helper
Add `pub fn max_run_bytes(block_size: u64, max_coalesced_get_bytes: u64) -> u64` to `blocks.rs`
(`(max_coalesced_get_bytes / block_size).max(1) * block_size`) and have `coalesce_runs` use the
same expression, so the scheduler assertion, the reservation derivation, and CLI validation all
agree with what `coalesce_runs` actually produces.

### Constructor and validation
- `RangeCache::new` gains `fetch_memory_budget_bytes: u64` after `demand_reserved_fetch_permits`
  (positional, so the compiler enumerates every call site). It computes
  `max_run = max_run_bytes(block_size, max_coalesced)` and builds `FetchScheduler::new(total,
  demand_reserved, budget_bytes, reserved_bytes, max_run, promote_whole_batch)`.
- `FetchScheduler::new` asserts `max_run <= u32::MAX` and `budget_bytes - reserved_bytes >=
  max_run` (with `reserved_bytes < budget_bytes`). Without the latter, `acquire_many_owned` on a
  run larger than the prefetch pool never completes and never errors — the same hang the
  `memory_budget_mb` floor in `cli.rs:216-231` guards against.
- `Cli::validate` adds the fatal-at-startup counterparts: `fetch_memory_budget_mb > 0`,
  `max_run_bytes(block_size, max_coalesced_get_bytes) <= u32::MAX`, and (via `checked_mul` on both
  the `fetch_memory_budget_mb * MiB` and `(demand_reserved_fetches + 1) * max_run_bytes` products,
  rejecting overflow as a validation error) `fetch_memory_budget_mb * MiB >=
  (demand_reserved_fetches + 1) * max_run_bytes`, with error messages naming
  `MICROMEGAS_OBJECT_CACHE_FETCH_MEMORY_BUDGET_MB`,
  `MICROMEGAS_OBJECT_CACHE_DEMAND_RESERVED_FETCHES`,
  `MICROMEGAS_OBJECT_CACHE_MAX_COALESCED_GET_BYTES`, and `MICROMEGAS_OBJECT_CACHE_BLOCK_SIZE` (all
  four feed the floor via `max_run_bytes`) and the computed floor. It also rejects any
  `fetch_memory_budget_mb` whose byte value exceeds
  `usize::MAX >> 3` (tokio's `Semaphore::MAX_PERMITS`), since `FetchScheduler::new` hands out one
  permit per byte and `Semaphore::new` panics above that limit.
- New `pub const DEFAULT_FETCH_MEMORY_BUDGET_BYTES: u64 = DEFAULT_TOTAL_FETCH_PERMITS as u64 *
  DEFAULT_MAX_COALESCED_GET_BYTES;` in `range_cache/mod.rs`; the CLI default derives its MiB value
  from it.
- `L1CacheStore` passes `L1_TOTAL_FETCH_PERMITS * DEFAULT_MAX_COALESCED_GET_BYTES` (128 MiB) as a
  new `L1_FETCH_MEMORY_BUDGET_BYTES` const, replacing the "roughly `L1_TOTAL_FETCH_PERMITS *
  DEFAULT_MAX_COALESCED_GET_BYTES`" prose with an actual bound.

### Telemetry
- `RangeCache::fetch_budget_stats()` returns `FetchBudgetStats`.
- `saturation_monitor::sample_once` keeps the four existing count gauges unchanged and adds
  `object_cache_fetch_mem_shared_occupancy_mb`, `object_cache_fetch_mem_shared_available_mb`,
  `object_cache_fetch_mem_prefetch_occupancy_mb`, `object_cache_fetch_mem_prefetch_available_mb`
  (bytes / MiB, unit `"megabytes"`, matching `object_cache_mem_budget_*_mb`). These are the gauges
  that would have shown pressure in the reported incident.
- `range_cache_fetch_permit_wait_ms` keeps measuring the whole `acquire_run_permit`, now covering
  both budgets.

## Implementation Steps
1. `blocks.rs`: add `max_run_bytes`; use it in `coalesce_runs`.
2. `range_cache/scheduler.rs`: extract `PriorityBudget` / `BudgetPermit` / `BudgetStats` from the
   existing fields and loop; add `FetchBudgetStats`; give `FetchScheduler` `count` and `bytes`
   budgets; rewrite `acquire_run_permit` (bytes then count) and `fetch_budget_stats`; add the
   budget assertions to `FetchScheduler::new`.
3. `range_cache/mod.rs`: add `DEFAULT_FETCH_MEMORY_BUDGET_BYTES`; new `RangeCache::new` parameter
   and reservation derivation; re-export `FetchBudgetStats`/`BudgetStats`; update the
   `fetch_budget_stats` return type and doc.
4. `range_cache/fetch.rs`: hoist the byte-range computation, pass run bytes into
   `acquire_run_permit`, drop `permit.count` after the GET and `permit.bytes` after fulfillment.
   Update the `fetch_blocks` doc's "`prefetch_concurrency * max_coalesced_get_bytes`" bound (and
   the matching sentence on `join_prefetch`) to name the prefetch byte pool.
5. `l1_store.rs`: add `L1_FETCH_MEMORY_BUDGET_BYTES`, pass it, update the const docs.
6. `object-cache-srv/src/cli.rs`: add `fetch_memory_budget_mb: u64`; reword
   `max_concurrent_fetches` help ("parallelism cap; transient fetch memory is bounded separately by
   `--fetch-memory-budget-mb`"); extend `Cli::validate`.
7. `object-cache-srv/src/object_cache_srv.rs`: pass `fetch_memory_budget_mb * 1024 * 1024`; safe
   because `Cli::validate` has already rejected values that would overflow `u64` or exceed the
   semaphore's permit limit.
8. `object-cache-srv/src/saturation_monitor.rs`: consume `FetchBudgetStats`, emit the four new
   gauges.
9. Update every test call site of `RangeCache::new` (pass `DEFAULT_FETCH_MEMORY_BUDGET_BYTES`
   unless the test is about the byte budget), including `saturation_tests.rs` (constructs a
   `RangeCache` directly); `cli_tests.rs` builds its `Cli` via `Cli::parse_from` and needs no
   change from a new `RangeCache::new` parameter.
10. Add the tests in Testing Strategy; docs and CHANGELOG.

## Files to Modify
- `rust/object-cache/src/blocks.rs`
- `rust/object-cache/src/range_cache/scheduler.rs`
- `rust/object-cache/src/range_cache/mod.rs`
- `rust/object-cache/src/range_cache/fetch.rs`
- `rust/object-cache/src/l1_store.rs`
- `rust/object-cache-srv/src/cli.rs`
- `rust/object-cache-srv/src/object_cache_srv.rs`
- `rust/object-cache-srv/src/saturation_monitor.rs`
- `rust/object-cache/tests/range_cache_tests.rs`, `telemetry_tests.rs`, `blocks_tests.rs`,
  `metric_tags_tests.rs`
- `rust/object-cache-srv/tests/cli_tests.rs`, `prefetch_tests.rs`, `saturation_tests.rs`,
  `memory_budget_tests.rs`, `shutdown_sequence_tests.rs`, `telemetry_tests.rs`
- `mkdocs/docs/admin/object-cache.md`, `mkdocs/docs/architecture/caching.md`
- `rust/object-cache-srv/README.md`
- `CHANGELOG.md`

## Trade-offs
- **Replace the count budget with bytes only** — rejected. With ~43 KB average runs, a 256 MiB
  byte-only budget would admit thousands of concurrent GETs, pushing the concern onto the origin
  client's connection pool and the NIC. Count remains the right bound for parallelism; bytes is the
  right bound for memory.
- **Single byte semaphore, no demand reservation in bytes** — rejected. Prefetch could then occupy
  the whole byte budget and demand would queue behind it even with count slots free, reintroducing
  the starvation the count reservation exists to prevent.
- **Reuse `--memory-budget-mb` (response path) for fetches** — rejected. A request holds its
  response permits while its fetch waits; drawing the fetch from the same pool is a hold-and-wait
  deadlock under saturation. It also measures a different thing (streaming windows, which for
  demand overlap the same buffers).
- **Separate `--demand-reserved-fetch-memory-mb` knob** — rejected in favor of deriving it from
  `demand_reserved_fetches * max_run_bytes`: one fewer knob to mis-set, and it keeps the existing
  reservation's meaning ("N full-size demand runs always fit").
- **Charge 2× run bytes to cover the foyer copy** — rejected. The copy is accounted by foyer's
  budgets once inserted; holding the 1× permit across the put loop already bounds the overlap.
- **1 MiB units, reusing `permits_for_bytes`** — rejected; see the one-permit-per-byte rationale
  above.

## Documentation
- `mkdocs/docs/admin/object-cache.md`: add `MICROMEGAS_OBJECT_CACHE_FETCH_MEMORY_BUDGET_MB` to the
  env-var table and `--fetch-memory-budget-mb` to the CLI-flag table; reword the
  `MAX_CONCURRENT_FETCHES` row to say it is a parallelism cap, not a memory knob; extend "Fetch
  scheduling & memory bounds" with the two-budget model (fetch budget bounds origin-GET buffers
  through backend admission; `--memory-budget-mb` bounds response streaming windows; peak transient
  ≈ their sum) and the startup floor; add the four `object_cache_fetch_mem_*_mb` gauges to the
  Saturation table; add an upgrade note that a config with `DEMAND_RESERVED_FETCHES`,
  `MAX_COALESCED_GET_BYTES`, or `BLOCK_SIZE` raised past the new 256 MiB floor will now fail to
  start, and that raising `MAX_CONCURRENT_FETCHES` no longer buys more fetch memory — such
  deployments must also set `MICROMEGAS_OBJECT_CACHE_FETCH_MEMORY_BUDGET_MB`.
- `mkdocs/docs/architecture/caching.md:96-99`: one sentence that the shared fetch budget is bounded
  in both concurrency and bytes.
- `rust/object-cache-srv/README.md`: new flag row; reworded `--max-concurrent-fetches` row.
- `CHANGELOG.md` (Unreleased): bug fix entry for #1537, with a **Minor breaking change** clause for
  `RangeCache::new`'s new parameter and `fetch_budget_stats()` returning `FetchBudgetStats`, plus an
  operator-facing upgrade note (mirroring the admin-doc note above) that non-default
  `DEMAND_RESERVED_FETCHES`/`MAX_COALESCED_GET_BYTES`/`BLOCK_SIZE` configs may now fail validation
  at startup and that `MAX_CONCURRENT_FETCHES` no longer implies more fetch memory.

## Testing Strategy
All no-DB unit/integration tests using the existing `CountingStore` gate and `MemoryBackend`.

- **Regression for the OOM (#1537)** — `range_cache_tests.rs`,
  `byte_budget_caps_in_flight_bytes_when_count_allows_more`: block size 1 KiB, run size 4 blocks
  (`max_coalesced = 4 KiB`), count total 32 / reserved 1, byte budget 12 KiB (reserved 4 KiB;
  demand reads draw on the full 12 KiB shared pool). Launch 8 disjoint 4-block demand reads with the gate closed; assert
  exactly 3 GETs reach origin (count alone would allow 8) and that `peak_in_flight` stays 3 after
  yielding; open the gate, all reads complete with correct bytes. This is the in-the-wild bug: a
  high count authorizing proportionally more buffer.
- **Demand not starved by prefetch saturating bytes** — prefetch runs fill the prefetch byte pool
  (count ample); a demand read still reaches origin via the reserved bytes, and a queued prefetch
  does not take them. Mirrors `demand_not_starved_under_prefetch_saturation` in the byte
  dimension.
- **Promotion drops the prefetch byte requirement** — mirror of
  `promotion_lets_demand_start_before_remaining_prefetch` with the byte pool, not the count pool,
  as the binding constraint.
- **Byte permit held through backend admission** — a `RangeCacheBackend` test double whose `put`
  blocks on a gate: after the origin GET completes, `fetch_budget_stats().count` shows the count
  slot released while `.bytes` still shows the run's charge; releasing the put gate returns the
  bytes. Pins the lifetime change in `spawn_run_fetch`.
- **Constructor assertions** — `#[should_panic]` for a budget whose prefetch pool is smaller than
  one max run.
- **`blocks_tests.rs`** — `max_run_bytes` for `max_coalesced` a multiple of, not a multiple of,
  and smaller than `block_size`, and agreement with the longest run `coalesce_runs` emits.
- **`cli_tests.rs`** — rejects `fetch_memory_budget_mb = 0`; rejects a `max_coalesced_get_bytes`
  whose `max_run_bytes` exceeds `u32::MAX`; rejects a budget below
  `(demand_reserved + 1) * max_run_bytes`; accepts the defaults.
- **`saturation_tests.rs`** — `sample_once` emits the four `object_cache_fetch_mem_*_mb` gauges
  with values reflecting a held run permit.
- Existing scheduler tests (`total_concurrency_never_exceeds_total`, the two priority tests, the
  prefetch-endpoint budget test in `object-cache-srv/tests/prefetch_tests.rs`) must pass unchanged
  in behavior with the default byte budget.
