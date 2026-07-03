# Object Cache Prefetch Endpoint + Client Method Plan

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
    /// is safe — the origin GET past EOF fails and nothing is stored.
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

### 3. Bounded prefetch queue + worker (backpressure)

A large trace can enqueue many GB of keys. The origin-fetch concurrency is already bounded by the
scheduler's `prefetch_permits`; what is *not* bounded is the number of items awaiting a permit (each
pending block holds an `InFlight` entry). Bound that with a queue and **load-shed on overflow** — the
correct semantics for best-effort prefetch, which must never apply backpressure to the caller's
write/query path.

- `PrefetchQueue`: a bounded `tokio::sync::mpsc::channel::<PrefetchItem>(capacity)`. The `Sender`
  goes in `AppState`; the `Receiver` is drained by a consumer task spawned at startup.
- Consumer loop drives fills at bounded concurrency (a `Semaphore` sized by
  `prefetch_worker_concurrency`). Each fill computes block indices from the caller-supplied
  `item.size` (whole object when `ranges` is absent/empty) or from the supplied ranges, then calls
  `cache.prefetch_blocks(&item.key, item.size, indices)` — no `size()` lookup, no origin HEAD:

```text
// `fills` is the JoinSet the consumer loop owns for the deterministic drain
// (see "Deterministic drain/completion signal" below and Implementation Step 5).
while let Some(item) = rx.recv().await {
    // Reap completed fills: a JoinSet retains an entry per spawned task until
    // it is joined, and in production the channel never closes, so without
    // this the set grows for the server's lifetime.
    while fills.try_join_next().is_some() {}
    let permit = worker_sem.clone().acquire_owned().await;   // bound in-flight fills
    let cache = cache.clone();
    fills.spawn(async move {
        let _permit = permit;
        // None or empty = whole object [0, size) (matches §1 contract);
        // present = only the supplied ranges. Ranges are already validated
        // against item.size by the handler (§4 step 3), so map [s, e] -> s..e.
        let indices = match item.ranges {
            None => block_indices_for(0..item.size),
            Some(ref rs) if rs.is_empty() => block_indices_for(0..item.size),
            Some(ref rs) => {
                let ranges: Vec<Range<u64>> = rs.iter().map(|[s, e]| *s..*e).collect();
                block_indices_for_ranges(&ranges)
            }
        };
        let outcome = cache.prefetch_blocks(&item.key, item.size, indices).await;
        if let Err(e) = outcome {
            imetric!("object_cache_prefetch_fill_error", "count", 1);
            debug!("prefetch fill failed key={} : {e:?}", item.key);
        } else {
            imetric!("object_cache_prefetch_keys_warmed", "count", 1);
        }
    });
}
```

(`block_indices_for` / `block_indices_for_ranges` denote the block-index computation from a byte
range using the cache's block size — the same mapping the existing core path uses; no new public
method on `RangeCache` is required.)

