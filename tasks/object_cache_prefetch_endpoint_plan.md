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
    (`range_cache.rs:174-175`), and a demand joiner promotes a prefetch entry via `own_or_join`
    (`range_cache.rs:200-206`). Prefetched blocks land in foyer at `CacheHint::Low`
    (`foyer_backend.rs:48-54`).
- **No HTTP surface.** The router exposes only `/obj/{*key}` (GET/HEAD) and `/ranges/{*key}` (POST)
  (`rust/object-cache-srv/src/object_cache_srv.rs:167-170`). There is no prefetch route and no
  client method on `CacheClientStore` (`rust/object-cache/src/client.rs`).
- **Handlers pattern** (`rust/object-cache-srv/src/handlers.rs`): validate key via
  `validate_key(&key, &state.allowed_prefixes)`, cap per-request work (`MAX_RANGES_PER_REQUEST`,
  `MAX_TOTAL_REQUESTED_BYTES`), acquire `mem_permits` for the *assembled response*, then call the
  cache. The demand handlers must gate on memory because they buffer a contiguous response;
  **prefetch returns no body and must not take a `mem_permit`** (its memory is already bounded by the
  scheduler).
- **Shared validation** already lives in the lib crate (`rust/object-cache/src/validation.rs`),
  the reuse point for request-type sharing between server and client.
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
    /// None or empty = warm the whole object. Present = warm only these ranges.
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
issue's "optionally with ranges" wording.

### 2. Core convenience: whole-object prefetch

`prefetch_ranges` needs explicit ranges, but the whole-object case (used by #1201 write warming)
doesn't know the size. Add one small method to `RangeCache` (keeps size resolution in the core, DRY):

```rust
pub async fn prefetch_object(&self, key: &str) -> Result<()> {
    let size = self.size(key).await?;
    if size == 0 { return Ok(()); }
    self.prefetch_ranges(key, &[0..size]).await
}
```

### 3. Bounded prefetch queue + worker (backpressure)

A large trace can enqueue many GB of keys. The origin-fetch concurrency is already bounded by the
scheduler's `prefetch_permits`; what is *not* bounded is the number of items awaiting a permit (each
pending block holds an `InFlight` entry). Bound that with a queue and **load-shed on overflow** — the
correct semantics for best-effort prefetch, which must never apply backpressure to the caller's
write/query path.

- `PrefetchQueue`: a bounded `tokio::sync::mpsc::channel::<PrefetchItem>(capacity)`. The `Sender`
  goes in `AppState`; the `Receiver` is drained by a consumer task spawned at startup.
- Consumer loop drives fills at bounded concurrency (a `Semaphore` sized by
  `prefetch_worker_concurrency`), each fill calling `prefetch_object` or `prefetch_ranges`:

```text
while let Some(item) = rx.recv().await {
    let permit = worker_sem.clone().acquire_owned().await;   // bound in-flight fills
    let cache = cache.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let outcome = match item.ranges {
            None => cache.prefetch_object(&item.key).await,
            Some(rs) => cache.prefetch_ranges(&item.key, &to_ranges(rs)).await,
        };
        if let Err(e) = outcome {
            imetric!("object_cache_prefetch_fill_error", "count", 1);
            debug!("prefetch fill failed key={} : {e:?}", item.key);
        } else {
            imetric!("object_cache_prefetch_keys_warmed", "count", 1);
        }
    });
}
```

Worker concurrency is a soft knob; the hard ceiling remains the scheduler's `prefetch_permits`.

### 4. `POST /prefetch` handler

```text
prefetch_handler(State(state), body: Bytes) -> Result<Response, StatusCode>
```

1. Deserialize `PrefetchRequest`; malformed JSON → `400`.
2. Cap batch size: reject > `MAX_PREFETCH_KEYS_PER_REQUEST` with `400` (bounds per-request work on an
   authenticated endpoint). Cap ranges-per-key with the existing `MAX_RANGES_PER_REQUEST`.
