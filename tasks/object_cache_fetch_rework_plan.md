# Object Cache Read-Path Fetch Rework Plan

Tracking issue: [#1203](https://github.com/madesroches/micromegas/issues/1203). Consolidates the
(now-closed) sub-issues #1193 (run coalescing), #1194 (drop moka), #1199 (priority scheduling),
#1202 (raise/configure concurrency). Unblocks prefetch (#1197/#1198) and the size single-flight +
negative cache (#1196). Follow-up to the range-aware read cache (#1188).

## Overview

The origin-fetch path in `rust/object-cache/src/range_cache.rs` was assembled from four independent
proposals that contradicted each other. This plan replaces it with one coherent design:

- **Single-flight becomes a RangeCache-owned in-flight map** (keyed per block), replacing the moka
  `block_cache`/`size_cache`. Foyer becomes a pure `get`/`put` cache. Owning the map is what lets a
  coalesced run-GET fulfill *every* block it covers, and lets a late demand read **re-prioritize** a
  block that a prefetch started — neither of which a black-box `Foyer::fetch` can do (why #1194's
  "use Foyer.fetch" was dropped).
- **Run coalescing**: contiguous *missing* blocks merge into one `origin.get_range`, bounded by a
  max-coalesced-GET size; the result is split back into per-block cache entries.
- **One global, configurable, priority-aware origin-fetch budget**, replacing the per-call
  `MAX_CONCURRENT_BLOCK_FETCHES = 16`. Demand reads get reserved capacity; prefetch uses spare
  capacity; queued prefetch entries promote to demand when a demand read joins them.
- **Cross-request memory bound** on concurrently-assembled bytes, replacing the per-request-only
  512 MiB cap, so peak RAM stays within an 8 GiB host.

Terminology (kept distinct throughout, per the issue):
- **Coalesce** = merge *distinct adjacent missing* blocks into one run GET.
- **Single-flight / dedup** = collapse concurrent fetches of *the same* block into one (the in-flight
  map). Different mechanism.

## Current State

`RangeCache` (`range_cache.rs:43-73`):
- Holds a 128 MiB moka `block_cache` and a 100k-entry moka `size_cache` on top of `FoyerBackend`,
  which itself has a RAM tier (`RAM_MB`, default 512 MiB). Every block is cached twice; moka exists
  **only** for single-flight via `try_get_with` (`range_cache.rs:126-149`).
- `get_block` (`range_cache.rs:110-151`) issues one `origin.get_range` per 1 MiB block. `get_range`
  (`:185-196`) and `get_ranges` (`:242-253`) fan out per block via `buffer_unordered(16)` — a large
  cold read becomes N separate S3 GETs.
- `MAX_CONCURRENT_BLOCK_FETCHES = 16` (`range_cache.rs:30`) is a per-call cap with no priority and no
  cross-request coordination.
- `size()` (`range_cache.rs:76-108`) dedups only via moka `size_cache`; concurrent misses for the
  same key each `origin.head`. No negative caching.

`FoyerBackend` (`foyer_backend.rs`): now `HybridCache<String, Bytes>` — **#1195 (avoid full-block
copies) is already landed** on this branch (stores/returns `Bytes`, no `Vec<u8>` copy). This plan
only extends it with a fill hint (below); the copy work is done.

`RangeCacheBackend` trait (`backend.rs`): `get(&str) -> Option<Bytes>`, `put(String, Bytes)`.
`MemoryBackend` (`memory_backend.rs`): plain `Mutex<HashMap>`, no single-flight (it never needed it —
single-flight lived in moka).

Handlers (`object-cache-srv/src/handlers.rs`): `MAX_TOTAL_REQUESTED_BYTES = 512 MiB` (`:23`) caps
each request's assembled bytes independently; nothing bounds the sum across concurrent requests.
`get_range`/`get_ranges` assemble the full response in memory (`assemble_range`, and the `BytesMut`
in `post_ranges_handler`).

CLI (`object-cache-srv/src/cli.rs`): env-driven config; no fetch-concurrency, coalescing, or
memory-budget knobs today.

## Design

### Component map

```
RangeCache (Clone; Arc fields)
 ├─ origin: Arc<dyn ObjectStore>
 ├─ backend: Arc<dyn RangeCacheBackend>        (Foyer: pure get/put + fill hint)
 ├─ block_size, ns
 └─ scheduler: Arc<FetchScheduler>             (NEW — replaces both moka caches)
      ├─ inflight: Mutex<HashMap<String, Arc<InFlight>>>   (per-block AND per-meta single-flight)
      ├─ shared_permits:  Semaphore(total)                 (bounds total concurrent origin GETs)
      └─ prefetch_permits: Semaphore(total - demand_reserved)  (caps prefetch fan-out)

AppState (object-cache-srv)
 └─ mem_permits: Arc<Semaphore(memory_budget_mb)>   (NEW — cross-request assembled-bytes bound; §5)
```

The two moka caches are deleted. `moka` is removed from `object-cache/Cargo.toml`. The
cross-request memory bound (`mem_permits`, §5) lives at the server layer, not in `FetchScheduler`:
its guard is an owned permit the handler moves into the axum response `Body`, so it belongs where the
body is built.

### 1. In-flight map (single-flight primitive)

One entry per outstanding fetch, keyed by the same string used for the backend (`blk:{ns}:{key}:{idx}`
for blocks, `meta:{ns}:{key}` for sizes — so the primitive is reusable by `size()` and by #1196).

```rust
#[derive(Copy, Clone, PartialEq, Eq)]
enum Priority { Demand = 0, Prefetch = 1 }   // lower = more urgent

struct InFlight {
    priority: AtomicU8,                       // mutable — a demand joiner can promote it
    promote: Notify,                          // wakes the owner's permit-acquisition loop
    // result slot filled by the owner (run executor), observed by all waiters:
    result: tokio::sync::watch::Sender<Option<Result<Bytes, Arc<anyhow::Error>>>>,
}
```

- **Lookup / ownership** (under the `inflight` mutex, atomic per block): a fetcher either *inserts* a
  new entry for a block (becomes the block's **owner**, responsible for producing its bytes) or finds
  an existing entry and becomes a **joiner** (awaits `result` via a `watch` receiver).
- **Result delivery**: an owner may own several contiguous blocks and fetch them as one run; when the
  run GET completes it splits the bytes and fulfills each owned block's `result` slot. On error, all
  owned slots receive the shared `Err`. The entry is removed from the map once fulfilled (so a later
  read re-checks the backend cache, which the owner has by then populated).
- **Owner cancellation must not strand joiners** (parity with moka's `try_get_with`, whose waiter
  takeover retries when the initializing future is dropped): the owner runs inside an axum request
  future that is dropped on client disconnect, and the `watch::Sender` lives in the map entry — so
  a dropped owner would otherwise leave joiners awaiting a slot nobody will fill, on an entry that
  leaks. Handle it one of two ways: run the origin GET as a detached task (`tokio::spawn`) so it
  completes and fulfills the slots regardless of the requester's lifetime, or install an owner
  drop-guard that removes the owner's unfulfilled entries and signals joiners to re-enter
  own-or-join (one of them becomes the new owner).
- **Watch vs. `Shared` future**: use `watch` (or `OnceCell<Result> + Notify`) rather than a
  `Shared` boxed future, because the fetch is not one-future-per-block — a run owner fulfills many
  slots. `Bytes` is cheap to clone, so broadcasting the result to N waiters is fine. Caveat:
  `watch::Sender::subscribe()` marks the value current at subscribe time as already *seen*, so a
  joiner that subscribes after the owner sent the result would hang in `changed().await`. Joiners
  must check `borrow()` for an already-present result before awaiting `changed()` (or loop with
  `borrow_and_update()`).
- **Error classification must survive the shared slot**: the `Arc<anyhow::Error>` a joiner pulls out
  of `result` must still let `is_not_found` (`validation.rs:36-42`) `downcast_ref::<object_store::Error>()`
  and see `NotFound`, so a missing key stays a 404. The owner therefore stores the origin error
  wrapped such that the `object_store::Error` remains downcastable (e.g. `anyhow::Error::from(origin_err)`).
  A joiner cannot move the owned `anyhow::Error` out of the shared `Arc` (`anyhow::Error` is not
  `Clone`), so it reconstructs an owned error from the shared one: `downcast_ref::<object_store::Error>()`
  on the `Arc`, and if it is `object_store::Error::NotFound`, rebuild a `NotFound` and return
  `anyhow::Error::from(it)` so the 404 downcast survives; other errors get a generic reconstructed
  `anyhow::Error`. Joiners must **not** stringify the shared error. The
  `.map_err(|e| anyhow!("{e}"))` pattern in `get_block` (`range_cache.rs:150`) flattens the error to a
  string and drops the `NotFound` downcast; it must **not** be reused for error propagation here.

This is the concrete reason single-flight must be our own map: a joiner can mutate
`priority`/`promote` on an existing entry. `Foyer::fetch` is a black box and cannot be re-tagged.

### 2. Run coalescing

Given the missing block indices a request **owns** (won ownership of), sorted ascending:

1. Group into maximal contiguous runs.
2. Split any run whose byte span exceeds `max_coalesced_get_bytes` into sub-runs at block
   boundaries.
3. Each (sub-)run = one `origin.get_range(path, run_start..run_end)` = one budget permit.
4. Split the returned bytes at block boundaries; `backend.put` each block and fulfill its in-flight
   slot.

Already-cached blocks and blocks owned by *another* concurrent request are never part of a run (they
are cache hits or joins) — so cached blocks are never refetched, and coalescing only ever merges
genuinely-missing, self-owned, adjacent blocks. Scattered blocks (common for cross-object prefetch)
don't coalesce and fall back to per-block GETs — concurrency is the lever there, not coalescing.

### 3. Global priority-aware fetch budget

Two `tokio::sync::Semaphore`s enforce a single shared ceiling with reserved demand headroom:

- **`shared_permits`** = `total` (default sized to the NIC, below). *Every* origin GET holds one, so
  total concurrent GETs ≤ `total`.
- **`prefetch_permits`** = `total - demand_reserved`. Prefetch GETs must hold **both** a
  `prefetch_permit` *and* a `shared_permit`; demand GETs hold **only** a `shared_permit`.

Invariants:
- Total concurrency ≤ `total` (everyone holds a shared permit).
- Prefetch concurrency ≤ `total - demand_reserved` (bounded by the smaller pool).
- Therefore demand always finds ≥ `demand_reserved` shared permits free → **demand is never starved
  by prefetch**. Deadlock-free: demand never touches `prefetch_permits`, and prefetch acquires
  `prefetch_permit` before `shared_permit` (consistent order).

**Permit acquisition with promotion** — the run owner does not pre-commit to a class; it loops on the
entry's *current* priority so a promotion mid-wait takes effect:

```
loop {
    if entry.priority == Demand {
        let shared = shared_permits.acquire().await;   // demand path: shared only
        return DemandPermit(shared);
    } else {                                            // prefetch path
        select! {
            class = prefetch_permits.acquire()  => {
                select! {
                    shared = shared_permits.acquire() => return PrefetchPermit(class, shared),
                    _ = entry.promote.notified()       => { drop(class); continue; } // re-check as demand
                }
            }
            _ = entry.promote.notified() => { continue; } // re-check: now Demand
        }
    }
}
```

For a run owning multiple blocks, the run's effective priority is the most-urgent (min) of its
blocks' priorities, and while parked it selects over **all of its owned blocks' per-entry `promote`
signals** (merged, e.g. `select_all`/`FuturesUnordered`) — not a single signal shared across the
run. Entries are created per block under the `inflight` mutex *before* coalescing partitions the
owned blocks into runs, so run membership isn't known at entry creation; and a signal shared
across several sub-run owners would lose wakeups exactly as §4 describes (`notify_one` stores one
permit for at most one waiter). A joiner always fires the demanded block's own entry signal (§4),
which wakes the owner of whichever run covers that block; the owner then re-evaluates its run's
min priority.

**Why per-GET, not per-block, permits**: the budget bounds concurrent *origin GETs* (a coalesced run
is one GET), which is what maps to NIC bandwidth. This is what makes NIC-sizing meaningful.

### 4. Prefetch → demand promotion dynamics

Both sub-parts are required (building only one silently stalls demand reads):

| State when a demand read arrives for a block | Action |
|---|---|
| **Already cached** | Instant hit. No priority question. |
| **In flight** (origin GET executing) | Attach to the entry, ride it to completion. An in-flight S3 GET can't be preempted or accelerated — nothing to move. |
| **Queued, not started** | **Promote**: set `priority = Demand`, `promote.notify_one()`. The owner's loop re-evaluates, drops the prefetch-class requirement, competes for a reserved demand permit, and starts now. |

Concrete payoff (issue's example): prefetch submits 100 scattered blocks; with the shipped defaults
(`total=32`, `demand_reserved=8`) `prefetch_permits = 24` → 24 in flight, 76 queued. A demand read
hits a queued block:
- *With promotion*: block flips to Demand, takes a reserved permit, starts now — latency ≈ one GET.
- *Reserved-only, no promotion*: the reserved permits sit idle (no demand-*classified* fetch exists
  to use them) and the block waits behind up to 76 prefetch blocks.

**Promotion granularity policy** (`promote_whole_batch`, default `false`):
- Default (precise): promote only the run(s) covering the demanded block. Because coalescing bounds a
  run to `max_coalesced_get_bytes`, promoting a run is a bounded, contiguous elevation — and demand
  readahead/coalescing tends to cover the neighbours anyway.
- Optional (anticipatory): promote the entire prefetch *batch* (all blocks submitted in one prefetch
  call). Can re-elevate blocks the query never touches; off by default.
- Mechanism for the batch case: every `InFlight` entry created by one prefetch call carries a shared
  batch handle (`Arc<BatchState>` holding the sibling entries' keys or weak refs); when
  `promote_whole_batch` is on, a demand joiner sets `priority = Demand` on each sibling entry via
  the handle and fires **each sibling's own per-entry `promote.notify_one()`** — not a batch-wide
  shared `Notify`. One `Notify` shared by several parked run owners loses wakeups: `notify_one`
  stores a permit for at most one waiter, and `notify_waiters` stores none, so an owner that read
  `priority == Prefetch` just before the joiner's store and then parks on `notified()` would miss
  the promotion until a prefetch permit happens to free. Per-entry `notify_one` is race-free —
  each entry's promote signal has exactly one owner waiting on it, and the stored permit covers
  the check-then-park window. The acquisition loop (§3) is unchanged: it only ever waits on its
  own entries' promote signal. Demand-created entries carry no handle.

(Note: "batch" = the set of keys submitted in one prefetch call; "run" = one coalesced GET. Default
promotion is run-scoped, not batch-scoped.)

### 5. Cross-request memory bound

Add a global assembled-bytes budget, quantized to blocks to keep permit counts small. It lives on the
server `AppState` (not `FetchScheduler`): the guard is an owned permit the handler moves into the
response `Body`, so it belongs at the HTTP layer where the body is assembled — and this keeps it off
the `RangeCache::new` constructor:

- `mem_permits: Arc<Semaphore(memory_budget_mb)>` where 1 permit ≈ 1 MiB.
- A request acquires `ceil(assembled_bytes / 1 MiB)` permits **before** assembling and holds them
  until the assembled `Bytes` is fully sent. The guard is moved into the response `Body` (a wrapper
  that owns the permit and drops it when the body is flushed) so the bound covers the response's
  full lifetime, not just the assembly window.
- **What the budget counts**: response-body bytes. The transient assembly peak is higher — ~2× for
  `get_range` (the block map is live while `assemble_range` copies into the result) and ~3× for
  `POST /ranges` (block map + per-range copies + the `BytesMut` framing buffer in
  `post_ranges_handler`) — so worst-case RAM attributable to request assembly is ~2–3× the budget.
  `memory_budget_mb` must be sized with that multiplier in mind (the 1024 MiB default → ~2–3 GiB
  worst-case transient, still comfortable on an 8 GiB host next to the 512 MiB Foyer RAM tier).
- The existing per-request `MAX_TOTAL_REQUESTED_BYTES` (512 MiB) stays as a per-request sanity cap;
  the new budget is the cross-request ceiling. A request larger than the whole budget is rejected
  (413) rather than deadlocking on unavailable permits.
- Prefetch does **not** draw from this budget: its transient buffer is bounded by
  `prefetch_concurrency × max_coalesced_get_bytes` and it discards bytes after `backend.put`.

Alternative considered: stream large responses block-by-block (no full-object buffer, removes the
per-request cap). Larger change to `assemble_range`/handlers; noted as a future option, not this
change (see Trade-offs).

### 6. Backend trait: fill hint (footnote from #1199)

Extend the trait so prefetch fills land as low-priority in Foyer's RAM tier and don't evict hot
demand data:

```rust
#[derive(Copy, Clone)]
pub enum FillHint { Demand, Prefetch }

async fn put(&self, key: String, value: Bytes, hint: FillHint);
```

- `FoyerBackend`: `Demand → insert`, `Prefetch → insert_with_hint(.., CacheHint::Low)` (both exist in
  foyer 0.14). `get` unchanged. **The RAM tier should be explicitly built with LRU eviction**
  (`HybridCacheBuilder::memory().with_eviction_config(LruConfig::default())`): in foyer 0.14.1 only
  LRU maps `CacheHint::Low → LruHint::LowPriority`; Lfu (w-TinyLFU), S3Fifo, and Fifo silently discard
  the hint. Foyer's *code* default is already LRU (`CacheBuilder::new` sets
  `LruConfig::default()`; the doc-comment claiming a w-TinyLFU default is stale), and `FoyerBackend`
  never overrides it — so the hint is already honored today. Setting `LruConfig` explicitly pins the
  policy defensively (against a future foyer default change) rather than switching away from
  w-TinyLFU.
- `MemoryBackend`: ignores the hint. No single-flight needed here anymore (it lived in moka; now it
  lives in `FetchScheduler`).

### 7. Priority plumbing through the public API

Introduce one internal core used by both range methods:

```rust
async fn fetch_blocks(&self, key, file_size, indices: &[u64], prio: Priority)
    -> Result<HashMap<u64, Bytes>>;
```

- `get_range` / `get_ranges` call it with `Priority::Demand` (public signatures unchanged). Only the
  `Demand` path accumulates the per-block bytes into the returned map for assembly.
- On the `Prefetch` path the core does **not** accumulate: as each owned run completes it
  `backend.put`s every block and drops the bytes immediately, returning an empty map. This is what
  keeps the prefetch peak bounded by `prefetch_concurrency × max_coalesced_get_bytes` (§5) rather than
  the full request size — a prefetch of N blocks never holds all N live at once.
- A `pub(crate)` prefetch entry point (`prefetch_ranges`/`prefetch_blocks`) calls it with
  `Priority::Prefetch`, warms the cache, and returns no assembled bytes. The prefetch **endpoint and
  client method are #1198** — this plan defines the priority-carrying core they build on, not the
  HTTP surface.
- `size()` is wrapped in the same in-flight primitive (dedup concurrent `head`s). **The single-flight
  indirection must preserve `object_store::Error::NotFound` to every waiter** so `is_not_found`
  (`validation.rs:36-42`, reached from `handlers.rs:43`/`:66`/`:263`) still downcasts it and returns
  404 for a missing key — today `size()` propagates the origin error unwrapped (`range_cache.rs:99`),
  and the shared-slot error must keep the `object_store::Error` downcastable (see §1). Do **not** reuse
  `get_block`'s stringifying `.map_err(|e| anyhow!("{e}"))` (`range_cache.rs:150`) for `size()`, which
  would turn a 404 into a 500. **Negative caching of NotFound is deferred to #1196**; only dedup and
  error-fidelity are done here (dedup is required because removing moka removes `size_cache`).

## Implementation Steps

### Phase 1 — In-flight primitive + drop moka (single-flight parity)
1. `range_cache.rs`: add `FetchScheduler` with the `inflight` map and `InFlight` entry type
   (`Priority`, `AtomicU8`, `promote: Notify`, `watch` result slot). Add lookup/own/join helpers.
2. Rewrite `get_block` → block-level own/join against the map; owner fetches via `origin.get_range`
   (still per-block for now), `backend.put`, fulfills slot; joiner awaits `watch`.
3. Fold `size()` into the primitive (dedup concurrent heads). Remove both moka caches and the `moka`
   dependency from `object-cache/Cargo.toml`.
4. `MemoryBackend`: unchanged behaviourally (drop any single-flight assumption).
5. Tests: add a counting/instrumented origin store; assert *N concurrent identical block reads → one
   origin GET*, and *N concurrent `size()` misses → one `head*` (replaces the moka-backed guarantee).
   Also: *owner cancelled mid-fetch → joiner still completes* (drop the owning read future while a
   joiner waits; the joiner must get bytes, not hang — covers the §1 cancellation handling).

### Phase 2 — Run coalescing
6. Add `coalesce_runs(sorted_missing_owned: &[u64], block_size, max_coalesced_get_bytes)
   -> Vec<Range<u64>>` in `blocks.rs`, with unit tests in `tests/blocks_tests.rs` (contiguous merge,
   gap split, oversize split, single block, scattered) — per the project convention that unit tests
   live under the crate's `tests/` folder.
7. `fetch_blocks`: compute missing → own → coalesce owned runs → one GET per run → split → put +
   fulfill per block. In this phase `max_coalesced_get_bytes` is a hardcoded const (8 MiB, the
   table default) — it becomes a `RangeCache::new` param in step 11 and a CLI flag in step 16.
   Route `get_range`/`get_ranges` through `fetch_blocks`. This deletes the `buffer_unordered`
   calls at `range_cache.rs:194`/`:251`, but **keep `MAX_CONCURRENT_BLOCK_FETCHES` as an interim
   cap** — execute the owned runs through `buffer_unordered(MAX_CONCURRENT_BLOCK_FETCHES)` — so a
   scattered cold read (which doesn't coalesce and stays per-block) never fans out unbounded in
   the Phase-2-only state; the global budget that supersedes it arrives in Phase 3 (the const and
   its `buffer_unordered` are removed in step 9).
8. Tests: cold contiguous read = few coalesced GETs (assert GET count/spans on the counting store);
   partially-cached read never refetches cached blocks; scattered read stays per-block.

### Phase 3 — Global priority budget
9. `FetchScheduler`: add `shared_permits` and `prefetch_permits`; implement the promotion-aware
   acquisition loop. Owner acquires a run permit before the GET. Remove the interim
   `MAX_CONCURRENT_BLOCK_FETCHES` cap from step 7 and the const at `range_cache.rs:30` in this
   step — the budget now bounds fan-out, and leaving the const would fail `clippy -D warnings`
   (dead_code).
10. Add `Priority` param to `fetch_blocks`; `get_range`/`get_ranges` pass `Demand`; add the
    `pub(crate)` prefetch core with `Prefetch`.
11. Add the config params (`total`, `demand_reserved`, `max_coalesced_get_bytes`,
    `promote_whole_batch`) to `RangeCache::new`, and update every existing caller in this same phase —
    passing hardcoded default values directly — so the workspace (and Phase 3's own tests) keeps
    compiling: the production caller (`object-cache-srv/src/object_cache_srv.rs`) and the three test
    call sites in `object-cache/tests/range_cache_tests.rs` (the `make_cache` helper plus the two
    inline `RangeCache::new` constructions). Phase 5 only folds these values behind CLI/env flags.
12. Tests: demand not starved (saturate prefetch, assert a demand GET starts within reserved
    capacity); promotion (queued prefetch block flips to demand and starts before remaining prefetch);
    total concurrency never exceeds `total` (instrumented semaphore/counter).

### Phase 4 — Memory bound + fill hint
13. Add `mem_permits: Arc<Semaphore>` to the server `AppState` (object-cache-srv), sized from
    `memory_budget_mb` — **not** to `FetchScheduler` or `RangeCache::new` (keeps the Phase 3 four-param
    constructor and its call sites unchanged). Handlers acquire from it sized to the assembled bytes
    and move the owned guard into the response `Body` wrapper. Reject requests larger than the whole
    budget with 413.
14. Extend `RangeCacheBackend::put` with `FillHint`; `FoyerBackend` maps `Prefetch → CacheHint::Low`.
    Thread the hint through the in-source `backend.put` call sites in `range_cache.rs`: `fetch_blocks`
    passes `FillHint::Demand` on the demand path and `FillHint::Prefetch` on the prefetch path at its
    per-run put sites, and `size()`'s put passes `FillHint::Demand`. (Without this the hint is a no-op —
    every fill would land as `Demand`.) Update `tests/foyer_backend_tests.rs`'s three `backend.put(...)`
    calls to pass a `FillHint`.
15. Tests: concurrent large reads block on the budget rather than OOM; body drop releases permits;
    oversize-vs-budget → 413. These are handler-level tests (the budget lives on the server
    `AppState`, §5 — the object-cache crate never sees it), so they go in a new
    `object-cache-srv/tests/memory_budget_tests.rs`; add a small `[lib]` target to object-cache-srv
    exposing the handler/state modules so `tests/` can import them (mirrors `analytics-web-srv`'s
    lib+bin split; the `[[bin]]` keeps its `object_cache_srv.rs` entry path).

### Phase 5 — Config + docs
16. `object-cache-srv/src/cli.rs`: add env/CLI flags (defaults below), and update `object_cache_srv.rs`
    to source the `RangeCache::new` config values from those flags instead of the Phase 3 hardcoded
    defaults. Validate at startup (next to the existing `block_size == 0` check,
    `object_cache_srv.rs:42`) that `demand_reserved < total` — `Semaphore::new(total -
    demand_reserved)` would underflow and panic on a misconfigured pair — and that `total > 0`.
17. Update `mkdocs/docs/admin/object-cache.md` env table and `object-cache-srv/README.md`.
18. `python3 ../build/rust_ci.py`; local MinIO smoke test (`start_minio.py` + `start_services.py`).

### New configuration

| Env var | Default | Meaning |
|---|---|---|
| `MICROMEGAS_OBJECT_CACHE_MAX_CONCURRENT_FETCHES` | `32` | Total concurrent origin GETs (NIC-sized for im4gn.large ~few Gbps; a starting point to be measured, not a blind 256). |
| `MICROMEGAS_OBJECT_CACHE_DEMAND_RESERVED_FETCHES` | `8` | Origin-GET slots always available to demand; prefetch is capped at `total - reserved`. |
| `MICROMEGAS_OBJECT_CACHE_MAX_COALESCED_GET_BYTES` | `8388608` (8 MiB) | Max span of one coalesced run GET; larger contiguous runs are split. |
| `MICROMEGAS_OBJECT_CACHE_MEMORY_BUDGET_MB` | `1024` | Cross-request cap on concurrently-assembled bytes (well under 8 GiB alongside the 512 MiB Foyer RAM tier). |
| `MICROMEGAS_OBJECT_CACHE_PROMOTE_WHOLE_BATCH` | `false` | On a demand hit into a prefetch batch, promote the whole batch (anticipatory) vs. only the covering run (default, precise). |

Default sizing rationale: at 1 MiB blocks and ~75 ms S3 GET latency, one concurrent GET ≈ ~110 Mbps;
~28 concurrent saturates ~3 Gbps, so `total = 32` is a defensible NIC-shaped default (coalescing
raises effective throughput per permit further). All values are configurable and meant to be tuned
against measurement.

## Files to Modify

- `rust/object-cache/src/range_cache.rs` — remove moka; add `FetchScheduler`, `InFlight`, `Priority`,
  `fetch_blocks`, coalescing wiring, prefetch core, size single-flight; thread `FillHint` through its
  `backend.put` call sites (`fetch_blocks` demand/prefetch puts, `size`'s put).
- `rust/object-cache/src/blocks.rs` — add `coalesce_runs`.
- `rust/object-cache/tests/blocks_tests.rs` — `coalesce_runs` unit tests.
- `rust/object-cache/src/backend.rs` — `FillHint` param on `put`.
- `rust/object-cache/src/foyer_backend.rs` — honor `FillHint` (`CacheHint::Low` for prefetch).
- `rust/object-cache/tests/foyer_backend_tests.rs` — pass `FillHint` to the three `put` calls.
- `rust/object-cache/src/memory_backend.rs` — `FillHint` signature (ignored).
- `rust/object-cache/Cargo.toml` — drop `moka`.
- `rust/object-cache/tests/range_cache_tests.rs` — single-flight, coalescing, priority tests (add a
  counting origin store helper).
- `rust/object-cache-srv/Cargo.toml` + `src/lib.rs` — NEW `[lib]` target exposing handler/state
  modules to integration tests (mirrors `analytics-web-srv`; bin entry path unchanged).
- `rust/object-cache-srv/tests/memory_budget_tests.rs` — NEW: memory-budget handler tests (gating,
  permit release on body drop, oversize → 413).
- `rust/object-cache-srv/src/cli.rs` — new flags/env.
- `rust/object-cache-srv/src/object_cache_srv.rs` — pass config to `RangeCache::new`.
- `rust/object-cache-srv/src/handlers.rs` — memory-budget acquisition + body-guard wrapper.
- `mkdocs/docs/admin/object-cache.md`, `rust/object-cache-srv/README.md` — env docs.

## Trade-offs

- **Own in-flight map vs. `Foyer::fetch`**: `fetch` is simpler but a black box — it cannot let a
  run-GET fulfill many blocks, nor let a late demand joiner re-prioritize a prefetch-started fetch.
  Both are load-bearing here (#1194's `fetch` proposal was dropped for exactly this).
- **Two semaphores vs. one custom priority scheduler**: two semaphores give the reserved-demand
  guarantee with a provable ceiling and no hand-rolled fairness. Promotion needs a small re-check
  loop on top; a fully custom scheduler would be more flexible but far more error-prone.
- **Memory-budget gate vs. streaming responses**: the gate is a localized change that bounds
  response-body bytes for the response's full lifetime (transient assembly overhead runs ~2–3× the
  counted bytes; see §5). Streaming would remove the per-request cap entirely and lower peak
  further, but reworks assembly and the HTTP body path — deferred as a follow-up.
- **Per-GET permits vs. per-block permits**: per-GET maps to NIC bandwidth (the real ceiling) and
  keeps coalescing meaningful; per-block would double-count coalesced runs.
- **Run-scoped promotion default**: precise (won't warm untouched blocks) at the cost of not
  anticipating the rest of a batch; the whole-batch knob covers the anticipatory case.

## Documentation

- `mkdocs/docs/admin/object-cache.md`: add the five new env vars to the table; add a short
  "Fetch scheduling & memory bounds" subsection (demand-over-prefetch priority, coalescing, budget).
- `rust/object-cache-srv/README.md`: mirror the env additions.
- Update the `RangeCache` module doc comment to describe the in-flight map and priority model (the
  write-once/no-invalidation note stays valid).

## Testing Strategy

- **Unit** (`tests/blocks_tests.rs`): `coalesce_runs` edge cases (merge, gap-split, oversize-split, scattered).
- **Integration** (`range_cache_tests.rs`, with an instrumented origin store counting GETs/heads and
  recording spans):
  - Single-flight: N concurrent identical block reads → 1 GET; N concurrent `size()` misses → 1 head;
    owner cancelled mid-fetch → joiner still completes (no hang, no leaked in-flight entry).
  - Coalescing: cold contiguous read → few GETs with expected spans; oversize run split at
    `max_coalesced_get_bytes`; partially-cached read never refetches cached blocks; scattered stays
    per-block.
  - Priority: demand not starved under prefetch saturation; queued prefetch block promotes and starts
    before remaining prefetch; total concurrent GETs never exceed `total`.
  - Correctness preserved: existing `get_range`/`get_ranges`/`size` byte-equality tests still pass.
- **Server integration** (`object-cache-srv/tests/memory_budget_tests.rs`, handler-level via the new
  `[lib]` target): concurrent large reads gate rather than exceed the budget; permit released on
  body drop; request larger than the whole budget → 413.
- **CI**: `cd rust && python3 ../build/rust_ci.py` (fmt, clippy `-D warnings`, tests).
- **Smoke**: `start_minio.py` + `start_services.py`; identical query results cache-on vs. bypass.

## Scope decisions (resolved)

- **Budget defaults accepted** (`total=32`, `demand_reserved=8`, `max_coalesced_get_bytes=8 MiB`,
  `memory_budget_mb=1024`): ship as configurable defaults and tune against measurement later — no
  pre-benchmark gate.
- **size()**: dedup single-flight only (forced by moka removal). Negative caching stays out — #1196.
- **Memory guard**: ride the permit on the response `Body` (bounds body bytes for their full
  lifetime; assembly transients run ~2–3× the counted bytes, see §5). No looser interim.
- **Prefetch surface**: keep `fetch_blocks` priority-parameterized; the `pub(crate)` prefetch entry
  point and HTTP surface land in #1198. This plan is the demand path + the primitives prefetch reuses.