**Empty-span guard (required before `blocks_for_range`).** The block-index computation must skip
empty spans, mirroring the existing `start < end` guards in the demand paths (`get_range` returns
early on `start >= end`, `range_cache.rs:758`; `prefetch_ranges` extends indices only `if start < e`,
`range_cache.rs:878`). Specifically: for a whole-object warm, return an **empty index set when
`item.size == 0`** (so `0..0` never reaches `blocks_for_range`); for supplied ranges, **skip any
range where `s >= e`**. This is mandatory because `blocks_for_range(start, end, block_size)`
(`blocks.rs:4-9`) computes `last = (end - 1) / block_size` — with `end == 0` the `u64` subtraction
underflows to `u64::MAX / block_size`, producing an enormous bogus index range, and its
`debug_assert!(start < end)` fires in test builds. An empty index set then no-ops safely in
`fetch_blocks`/`prefetch_blocks` (matching the Testing Strategy's "`size == 0` → no-op" contract).

Worker concurrency is a soft knob; the hard ceiling remains the scheduler's `prefetch_permits`.

**Deterministic drain/completion signal (for tests).** A detached fire-and-forget spawn would
surface no join handle, leaving a test no way to observe when the pipeline has finished — a demand
read issued right after `202` could race an in-flight (or not-yet-started) fill. To make completion
observable without `tokio::time::sleep`, the worker tracks its spawned fills instead of
firing-and-forgetting: the code block above collects them in the `tokio::task::JoinSet` (`fills`)
owned by the consumer loop (an `AtomicUsize` in-flight counter paired with a `tokio::sync::Notify`
would work equally well). Because a `JoinSet` retains completed tasks until they are joined — and in
production the `Sender` lives in `AppState` so the channel never closes — the loop **must reap on
each iteration** (`while fills.try_join_next().is_some() {}`, as in the code block above); otherwise
the set grows unboundedly over the server's lifetime. When the channel closes (all `Sender`s dropped),
`rx.recv()` returns `None`; the loop then awaits every outstanding fill in the `JoinSet` before the
consumer's `JoinHandle` resolves. A test therefore drives a deterministic drain by dropping the
sender (or via an explicit shutdown handle) and awaiting that `JoinHandle` — at which point all
enqueued fills have completed their `prefetch_blocks` calls. This drain covers the *fill* completion
only; because §7 routes prefetch through foyer's asynchronous SSD flush, tests must additionally
`close()` the backend (the deterministic-flush mechanism) before asserting cache presence (see
Testing Strategy).

The sender-drop drain composes differently across the two test styles. Direct-handler tests (the
`memory_budget_tests.rs` style, calling handlers with an owned `AppState`) own every sender clone,
so dropping the state closes the channel directly. Tests that spawn the full axum app **cannot**
drop the sender while the server runs — every `AppState` clone inside the router holds a
`prefetch_tx` clone — so they instead retain a clone of the `RangeCache` (and the counting origin
store) outside the server, and drain by shutting the server down (dropping all `AppState` clones,
hence all senders), awaiting the worker `JoinHandle`, then `close()`-ing the backend; warming
assertions and the follow-up demand read then run directly against the retained cache handle, not
over HTTP. If a post-drain demand read ever must go over HTTP, switch the worker's completion
signal to the `AtomicUsize` + `Notify` quiesce variant, which observes in-flight-reaches-zero
without requiring the channel to close.

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
- `MICROMEGAS_OBJECT_CACHE_PREFETCH_WORKER_CONCURRENCY` (default `8`) — concurrent in-flight fills.

Reject `0` for either at startup (fatal config error), matching the existing guards.

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
5. Add `prefetch_queue` module (or inline in `handlers.rs`) with the bounded `mpsc` + consumer-loop
   builder returning `(Sender, JoinHandle)`. The worker exposes a deterministic drain/completion
   signal for tests (§3): the consumer loop tracks spawned fills in a `JoinSet` (or an
   `AtomicUsize` + `tokio::sync::Notify`), reaping completed entries each iteration
   (`try_join_next` — a `JoinSet` retains completed tasks until joined, and the production channel
   never closes), and on channel close it awaits every outstanding fill
   before the `JoinHandle` resolves — so a test that drops the sender and awaits the handle knows
   all fills are done, with no `tokio::time::sleep`.
6. Extend `AppState` (`app_state.rs`) with `prefetch_tx: mpsc::Sender<PrefetchItem>` (only the
   sender lives in `AppState`). Add `MAX_PREFETCH_KEYS_PER_REQUEST` (`4096`) as a module-level
   `const` in `handlers.rs`, alongside the existing per-request caps (`MAX_RANGES_PER_REQUEST`,
   `MAX_TOTAL_REQUESTED_BYTES`) — not as an `AppState` field. Adding `prefetch_tx` changes the
   `AppState::new` signature, so update every caller — including the `make_state` helper in
   `tests/memory_budget_tests.rs`, which must construct and pass a throwaway `prefetch_tx`.
   (Alternatively, keep the existing 3-arg constructor working via a builder/default so `make_state`
   is untouched.)
7. Add `prefetch_handler` to `handlers.rs` (validation, cap, `try_send`, `202` + counts, no
   mem_permit).
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
    `range_cache_block_len_mismatch` (§2 guard).
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
  module `const` (alongside the existing caps) (+ queue/worker if inlined here).
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
  issue calls for.
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
- `PrefetchItem` doc comment (the required `size` contract) and the `FoyerBackend::put` SSD-only
  branch (§7).
- Changelog entry.

## Testing Strategy
- **Unit** (`object-cache`): `PrefetchRequest`/`PrefetchResponse`/`PrefetchItem` (with `size`) serde
  round-trip; a whole-object fill with `size == 0` yields an empty block-index set and is a no-op.
- **Size-trust guard** (`object-cache`, `range_cache_tests.rs`): prefetch a key with an undersized
  `size` (the fill stores a truncated final block), then a demand `get_range` at the true size —
  the §2 hit-path guard detects the short block, refetches it (origin GET count increases),
  returns the full correct bytes, and bumps `range_cache_block_len_mismatch`. An oversized `size`
  fails the fill without storing anything, leaving a subsequent demand read unaffected.
- **Server integration** (`object-cache-srv/tests/prefetch_tests.rs`): these tests need a counting
  origin-store wrapper (one that increments a counter on each origin GET) added for the suite.
  `memory_budget_tests.rs`'s `DelayedStore` only gates via a `Semaphore` and counts nothing, so it
  can be copied as a wrapping pattern but does not itself provide the request counter these
  assertions require.
  - **Deterministic completion, not `sleep`.** The prefetch pipeline is fully async (JoinSet-tracked
    fills in §3, async SSD flush in §7), so an assertion made "right after `202`" is racy. Every
    warming assertion below is gated on a two-step deterministic wait, never
    `tokio::time::sleep`: (a) **drain the worker** — close the channel per §3's per-test-style
    note (direct-handler tests drop the `prefetch_tx`; spawned-server tests shut the server down
    so every `AppState` clone and its sender drops) and `await` the consumer's `JoinHandle` (§3),
    which resolves only after all spawned `prefetch_blocks` fills have completed; then (b) **flush the SSD tier** — `close()` the
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

## Open Questions
1. **`ObjectPrefetch` trait now or with the first consumer?** Defining it here fixes the contract but
   adds unused surface until #1200/#1201. Recommendation: add it now (cheap, and it's the reuse
   point); acceptable to defer to #1200 if we'd rather not ship an unused trait.
2. **Whole-object fill scope for large objects.** A whole-object item warms every block covering
   `[0, size)` via `prefetch_blocks` (using the caller-supplied `size`). For a multi-GB partition
   (#1201) that is a lot of blocks at once — do we want a per-object block cap here, or leave
   bounding to the queue + scheduler? Leaning: leave it to the queue/scheduler for #1198 and revisit
   caps in #1200 (which handles trace-sized enumeration).
3. **Response detail.** Is the `accepted/rejected/dropped` body useful to callers, or is a bare `202`
   with the detail only in metrics enough? Leaning: keep the small body — cheap and observable.