3. For each item: `validate_key(&item.key, &state.allowed_prefixes)`; reject inverted/degenerate
   ranges (`s >= e`), matching the demand paths. A failing item is **skipped** (counted in
   `rejected`), not fatal — a batch with one bad key still warms the rest.
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
    async fn prefetch(&self, items: Vec<PrefetchItem>) -> anyhow::Result<()>;
}
```

Wiring an `Arc<dyn ObjectPrefetch>` into the analytics/daemon layers is #1200/#1201, not this issue;
defining the trait here fixes the contract they depend on (open/closed).

### 6. CLI / config additions (`cli.rs`)

Follow the existing env-var pattern (`MICROMEGAS_OBJECT_CACHE_*`) and validate at startup like the
other numeric knobs in `object_cache_srv.rs:39-68`:

- `MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY` (default `4096`) — bounded channel depth.
- `MICROMEGAS_OBJECT_CACHE_PREFETCH_WORKER_CONCURRENCY` (default `8`) — concurrent in-flight fills.

Reject `0` for either at startup (fatal config error), matching the existing guards.

## Implementation Steps

### Phase 1 — shared types + core convenience
1. Add `rust/object-cache/src/prefetch.rs` with `PrefetchItem`/`PrefetchRequest`/`PrefetchResponse`
   and the `ObjectPrefetch` trait; export from `rust/object-cache/src/lib.rs`.
2. Add `RangeCache::prefetch_object` (`range_cache.rs`).

### Phase 2 — server endpoint + queue
3. Add `prefetch_queue` module (or inline in `handlers.rs`) with the bounded `mpsc` + consumer-loop
   builder returning `(Sender, JoinHandle)`.
4. Extend `AppState` (`app_state.rs`) with `prefetch_tx: mpsc::Sender<PrefetchItem>` and the
   per-request key cap constant.
5. Add `prefetch_handler` to `handlers.rs` (validation, cap, `try_send`, `202` + counts, no
   mem_permit).
6. In `object_cache_srv.rs`: add the two CLI options + startup validation; build the queue/worker;
   store the sender in `AppState`; register `.route("/prefetch", post(prefetch_handler))` on
   `obj_router` (inside the auth layer).

### Phase 3 — client
7. Add `CacheClientStore::prefetch` inherent method and `impl ObjectPrefetch for CacheClientStore`
   (`client.rs`).

### Phase 4 — metrics + docs + tests
8. Metrics: `object_cache_prefetch_requests`, `object_cache_prefetch_keys_enqueued`,
   `object_cache_prefetch_dropped`, `object_cache_prefetch_keys_warmed`,
   `object_cache_prefetch_fill_error`, `range_cache_client_prefetch_error`.
9. Docs (below).
10. Tests (below).

## Files to Modify
- `rust/object-cache/src/prefetch.rs` (new) — shared types + `ObjectPrefetch` trait.
- `rust/object-cache/src/lib.rs` — export the new module.
- `rust/object-cache/src/range_cache.rs` — `prefetch_object`.
- `rust/object-cache/src/client.rs` — `prefetch` method + trait impl.
- `rust/object-cache-srv/src/app_state.rs` — queue sender + key cap.
- `rust/object-cache-srv/src/handlers.rs` — `prefetch_handler` (+ queue/worker if inlined here).
- `rust/object-cache-srv/src/cli.rs` — two new options.
- `rust/object-cache-srv/src/object_cache_srv.rs` — startup validation, queue build, route.
- `rust/object-cache-srv/tests/prefetch_tests.rs` (new) — handler + client integration tests.
- `mkdocs/docs/admin/object-cache.md`, `rust/object-cache-srv/README.md` — endpoint + env docs.

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
- **No negative-cache coupling here.** Warming a key that doesn't exist yet just fails the fill
  quietly; the NotFound-TTL interaction is #1196/#1201, out of scope.

## Documentation
- `mkdocs/docs/admin/object-cache.md`: document `POST /prefetch` (body shape, `202` semantics,
  load-shedding) and add the two new env vars to the config table.
- `rust/object-cache-srv/README.md`: mirror the endpoint + env additions.
- `RangeCache` module doc / `prefetch_object` doc comment.
- Changelog entry.

## Testing Strategy
- **Unit** (`object-cache`): `PrefetchRequest`/`PrefetchResponse` serde round-trip;
  `prefetch_object` on a zero-byte object is a no-op.
- **Server integration** (`object-cache-srv/tests/prefetch_tests.rs`, using a counting/instrumented
  origin store like `memory_budget_tests.rs`'s `DelayedStore`):
  - `POST /prefetch` for uncached keys → `202`; after the fill drains, the blocks are present in the
    backend and a subsequent demand `get_range` of the same key issues **no** new origin GET (served
    from cache).
  - Fills run at prefetch priority: a saturating prefetch batch does not starve a concurrent demand
    read (reuse the priority assertions from the #1203 suite at the HTTP layer).
  - Queue full → excess items reported as `dropped`, `object_cache_prefetch_dropped` incremented,
    still `202`.
  - Batch with an out-of-prefix / inverted-range item → that item `rejected`, the valid ones warmed.
  - Prefetch acquires **no** `mem_permit`: a prefetch whose total bytes exceed `memory_budget_mb`
    still succeeds (contrast with the demand `413`).
- **Client round-trip**: spawn the axum app on an ephemeral port, point a `CacheClientStore` at it,
  call `prefetch`, assert warming as above; assert `prefetch` returns `Err` (and increments the
  client error metric) when the server is unreachable, without panicking.
- **CI**: `cd rust && python3 ../build/rust_ci.py` (fmt, clippy `-D warnings`, tests).
- **Smoke**: `start_minio.py` + `start_services.py`; `curl -XPOST /prefetch`, then confirm the demand
  read is a cache hit.

## Open Questions
1. **`ObjectPrefetch` trait now or with the first consumer?** Defining it here fixes the contract but
   adds unused surface until #1200/#1201. Recommendation: add it now (cheap, and it's the reuse
   point); acceptable to defer to #1200 if we'd rather not ship an unused trait.
2. **Whole-object prefetch scope for large objects.** `prefetch_object` warms every block of an
   object. For a multi-GB partition (#1201) that is a lot at once — do we want a per-object block cap
   here, or leave bounding to the queue + scheduler? Leaning: leave it to the queue/scheduler for
   #1198 and revisit caps in #1200 (which handles trace-sized enumeration).
3. **Response detail.** Is the `accepted/rejected/dropped` body useful to callers, or is a bare `202`
   with the detail only in metrics enough? Leaning: keep the small body — cheap and observable.
