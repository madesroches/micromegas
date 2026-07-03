# Object Cache Prefetch Endpoint + Client Method Plan

## Implementation status: IMPLEMENTED (branch not merged)

Every phase is implemented on this branch (`prefetch.rs`, `prefetch_queue.rs`, `prefetch_handler`,
the `FoyerBackend::put` SSD-only branch, the size-trust guard, the client `prefetch` method/trait,
CLI knobs, docs, tests). The streaming refactor called for in the previous revision of this status
note is done: `prefetch_queue.rs`'s consumer streams lazy `WINDOW_BLOCKS`-sized block-index windows
(§3) instead of collecting the full index list, the `item.size > MAX_TOTAL_REQUESTED_BYTES`
rejection is deleted from `prefetch_handler` (§4), and the size-cap docs text and the "oversized
`size` rejected" test case are replaced with streaming/stop-on-error coverage (§ Testing Strategy).
There is now no per-item size ceiling anywhere in the request path.

`FoyerBackend::put`'s prefetch branch bumps `range_cache_prefetch_admission_unexpected_none` and
logs a warning on the defensive (should-never-fire) case where `.force().insert(value)` returns
`None`; it is in the metrics list (§ Implementation Steps step 10) and the docs' metrics table.

Before merge, see **Open Questions #4**: whether `MAX_PREFETCH_KEYS_PER_REQUEST` (the 4096-key batch
cap, §4 step 2) is still a necessary limit now that streaming has removed the analogous per-item
size cap, or whether it bounds a genuinely different cost that streaming doesn't touch.

Tracking: [#1198](https://github.com/madesroches/micromegas/issues/1198) — part of
[#1197](https://github.com/madesroches/micromegas/issues/1197) (prefetch support). Builds on the
now-merged read-path rework ([#1203](https://github.com/madesroches/micromegas/issues/1203) / PR
#1216, plan in `tasks/completed/object_cache_fetch_rework_plan.md`).

## Overview

Expose the prefetch-priority fill path that already exists in `RangeCache` over HTTP, plus a client
method to drive it. This activates a subsystem that is currently dead in production: the priority
scheduler, promotion machinery, and `FillHint::Prefetch` were all built by #1203 but have no caller
outside integration tests. A `POST /prefetch` endpoint accepts a batch of keys (whole-object or
specific ranges), enqueues them at prefetch priority behind a bounded queue, and returns `202
Accepted` immediately without blocking on the fetch. The two triggers that will call it —
query-layer warming (#1200) and write-time partition warming (#1201) — are out of scope here; this
issue delivers the reusable surface they build on.

## Current State

- **Core fill path exists and is `pub`** (`rust/object-cache/src/range_cache.rs:862-896`):
  - `prefetch_ranges(&self, key, ranges) -> Result<()>` — resolves size, validates bounds, computes
    the block set, drives `fetch_blocks(.., Priority::Prefetch)`.
  - `prefetch_blocks(&self, key, file_size, indices) -> Result<()>`.
  - Both return no bytes; the prefetch arm of `fetch_blocks` (`range_cache.rs:697-716`) writes to the
    backend and drops the bytes as each run completes, so peak RAM is bounded by
    `prefetch_concurrency * max_coalesced_get_bytes`, not the request size.
  - Priority is enforced by `FetchScheduler`: `prefetch_permits` = `total - demand_reserved`
    (`range_cache.rs:175`), and a demand joiner promotes a prefetch entry via `own_or_join`
    (`range_cache.rs:200-206`). Prefetched blocks currently land in foyer's RAM tier at
    `CacheHint::Low` (`FoyerBackend::put` → `insert_with_hint`, `foyer_backend.rs:75-77`); §7 of this
    plan changes prefetch to SSD-only admission so it no longer persists in RAM.
- **No HTTP surface.** The router exposes only `/obj/{*key}` (GET/HEAD) and `/ranges/{*key}` (POST)
  (`rust/object-cache-srv/src/object_cache_srv.rs:167-170`). There is no prefetch route and no
  client method on `CacheClientStore` (`rust/object-cache/src/client.rs`).
- **Handlers pattern** (`rust/object-cache-srv/src/handlers.rs`): validate key via
  `validate_key(&key, &state.allowed_prefixes)`, cap per-request work (`MAX_RANGES_PER_REQUEST`,
  `MAX_TOTAL_REQUESTED_BYTES`), acquire `mem_permits` for the *assembled response*, then call the
  cache. The demand handlers must gate on memory because they buffer a contiguous response;
  **prefetch returns no body and must not take a `mem_permit`** (its memory is already bounded by the
  scheduler).
- **Shared validation** already lives in the lib crate (`rust/object-cache/src/validation.rs`), but
  it only shares `validate_key` — there is no pre-existing shared-request-type precedent. The `/ranges`
  path does not share request types (its request struct is private in `handlers.rs`, and the client
  hand-builds JSON). The shared request types (§1's `prefetch.rs`) are therefore a **new** pattern,
  not reuse of an existing shared-type point.
- **AppState** (`rust/object-cache-srv/src/app_state.rs`) is `Clone` and holds the cache, prefix
  allowlist, and memory-permit state — the place to add the prefetch queue handle.

## Design

### 1. Shared request types (`object-cache` crate)

New module `rust/object-cache/src/prefetch.rs`, used by both the server handler and the client so the
wire shape is defined once (DRY):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefetchItem {
    pub key: String,
    /// The object's file size, supplied by the caller. Both triggers already know
    /// it: `Partition.file_size` (persisted in PostgreSQL, `partition.rs:20`) for
    /// query/write warming, and `PartitionWriteResult.file_size`
    /// (`write_partition.rs:407`) for the write path. Supplying it lets the server
    /// drive fills through `prefetch_blocks(key, file_size, indices)` with no
    /// origin HEAD (prefetch targets cold objects, so a server-side `size()` would
    /// force an avoidable HEAD).
    ///
    /// Contract: this must be the object's exact current size. The server
    /// trusts it without verification; an undersized value stores a truncated
    /// final block under the same block key demand reads use (see §2's
    /// size-trust guard for the hazard and its mitigation). An oversized value
    /// is safe *for cache correctness* — the origin GET past EOF fails and
    /// nothing is stored — but see the "oversized size" correction below §2:
    /// it is not safe for *resource use*, because the block-index list is
    /// sized to `size` and built eagerly, before any origin GET happens.
    pub size: u64,
    /// None or empty = warm the whole object `[0, size)`. Present = warm only these
    /// ranges (validated against `size`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranges: Option<Vec<[u64; 2]>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefetchRequest {
    pub keys: Vec<PrefetchItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefetchResponse {
    pub accepted: usize,   // enqueued
    pub rejected: usize,   // failed key/prefix/range validation, skipped
    pub dropped: usize,    // queue full, load-shed
}
```

Whole-object vs ranged is expressed by `ranges` being absent/empty vs populated, matching the
issue's "optionally with ranges" wording. `size` is always caller-supplied, so the server never
resolves size itself.

The `#[derive(Serialize, Deserialize)]` requires a `serde` dependency, which the crate does not yet
have (`rust/object-cache/Cargo.toml` only has `serde_json`). Add `serde.workspace = true` (the
workspace pins the `derive` feature, `rust/Cargo.toml:78`) in alphabetical order.

### 2. Fills go through the size-carrying core path

Because callers always supply `size`, the server drives fills through the existing size-carrying
core method `prefetch_blocks(key, file_size, indices)` (`range_cache.rs:892`) — it takes the
caller-known size and skips the cold-miss origin HEAD. There is **no** `prefetch_object` and no
call to `self.size()`: whole-object warming is just `prefetch_blocks` over the block indices covering
`[0, size)`.

If a size-carrying ranged convenience is later wanted, it should take `file_size` explicitly,
compute block indices from that size (whole object) or from the supplied ranges, and call
`prefetch_blocks` — it must **not** call `self.size()`.

**Size-trust contract + hit-path length guard.** Trusting the caller's `size` opens a poisoning
hazard the demand paths don't have: block cache keys are `blk:{ns}:{key}:{idx}`
(`range_cache.rs:566`) with no size component, shared by prefetch and demand. A prefetch with an
*undersized* `size` clamps the final block's GET to that wrong size (`block_byte_range` clamps
`end` to `file_size`, `blocks.rs:13`) and stores the truncated block; a later demand read — which
resolves the *true* size via `size()` — finds the short block on the hit path (used as-is, with no
length validation, `range_cache.rs:584`) and `assemble_range` silently clips to the stored length
(`blocks.rs:55-57`), returning fewer bytes than requested with **no error**. (An *oversized*
`size` is safe: the final run's GET requests bytes past EOF, the origin errors, and nothing is
stored.)

Mitigation shipped with this issue: in `fetch_blocks`' backend-probe path, validate each hit's
length against the expected `block_byte_range(idx, block_size, file_size).len()`; on a mismatch,
bump `range_cache_block_len_mismatch`, treat the block as a miss, and refetch/overwrite it. This
costs one length compare per hit, heals a poisoned entry on the next correctly-sized read, and
also defends against origin objects that changed size. The trust contract is documented on
`PrefetchItem::size` (§1).

**Resource-cost of computing which blocks to fetch (why the fills stream).** The cache-correctness
argument above ("an oversized `size` is safe") does not cover the *in-process cost of computing
which blocks to fetch* before any origin GET happens. The original branch collected the full
block-index range into a `Vec<u64>` up front (`block_indices_for`'s `.collect()`), and `fetch_blocks`
builds `probe_futs`, `missing`, and an `entries: HashMap<u64, Arc<InFlight>>` all sized to that
index count. None of that is bounded by the worker concurrency or `max_coalesced_get_bytes` (both
only bound the *origin-fetch* side). For a legitimate 2 GB partition (~2000 blocks at 1 MiB) it is
trivial (~1 MB of bookkeeping), but for an adversarial or buggy `size` (`u64::MAX`) the `Vec<u64>`
alone needs 100+ TB and Rust aborts the process on allocation failure — a crash/DoS vector from one
malformed caller. The branch's first mitigation was a flat 512 MiB per-item cap; that was too blunt
(it rejects the very multi-GB partitions #1201 exists to warm) and is being removed.

**Resolution (this is the refactor being folded in): the worker never materializes the full index
list.** It streams the block-index space lazily in bounded windows and drives them through
`buffer`-family concurrency (§3). `0..num_blocks` is a lazy iterator — representing `size == u64::MAX`
costs nothing — and each window is a bounded `Vec<u64>` (`WINDOW_BLOCKS` long), so `fetch_blocks`'s
per-call structures are bounded regardless of `size`. Peak memory is
`worker_concurrency * window_concurrency * WINDOW_BLOCKS`, independent of the object size. This
removes the OOM vector at its root, so **no per-item size ceiling is needed** and the 512 MiB cap is
deleted. The only residual concern is a garbage over-claimed `size` spinning through windows past the
real EOF; that is bounded by **stopping an item's window stream on the first origin fetch error** — a
range fully past EOF errors at the origin, so the stream halts within `window_concurrency` windows of
the true end, with no magic number (see §3).

### 3. Bounded prefetch queue + streaming worker (backpressure)

A large trace can enqueue many GB of keys. Two things must be bounded independently:

1. **The number of items awaiting service** — bounded by a queue that **load-sheds on overflow**,
   the correct semantics for best-effort prefetch (it must never apply backpressure to the caller's
   write/query path).
2. **The per-item work done to compute and hold in-flight blocks** — bounded by streaming the
   block-index space in windows rather than materializing it (§2). Without this a single
   whole-object item with a large or garbage `size` blows up regardless of how few items are queued.

**Queue.** A bounded `tokio::sync::mpsc::channel::<PrefetchItem>(capacity)`. The `Sender` goes in
`AppState`; the `Receiver` is drained by a consumer spawned at startup. The handler `try_send`s and
counts `dropped` on `Full` (§4) — this is the only backpressure point.

**Streaming consumer.** The consumer processes items concurrently and, *within* each item, streams
lazy block-index windows through `buffered` (ordered, so stop-on-error is well-defined). Nothing is
ever sized to `item.size`:

```text
use tokio_stream::wrappers::ReceiverStream;
use futures::stream::StreamExt;

ReceiverStream::new(rx)
    .for_each_concurrent(worker_concurrency, |item| {
        let cache = cache.clone();
        async move { warm_item(&cache, item, block_size).await; }  // best-effort; errors logged
    })
    .await;
// Resolves only once the channel is closed (all senders dropped) AND every in-flight
// `warm_item` has finished — this is the deterministic drain (see below).

async fn warm_item(cache: &RangeCache, item: PrefetchItem, block_size: u64) {
    // Lazy iterator of bounded windows over either [0, size) or the supplied ranges.
    // `lazy_windows` walks block indices in chunks of WINDOW_BLOCKS and NEVER collects
    // the whole space — `size == u64::MAX` is just a long lazy iterator.
    let windows = lazy_windows(&item, block_size);          // Iterator<Item = Vec<u64>>, each <= WINDOW_BLOCKS
    let mut stream = futures::stream::iter(windows)
        .map(|w| {
            let cache = cache.clone();
            let key = item.key.clone();
            async move { cache.prefetch_blocks(&key, item.size, &w).await }
        })
        .buffered(WINDOW_CONCURRENCY);                       // ordered: results in window order
    while let Some(res) = stream.next().await {
        if let Err(e) = res {
            // A window fully past the real EOF errors at the origin (over-claimed
            // `size`); a transient failure also lands here. Either way stop this
            // item — there is nothing useful past the first failing window. This
            // is the bound that replaces the size cap: `buffered` only advances the
            // lazy `windows` iterator as it needs the next future, so on break the
            // remaining windows are never generated.
            imetric!("object_cache_prefetch_fill_error", "count", 1);
            debug!("prefetch fill failed key={} : {e:?}", item.key);
            return;
        }
    }
    imetric!("object_cache_prefetch_keys_warmed", "count", 1);
}
```

- **Spawn levels — verified against the code.** After this refactor there are exactly **two**
  `tokio::spawn` sites, each at the right level and no deeper:
  1. **One consumer task** wrapping `ReceiverStream(rx).for_each_concurrent(...)` — the background
     worker; top-level and necessary (its `JoinHandle` is also the deterministic-drain signal below).
  2. **One per coalesced run** inside `fetch_blocks` (`range_cache.rs:668`) — the real I/O (origin
     GET + `backend.put`) plus its `FulfillGuard`/`scheduler.remove_entry` cleanup, which writes to
     the backend *before* `fulfill`. This is "as top-level as necessary": it is the outermost point
     that both (a) guarantees every owned `InFlight` entry is fulfilled independently of the caller
     (so a dropped caller / shut-down worker can't strand joiners) and (b) keeps runs *parallel*
     across runtime threads. Hoisting it to one spawn per `fetch_blocks` call would serialize a
     call's runs onto a single task; pushing it to per-block would add overhead against the
     coalescing unit.
  There is **no** spawn per window or per item. `warm_item` is lightweight orchestration (key
  formatting, `own_or_join`, awaiting joins) and is correct to poll cooperatively under
  `buffered`/`for_each_concurrent`; its heavy work is already parallelized by the per-run spawns
  underneath. This refactor also *removes* the current branch's per-fill `JoinSet::spawn`
  (`prefetch_queue.rs`) — that loses only orchestration parallelism (negligible; I/O parallelism is
  preserved by the per-run spawns) in exchange for a simpler pipeline. Adding a per-item/window spawn
  would only make the concurrency bound fuzzier (a spawned task starts eagerly, not when the
  combinator polls it) and force `JoinError` handling, for no I/O-parallelism gain.
- **`WINDOW_BLOCKS`** is sized so one window contains several coalesced runs (so a single window can
  already saturate the scheduler's `prefetch_permits`); `WINDOW_CONCURRENCY` can then be small (even
  `1`). Total in-flight `prefetch_blocks` = `worker_concurrency * WINDOW_CONCURRENCY`, each bounded to
  `WINDOW_BLOCKS`; the hard global ceiling on actual origin fetches remains `prefetch_permits`.

**`lazy_windows` + empty-span guard.** For a whole-object warm the span is `[0, size)`; for supplied
ranges it is the (validated) `[s, e)` spans. `lazy_windows` walks each span's block indices in
`WINDOW_BLOCKS` chunks, lazily, and must **skip empty spans**: emit nothing for `size == 0`, and skip
any range with `s >= e`. This is mandatory because `blocks_for_range(start, end, block_size)`
(`blocks.rs:4-9`) computes `last = (end - 1) / block_size` — with `end == 0` the `u64` subtraction
underflows to `u64::MAX / block_size` and its `debug_assert!(start < end)` fires in test builds. An
empty window set makes `warm_item` a no-op (matching the Testing Strategy's "`size == 0` → no-op"
contract). No cross-window dedup is needed even when supplied ranges overlap: the scheduler's
`own_or_join` and the backend hit-path already dedup at the block level.

**Deterministic drain/completion signal (for tests).** The prefetch pipeline is async, so an
assertion made "right after `202`" is racy. The completion signal is the `for_each_concurrent`
future itself: it resolves only after the channel closes (all senders dropped) *and* every in-flight
`warm_item` has finished. Spawning the consumer with `tokio::spawn` yields a `JoinHandle` that
resolves at exactly that moment — no `JoinSet`, no manual reaping, no `tokio::time::sleep`. A test
drives a deterministic drain by dropping every sender and awaiting that `JoinHandle`. This covers
*fill* completion only; because §7 routes prefetch through foyer's asynchronous SSD flush, tests must
additionally `close()` the backend (the deterministic-flush mechanism) before asserting cache
presence (see Testing Strategy).

The sender-drop drain composes differently across the two test styles. Direct-handler tests (the
`memory_budget_tests.rs` style, calling handlers with an owned `AppState`) own every sender clone, so
dropping the state closes the channel directly. Tests that spawn the full axum app **cannot** drop
the sender while the server runs — every `AppState` clone inside the router holds a `prefetch_tx`
clone — so they instead retain a clone of the `RangeCache` (and the counting origin store) outside
the server, and drain by shutting the server down (dropping all `AppState` clones, hence all
senders), awaiting the worker `JoinHandle`, then `close()`-ing the backend; warming assertions and
the follow-up demand read then run directly against the retained cache handle, not over HTTP.

### 4. `POST /prefetch` handler

```text
prefetch_handler(State(state), body: Bytes) -> Result<Response, StatusCode>
```

1. Deserialize `PrefetchRequest`; malformed JSON → `400`.
2. Cap batch size: reject > `MAX_PREFETCH_KEYS_PER_REQUEST` (`4096`, matching
   `MAX_RANGES_PER_REQUEST`) with `400` (bounds per-request work on an
   authenticated endpoint). Cap ranges-per-key with the existing `MAX_RANGES_PER_REQUEST`.
3. For each item: `validate_key(&item.key, &state.allowed_prefixes)`; validate each supplied range
   against the caller-known `item.size` — reject inverted/degenerate ranges (`s >= e`) and
   out-of-bounds ranges (`e > item.size`), matching the demand paths. `ranges` absent or empty is
   **accepted** as a whole-object warm of `[0, item.size)` (per the §1 contract; the consumer loop
   computes the block indices from `item.size`), not rejected. A failing item is **skipped**
   (counted in `rejected`), not fatal — a batch with one bad key still warms the rest.
   **`item.size` itself is NOT capped** — the streaming worker (§3) bounds per-item work regardless
   of size, and an over-claimed `size` is bounded by stop-on-first-error, so there is no
   `item.size > MAX_TOTAL_REQUESTED_BYTES` rejection (that stopgap is removed; see §2 and
   "Superseded: the size cap" below).
4. `try_send` each accepted item onto the queue:
   - `Ok` → `accepted += 1`
   - `Err(TrySendError::Full)` → `dropped += 1`, `imetric!("object_cache_prefetch_dropped", ..)`
   - `Err(TrySendError::Closed)` → `503` (worker gone; should not happen in normal operation)
5. Emit `object_cache_prefetch_requests` and `object_cache_prefetch_keys_enqueued`.
6. Respond `202 Accepted` with `PrefetchResponse` JSON. **No `mem_permit` is acquired** — the
   response carries no object bytes.

Route registration (behind the same auth middleware as the other data routes, in `obj_router`):

```rust
.route("/prefetch", post(prefetch_handler))
```

Keys live in the body (not the path) because a request is inherently multi-key, unlike
`/obj/{*key}`.

### 5. Client surface (`CacheClientStore`)

`prefetch` is not part of the `ObjectStore` trait, and downstream callers (#1200 analytics, #1201
daemon) hold `Arc<dyn ObjectStore>` — so a plain inherent method is not reachable through their
handle. Provide both:

- Inherent method (directly testable against a live server):

```rust
impl CacheClientStore {
    pub async fn prefetch(&self, items: Vec<PrefetchItem>) -> Result<PrefetchResponse> {
        // POST {base}/prefetch with PrefetchRequest { keys: items }, bearer auth,
        // parse PrefetchResponse. On transport error or non-2xx: debug-log +
        // imetric!("range_cache_client_prefetch_error") and return Err.
        // Best-effort: callers ignore the error (no demand read to serve, so no
        // fallback — unlike get_opts/get_ranges).
    }
}
```

- A dyn-compatible seam so #1200/#1201 can hold the capability without downcasting — a minimal trait
  in the `object-cache` crate implemented by `CacheClientStore`:

```rust
#[async_trait]
pub trait ObjectPrefetch: Send + Sync {
    async fn prefetch(&self, items: Vec<PrefetchItem>) -> anyhow::Result<PrefetchResponse>;
}
```

Returning `PrefetchResponse` (not `()`) matches the inherent method so dyn consumers
(#1200/#1201) can observe the `accepted`/`rejected`/`dropped` counts — the load-shed
observability the response body is justified by.

Wiring an `Arc<dyn ObjectPrefetch>` into the analytics/daemon layers is #1200/#1201, not this issue;
defining the trait here fixes the contract they depend on (open/closed).

### 6. CLI / config additions (`cli.rs`)

Follow the existing env-var pattern (`MICROMEGAS_OBJECT_CACHE_*`) and validate at startup like the
other numeric knobs in `object_cache_srv.rs:39-68`:

- `MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY` (default `4096`) — bounded channel depth.
- `MICROMEGAS_OBJECT_CACHE_PREFETCH_WORKER_CONCURRENCY` (default `8`) — concurrent items warmed
  (`for_each_concurrent` degree, §3).

Reject `0` for either at startup (fatal config error), matching the existing guards.

`WINDOW_BLOCKS` (per-window block count) and `WINDOW_CONCURRENCY` (in-flight windows per item) are
module-level `const`s in `prefetch_queue.rs`, not env knobs — they bound per-item memory (§3) and
have no reason to be operator-tuned. Size `WINDOW_BLOCKS` so one window spans several coalesced runs
(a single window already saturates `prefetch_permits`), so `WINDOW_CONCURRENCY` can be `1`.

### 7. Prefetch is SSD-only (no RAM residency)

Prefetched bytes must not *retain* RAM-tier residency — a prefetch fill is dropped immediately and
does not persist in RAM to compete with hot demand entries. Today `FoyerBackend::put`
(`foyer_backend.rs:75-77`) always calls
`self.cache.insert_with_hint(key, value, hint.into())`, which inserts into the RAM (memory) tier
first, so prefetched blocks currently live in RAM as `CacheHint::Low` (first-to-evict) entries.

Change `FoyerBackend::put` to branch on the fill hint:

- `FillHint::Prefetch` → `self.cache.storage_writer(key).force().insert(value)` — foyer 0.14.1's
  disk-only admission path, with `.force()` so admission is **deterministic**. Without `.force()`,
  `insert()` runs `insert_inner`, which consults the disk admission picker (`if !self.pick()`,
  `hybrid/writer.rs:116-141`) and returns `None` (writing nothing) if the picker declines — that
  would make prefetch presence nondeterministic and flake the Testing Strategy assertion.
  `.force()` bypasses the picker so the block is always admitted to the SSD tier. The write holds
  only an *ephemeral* RAM record that is dropped immediately (`foyer` `hybrid/writer.rs:138-146`,
  ephemeral drop at `foyer-memory` `raw.rs:738-748`): it gains **no eviction-structure residency**
  and is removed on drop, so prefetched blocks do not *retain* RAM residency. `put` drops the
  returned `Option<HybridCacheEntry>` immediately; on an unexpected `None` (should not occur under
  `.force()`) it logs / bumps a metric rather than assuming the write succeeded.
- demand fills → unchanged, `insert_with_hint` (RAM tier).

This supersedes the old `CacheHint::Low` RAM-residency behavior for prefetch: prefetched blocks no
longer live in RAM at all. The backend write for both paths still happens at `range_cache.rs:679`
via `backend.put(.., hint)`; only the hint-based branch inside `put` is new.

To make "RAM tier byte usage unchanged" observable from the integration test suite (which compiles
as a separate crate and cannot reach the private `FoyerBackend.cache` field, `foyer_backend.rs:10`),
add a public introspection accessor on `FoyerBackend`: `pub fn ram_usage(&self) -> usize` delegating
to `self.cache.memory().usage()` (foyer 0.14.1's `HybridCache::memory().usage()`,
`foyer-memory-0.14.1/src/cache.rs:656-663`). The SSD-only test asserts this value is unchanged across
a prefetch fill.

Because the prefetch branch now uses `storage_writer` (no `CacheHint`), only
`FillHint::Demand => CacheHint::Normal` is ever reached in the `FillHint`→`CacheHint` conversion.
Note the now-dead `FillHint::Prefetch => CacheHint::Low` arm **cannot simply be deleted** — the impl
is a `match` over the two-variant `FillHint` enum, so removing one arm is a non-exhaustive-match
compile error (E0004). Instead, **delete the whole `From<FillHint> for CacheHint` impl**
(`foyer_backend.rs:48-55`, which has no adjacent comment; `put` is its only user in the crate) and
have the demand branch call `insert_with_hint(key, value, CacheHint::Normal)` directly, so the
conversion doesn't carry a stale mapping.

Separately, the LRU-pinning comment lives elsewhere: on the `.with_eviction_config(LruConfig::default())`
call in `new_with_shards` (`foyer_backend.rs:29-34`). Its rationale is that only LRU maps
`CacheHint::Low`, so pinning LRU defended `FillHint::Prefetch` against a future foyer default change.
Once no `FillHint` maps to `CacheHint::Low`, that defensive `LruConfig::default()` pinning no longer
protects prefetch — update that comment accordingly (the pinning may be removed, or kept with an
updated rationale that no longer references prefetch).

## Implementation Steps

### Phase 1 — shared types + dependencies
1. Add `serde.workspace = true` (derive feature) to `rust/object-cache/Cargo.toml` in alphabetical
   order — required for the `#[derive(Serialize, Deserialize)]` in `prefetch.rs` (the crate currently
   has only `serde_json`).
2. Add `rust/object-cache/src/prefetch.rs` with `PrefetchItem` (with required `size`)
   /`PrefetchRequest`/`PrefetchResponse` and the `ObjectPrefetch` trait; export from
   `rust/object-cache/src/lib.rs`. No new method on `RangeCache` is needed — the consumer drives
   fills through the existing `prefetch_blocks(key, file_size, indices)` using the caller-supplied
   `size`.
3. Change `FoyerBackend::put` (`rust/object-cache/src/foyer_backend.rs`) to branch on the fill hint:
   `FillHint::Prefetch` → `storage_writer(key).force().insert(value)` (SSD-only, ephemeral RAM
   record; `.force()` bypasses the disk admission picker for deterministic admission, per §7);
   demand → `insert_with_hint(key, value, CacheHint::Normal)` directly. Delete the whole
   `From<FillHint> for CacheHint` impl (`foyer_backend.rs:48-55`, no adjacent comment; `put` is its
   only user) — removing only the `Prefetch` arm would be a non-exhaustive-match compile error (§7). Separately, update the LRU-pinning comment at `foyer_backend.rs:29-34`
   (on `.with_eviction_config(LruConfig::default())`): with no `FillHint` mapping to `CacheHint::Low`,
   the defensive `LruConfig::default()` pinning no longer protects prefetch and may be removed or
   kept with an updated rationale (§7). Also add the `pub fn ram_usage(&self) -> usize` accessor
   (delegating to `self.cache.memory().usage()`) so the SSD-only integration test can assert RAM-tier
   byte usage is unchanged (§7).
4. Add the §2 size-trust guard in `fetch_blocks` (`rust/object-cache/src/range_cache.rs`):
   validate each backend hit's length against `block_byte_range(idx, block_size, file_size).len()`,
   treating a mismatch as a miss (refetch + overwrite) and bumping
   `range_cache_block_len_mismatch`.

### Phase 2 — server endpoint + queue
5. Add `prefetch_queue` module with the bounded `mpsc` + **streaming** consumer builder returning
   `(Sender, JoinHandle)` (§3). The consumer is `ReceiverStream::new(rx).for_each_concurrent(
   worker_concurrency, warm_item)` wrapped in one `tokio::spawn`; `warm_item` streams lazy
   `WINDOW_BLOCKS`-sized windows via `futures::stream::iter(lazy_windows).map(prefetch_blocks)
   .buffered(WINDOW_CONCURRENCY)` and `return`s on the first `Err` (the stop-on-error bound that
   replaces the size cap). No `JoinSet`, no per-fill semaphore, no manual reaping — the
   `for_each_concurrent` future is itself the deterministic drain (resolves when all senders drop
   *and* all in-flight `warm_item`s finish), so a test that drops the sender and awaits the
   `JoinHandle` knows all fills are done with no `tokio::time::sleep`. Add `lazy_windows` (lazy,
   empty-span-guarded per §3) and the `WINDOW_BLOCKS`/`WINDOW_CONCURRENCY` module consts here. The
   crate needs `futures` and `tokio-stream` (for `ReceiverStream`) — add in alphabetical order if
   absent.
6. Extend `AppState` (`app_state.rs`) with `prefetch_tx: mpsc::Sender<PrefetchItem>` (only the
   sender lives in `AppState`). Add `MAX_PREFETCH_KEYS_PER_REQUEST` (`4096`) as a module-level
   `const` in `handlers.rs`, alongside the existing per-request caps (`MAX_RANGES_PER_REQUEST`,
   `MAX_TOTAL_REQUESTED_BYTES`) — not as an `AppState` field. Adding `prefetch_tx` changes the
   `AppState::new` signature, so update every caller — including the `make_state` helper in
   `tests/memory_budget_tests.rs`, which must construct and pass a throwaway `prefetch_tx`.
   (Alternatively, keep the existing 3-arg constructor working via a builder/default so `make_state`
   is untouched.)
7. Add `prefetch_handler` to `handlers.rs` (validation, `try_send`, `202` + counts, no mem_permit).
   Caps applied are the **batch-size** cap (`MAX_PREFETCH_KEYS_PER_REQUEST`) and **ranges-per-key**
   cap (`MAX_RANGES_PER_REQUEST`) only — **no per-item `size` cap** (removed; §4/§2).
8. In `object_cache_srv.rs`: add the two CLI options + startup validation; build the queue/worker;
   store the sender in `AppState`; register `.route("/prefetch", post(prefetch_handler))` on
   `obj_router` (inside the auth layer).

### Phase 3 — client
9. Add `CacheClientStore::prefetch` inherent method and `impl ObjectPrefetch for CacheClientStore`
   (`client.rs`).

### Phase 4 — metrics + docs + tests
10. Metrics: `object_cache_prefetch_requests`, `object_cache_prefetch_keys_enqueued`,
    `object_cache_prefetch_dropped`, `object_cache_prefetch_keys_warmed`,
    `object_cache_prefetch_fill_error`, `range_cache_client_prefetch_error`,
    `range_cache_block_len_mismatch` (§2 guard), `range_cache_prefetch_admission_unexpected_none`
    (§7's defensive `FoyerBackend::put` branch — should never fire; a sustained non-zero rate means
    foyer's `.force().insert(value)` is returning `None` when it shouldn't).
11. Docs (below).
12. Tests (below).

## Files to Modify
- `rust/object-cache/Cargo.toml` — add `serde.workspace = true` (derive) in alphabetical order.
- `rust/object-cache/src/prefetch.rs` (new) — shared types (`PrefetchItem` with required `size`) +
  `ObjectPrefetch` trait.
- `rust/object-cache/src/lib.rs` — export the new module.
- `rust/object-cache/src/range_cache.rs` — hit-path block-length validation in `fetch_blocks`
  (§2 size-trust guard) + `range_cache_block_len_mismatch` metric.
- `rust/object-cache/src/foyer_backend.rs` — `put` branches to SSD-only
  `storage_writer(key).force().insert(value)` for `FillHint::Prefetch` (§7); delete the whole
  `From<FillHint> for CacheHint` impl (`:48-55`, no comment; arm-only removal would not compile),
  inlining `CacheHint::Normal` in the demand branch, and update the LRU-pinning comment at `:29-34`;
  add the
  `pub fn ram_usage(&self) -> usize` introspection accessor for the SSD-only test (§7).
- `rust/object-cache/src/client.rs` — `prefetch` method + trait impl.
- `rust/object-cache-srv/src/app_state.rs` — queue sender `prefetch_tx` (changes `AppState::new`
  signature); the `MAX_PREFETCH_KEYS_PER_REQUEST` cap is a module `const` in `handlers.rs`, not here.
- `rust/object-cache-srv/tests/memory_budget_tests.rs` — update the `make_state` helper for the new
  `AppState::new` signature (construct/pass a throwaway `prefetch_tx`), unless a builder/default keeps
  the 3-arg path working.
- `rust/object-cache-srv/src/handlers.rs` — `prefetch_handler`, `MAX_PREFETCH_KEYS_PER_REQUEST`
  module `const` (alongside the existing caps). **No per-item size cap.**
- `rust/object-cache-srv/src/prefetch_queue.rs` — bounded `mpsc` + streaming consumer
  (`for_each_concurrent` over items, per-item lazy `buffered` window stream, stop-on-error),
  `lazy_windows`, and the `WINDOW_BLOCKS`/`WINDOW_CONCURRENCY` consts (§3).
- `rust/object-cache-srv/Cargo.toml` — add `futures` and `tokio-stream` (for `ReceiverStream`) in
  alphabetical order if not already present.
- `rust/object-cache-srv/src/cli.rs` — two new options.
- `rust/object-cache-srv/src/object_cache_srv.rs` — startup validation, queue build, route.
- `rust/object-cache-srv/tests/prefetch_tests.rs` (new) — handler + client integration tests.
- `mkdocs/docs/admin/object-cache.md` — prose `POST /prefetch` subsection + two new knobs in both the
  env-var and CLI-flags tables; `rust/object-cache-srv/README.md` — `POST /prefetch` row in the HTTP
  API table + env/CLI knobs.

## Trade-offs
- **Load-shed on overflow vs block/503.** Best-effort prefetch must never stall the caller, so a full
  queue drops items (with a metric) and still returns `202`. A blocking send or `503` would push
  backpressure onto a fire-and-forget caller — wrong for this path. The `dropped` count in the
  response keeps it observable.
- **Body-keyed vs path-keyed endpoint.** `/obj` and `/ranges` key by path, but prefetch is
  multi-key by nature, so keys live in the JSON body. Consistent with the "batch of keys" framing in
  #1198/#1200/#1201.
- **Trait + inherent method vs inherent only.** The trait is a small addition #1198's own tests don't
  need, but downstream consumers hold `dyn ObjectStore` and can't reach an inherent method; defining
  the contract now avoids a later downcast hack.
- **Separate queue vs bare `tokio::spawn` per request.** A bare spawn is simpler but unbounded — a
  burst enqueues unbounded `InFlight` entries. The bounded queue is the backpressure mechanism the
  issue calls for. The queue bounds *how many items* are in flight; the streaming worker (§3) bounds
  *the work per item* — both are needed, and neither substitutes for the other.
- **Streaming windows vs eager index list + size cap.** The worker streams the block-index space in
  bounded windows (§3) rather than collecting it, so peak per-item memory is a constant regardless of
  `item.size`. This removes the process-abort (OOM) vector at its root and lets the endpoint accept
  the legitimate multi-GB partitions #1201 warms — with **no per-item size ceiling**. The earlier
  branch's 512 MiB cap (reusing `MAX_TOTAL_REQUESTED_BYTES`) is removed; see "Superseded: the size
  cap" below. The residual over-claimed-`size` spin is bounded by stop-on-first-error, not a byte
  ceiling.
- **SSD-only prefetch vs RAM residency.** Prefetch writes bypass the RAM tier (§7), so a subsequent
  demand hit on a prefetched block reads from SSD, not RAM (slightly slower than a RAM hit). In
  exchange, a prefetch fill does not retain RAM residency — it is dropped immediately and never
  persists in RAM to compete with hot demand entries — the point of a best-effort background warm.
- **Caller-supplied size vs server-side resolution.** Requiring `size` on each `PrefetchItem` means
  the server never issues an origin HEAD to size a cold object (which prefetch targets by
  definition). Both triggers already have the size (`Partition.file_size` /
  `PartitionWriteResult.file_size`), so this is free for callers and removes a network round-trip per
  key. The cost is a trust contract: an undersized value would poison the final block's cache entry
  with truncated bytes — mitigated by the §2 hit-path length guard, which detects and refetches
  wrong-length blocks on the next correctly-sized read.
- **No negative-cache coupling here.** Warming a key that doesn't exist yet just fails the fill
  quietly; the NotFound-TTL interaction is #1196/#1201, out of scope.

## Documentation
- `mkdocs/docs/admin/object-cache.md`: add a prose `POST /prefetch` subsection (body shape, `202`
  semantics, load-shedding) — this page has no per-endpoint HTTP API table, so document it as prose;
  add the two new knobs to **both** the Environment variables table and the CLI flags table. (The
  tables are not perfect mirrors today — `MICROMEGAS_API_KEYS` has no CLI row, and
  `--disable-auth`/`--allow-all-prefixes` have no env rows — but every tunable knob appears in
  both; keep that true for the two new ones.) Drive-by while editing that table: its
  `--allowed-prefix` row is wrong — the actual flag is `--prefix` (`cli.rs:46`); fix the flag
  name.
- `rust/object-cache-srv/README.md`: add the `POST /prefetch` row to the HTTP API table alongside
  `/obj` and `/ranges`, and mirror the two new env/CLI knobs.
- **Remove the 512 MiB per-item size-cap text** the branch already added to `mkdocs` and `README.md`
  — the cap is gone (§2/§3). Keep the 4096-key batch-cap text. State instead that `/prefetch`
  imposes no per-item size limit (whole-object warming of arbitrarily large partitions is supported).
- `PrefetchItem` doc comment (the required `size` contract) and the `FoyerBackend::put` SSD-only
  branch (§7).
- Changelog entry.

## Testing Strategy
- **Unit** (`object-cache`): `PrefetchRequest`/`PrefetchResponse`/`PrefetchItem` (with `size`) serde
  round-trip; a whole-object fill with `size == 0` yields an empty block-index set and is a no-op.
- **Size-trust guard** (`object-cache`, `range_cache_tests.rs`): prefetch a key with an undersized
  `size` (the fill stores a truncated final block), then a demand `get_range` at the true size —
  the §2 hit-path guard detects the short block, refetches it (origin GET count increases),
  returns the full correct bytes, and bumps `range_cache_block_len_mismatch`.
- **Streaming bound / over-claimed `size`** (`object-cache-srv`, `prefetch_tests.rs`): a prefetch
  with a `size` far larger than the real object does **not** OOM or hang — the worker streams
  windows lazily and stops on the first origin error past the true EOF. Assert the item completes
  (worker drains), the real blocks are warmed (a subsequent demand read for the real range is a
  cache hit), and `object_cache_prefetch_fill_error` is bumped. A garbage `size` (e.g. `u64::MAX`)
  is accepted by the handler (no size cap) and bounded the same way. Also assert a legitimately
  large `size` (well past the old 512 MiB cap) is accepted and warms its early blocks.
- **Server integration** (`object-cache-srv/tests/prefetch_tests.rs`): these tests need a counting
  origin-store wrapper (one that increments a counter on each origin GET) added for the suite.
  `memory_budget_tests.rs`'s `DelayedStore` only gates via a `Semaphore` and counts nothing, so it
  can be copied as a wrapping pattern but does not itself provide the request counter these
  assertions require.
  - **Deterministic completion, not `sleep`.** The prefetch pipeline is fully async (the streaming
    `for_each_concurrent` worker in §3, async SSD flush in §7), so an assertion made "right after
    `202`" is racy. Every warming assertion below is gated on a two-step deterministic wait, never
    `tokio::time::sleep`: (a) **drain the worker** — close the channel per §3's per-test-style
    note (direct-handler tests drop the `prefetch_tx`; spawned-server tests shut the server down
    so every `AppState` clone and its sender drops) and `await` the consumer's `JoinHandle` (§3),
    which resolves only after the `for_each_concurrent` stream ends and every in-flight `warm_item`
    (hence its `prefetch_blocks` windows) has completed; then (b) **flush the SSD tier** — `close()` the
    backend, the deterministic-flush mechanism existing cache tests use (reads still work after
    `close()`), so the ephemeral RAM record's asynchronous SSD write is durable before reading. Only
    after (a) + (b) does the test assert presence / issue the demand
    read. Presence is deterministic (not merely timing-dependent) because §7 uses `.force()` to
    bypass foyer's disk admission picker — the block is always admitted, so the picker can never
    silently decline the write and flake this assertion.
  - `POST /prefetch` for uncached keys → `202`; after draining the worker and flushing the SSD tier
    (per above), the blocks are present in the backend and a subsequent demand `get_range` of the
    same key issues **no** new origin GET (served from cache).
  - **SSD-only admission** (§7): after draining the worker and flushing the SSD tier, the RAM
    (memory) tier byte usage — read via the new `FoyerBackend::ram_usage()` accessor (§7) — is
    unchanged from before the prefetch fill; the prefetched block is served from SSD, not RAM
    (contrast a demand fill, which does populate the RAM tier). Since callers supply `size`, this
    fill also issues no origin HEAD.
  - Fills run at prefetch priority: a saturating prefetch batch does not starve a concurrent demand
    read (reuse the priority assertions from the #1203 suite at the HTTP layer).
  - Queue full → excess items reported as `dropped`, `object_cache_prefetch_dropped` incremented,
    still `202`.
  - Batch with an out-of-prefix / inverted-range item → that item `rejected`, the valid ones warmed.
  - Prefetch acquires **no** `mem_permit`: a prefetch whose total bytes exceed `memory_budget_mb`
    still succeeds (contrast with the demand `413`).
- **Client round-trip**: spawn the axum app on an ephemeral port, point a `CacheClientStore` at it,
  call `prefetch`; then drain per §3's spawned-server recipe (retain a `RangeCache`/counting-origin
  clone outside the server, shut the server down to drop all senders, await the worker handle,
  `close()` the backend) and assert warming directly on the retained cache handle; assert
  `prefetch` returns `Err` (and increments the
  client error metric) when the server is unreachable, without panicking.
- **CI**: `cd rust && python3 ../build/rust_ci.py` (fmt, clippy `-D warnings`, tests).
- **Smoke**: `start_minio.py` + `start_services.py`; `curl -XPOST /prefetch`, then confirm the demand
  read is a cache hit.

## Superseded: the size cap (removed before merge)

The branch's first defense against a pathological caller-supplied `size` was a `prefetch_handler`
rejection of any `PrefetchItem` with `size > MAX_TOTAL_REQUESTED_BYTES` (512 MiB). **This is being
removed and replaced by the streaming worker (§2/§3)** — it is recorded here only so the diff that
deletes it is understood, not proposed again.

Why the cap was wrong:

- **The root cause was eager materialization of the block-index list** (`block_indices_for`'s
  `.collect()`, plus `fetch_blocks`' `probe_futs`/`missing`/`entries`, all sized to block count),
  not the caller-supplied `size` itself. Streaming the index space in bounded windows (§3) fixes
  the cause: peak per-item memory is a constant regardless of `size`, so no OOM vector remains and
  no size ceiling is required.
- **512 MiB was too small for this endpoint's own purpose.** `PrefetchItem.size` is sourced from
  `Partition.file_size`, and partition size here is time-window-driven, not size-capped — a
  legitimate partition can exceed 512 MiB. The cap silently rejected whole-object prefetch for
  exactly the multi-GB partitions #1201's write-time warming exists to warm. Reusing
  `MAX_TOTAL_REQUESTED_BYTES` also conflated two concerns: on `/ranges` it bounds *buffered response
  bytes held in memory*; `/prefetch` buffers no response body, so the same constant was doing an
  unrelated job.
- **The over-claimed-`size` spin is bounded without a byte ceiling.** Streaming turns the old
  OOM-abort into a bounded lazy walk; a `size` past the real EOF stops at the first origin error
  (§3 stop-on-error), and an undersized `size` is still healed by the §2 hit-path length guard. So
  both trust hazards are covered without capping legitimate objects.

Implementation note: `prefetch_blocks(key, file_size, indices)` is stateless per call — nothing in
`fetch_blocks` persists across calls beyond what it creates and tears down internally (the
`FulfillGuard` cleans up the scheduler's entries at the end of each call) — so calling it per window
in a loop is a drop-in, with per-window overhead limited to one extra `HashMap`/scheduler-mutex
round trip, negligible next to the actual I/O.

## Open Questions
1. **`ObjectPrefetch` trait now or with the first consumer?** Defining it here fixes the contract but
   adds unused surface until #1200/#1201. Recommendation: add it now (cheap, and it's the reuse
   point); acceptable to defer to #1200 if we'd rather not ship an unused trait.
2. **Whole-object fill scope for large objects. Resolved — the streaming worker bounds per-item
   work; no size cap.** The queue and `FetchScheduler` only bound *origin-fetch concurrency*, not
   the size of the block-index list built per item — which the original branch constructed eagerly
   (proportional to `item.size`). The fix is to stream the index space in bounded windows (§3), so
   peak per-item memory is a constant regardless of `size`. The interim 512 MiB cap is removed (see
   "Superseded: the size cap"). An over-claimed `size` is bounded by stop-on-first-error, an
   undersized one by the §2 length guard.
3. **Response detail.** Is the `accepted/rejected/dropped` body useful to callers, or is a bare `202`
   with the detail only in metrics enough? Leaning: keep the small body — cheap and observable.
4. **Is `MAX_PREFETCH_KEYS_PER_REQUEST` (4096, §4 step 2) still a necessary limit, now that streaming
   removes the analogous per-item size cap?** The size cap existed because per-item work was
   proportional to `size`; streaming windows made that bound unconditional, so the size cap could be
   deleted outright (§2/§3, "Superseded: the size cap"). The batch-size cap bounds something
   different — the handler's own per-request loop over `req.keys` (validation, `try_send`) and the
   JSON body size — not per-item fill work, so it isn't obviously fixed by the same streaming change.
   Before landing, check whether the per-request loop itself has any cost that scales badly with an
   unbounded key count (e.g., deserializing an arbitrarily large JSON body before any validation
   runs) and whether that's better addressed by a body-size limit than a key-count cap. Leaning:
   probably still worth keeping *some* batch cap independent of the streaming fix, since it bounds
   request-parsing/handler-loop cost rather than fill cost — but confirm rather than assume.
