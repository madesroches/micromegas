# Stream `/ranges` and Single-Range GET Responses (Issue #1222) Plan

## Overview

Rework `object-cache-srv`'s read path so response bytes are written to the socket as they are
fetched instead of being assembled whole in memory. This removes `MAX_TOTAL_REQUESTED_BYTES`
(512 MiB), both handlers' whole-budget rejections, and the fragile block-accounting in
`post_ranges_handler` that duplicates `get_ranges`'s dedup math. Per-request memory is charged
proportionally to the response size, capped at a fixed window, against the existing
`mem_permits` budget.

Phase 2 of the #1218 rework; phase 1 (`/prefetch` NDJSON ingestion, #1218,
`tasks/completed/1218_prefetch_ndjson_streaming_plan.md`) has already landed (commit
`c867c3134`) — this plan builds on it.

## Current State

- `post_ranges_handler` (`rust/object-cache-srv/src/handlers.rs:245-370`): validates, then
  charges `mem_permits` for `max(framed response size, distinct blocks touched × block_size)` —
  the `touched_blocks` computation (`handlers.rs:304-310`) duplicates `get_ranges`'s block-dedup
  logic (`range_cache.rs:824-842`) to estimate peak retention. Rejects > 512 MiB
  (`MAX_TOTAL_REQUESTED_BYTES`) with 413. Then `cache.get_ranges()` materializes every range,
  and the handler assembles all of them into one `BytesMut` with 8-byte LE length prefixes
  (`handlers.rs:334-338`), returned as a one-shot `PermitBody` (`handlers.rs:51-69`) that holds
  the permits for the body's lifetime.
- `get_range_handler` (`handlers.rs:99-238`): same disease — buffers the whole span, carries its
  own 512 MiB rejection (`handlers.rs:175-180`) and whole-budget check (`handlers.rs:182-190`).
- `RangeCache::get_range` / `get_ranges` (`rust/object-cache/src/range_cache.rs:754-872`):
  size lookup → out-of-bounds validation → one `fetch_blocks(Demand)` call for **all** touched
  blocks (held simultaneously) → assemble.

Client side: `CacheClientStore::get_ranges` (`rust/object-cache/src/client.rs:406-475`) buffers
the response with `resp.bytes()`, walks the length-prefixed frames, and **already falls back to
the direct store on truncated framing** (`client.rs:457-472`) — this is what makes mid-stream
aborts safe.

Existing streaming precedent: the prefetch fill worker
(`rust/object-cache-srv/src/prefetch_queue.rs`) already streams the block-index space in lazy
`WINDOW_BLOCKS = 64` windows with `buffered(WINDOW_CONCURRENCY)` — the read path adopts the
same shape. `async-stream = "0.3"` is already a workspace dependency (`rust/Cargo.toml:37`).

## Design

**New core: `RangeCache::stream_ranges`** (in `range_cache.rs`):

```
pub async fn stream_ranges(&self, key: &str, ranges: Vec<Range<u64>>, caller: StreamRangesCaller)
    -> Result<impl Stream<Item = Result<Bytes>> + Send + 'static>
```

`StreamRangesCaller` is a two-variant enum (`Range` / `Ranges`) selecting which of the two
existing error-metric names `stream_ranges` emits on its upfront-validation failures and on a
mid-stream `fetch_blocks` failure — see below.

- Takes `&self` only for the upfront lookup/validation; the `try_stream!` body is built from an
  owned `self.clone()` (`RangeCache` is cheaply `Clone` — `Arc`-shared handles plus small scalars —
  `range_cache.rs:428-434`)
  moved into the stream, matching the pattern `prefetch_queue.rs:84,124` already use to get an
  owned, `'static` stream future. This is required because axum's `Body::from_stream` (used by
  the handlers below) needs `Send + 'static`, and a stream capturing `&self` cannot satisfy that.
- **Upfront (before the stream exists, so errors keep proper status codes):** `size()` lookup
  (→ 404 via `is_not_found`), out-of-bounds validation of every range (→ 416 via
  `RangeError::OutOfBounds`). Mirrors today's `get_ranges` prologue.
- **Stream body** (via `async_stream::try_stream!`): iterate ranges in request order, skipping
  any degenerate `start >= end` range without calling `blocks_for_range` (which
  `debug_assert!(start < end)`s and computes `end - 1`, panicking/underflowing on such input) —
  it simply yields nothing for that range, matching the existing degenerate-range skips in
  `get_range`/`get_ranges`/`prefetch_ranges` (`start >= end` return at `range_cache.rs:776`;
  `start < end` inclusion guards at `range_cache.rs:836,896`; `get_ranges`'s actual
  degenerate→`Bytes::new()` return is at `range_cache.rs:860`). For the remaining
  ranges, iterate lazy block windows of `DEMAND_WINDOW_BLOCKS = 8` (8 MiB at the default 1 MiB
  block size — one coalesced origin run at the default `max_coalesced_get_bytes`); per window,
  `fetch_blocks(Demand)` → assemble the window∩range slice → yield `Bytes`. Windows are
  pipelined with `buffered(2)` (order-preserving) so the next window fetches while the previous
  flushes to the socket.
- Peak memory per stream ≈ `pipeline_depth × DEMAND_WINDOW_BLOCKS × block_size` — independent
  of request size. No cross-range block dedup (unlike today's `get_ranges`): a block shared by
  two ranges is re-requested, but the second request is a backend (RAM/SSD) hit or joins the
  in-flight fill via `own_or_join`, never a second origin GET.
- `get_range` and `get_ranges` are reimplemented as collectors over `stream_ranges` (one fill
  path, deletes their duplicated prologue). Both handlers below call `stream_ranges` directly
  (not through these collectors), so the `range_cache_get_range_error` /
  `range_cache_get_ranges_error` `imetric!` emissions from that prologue
  (`range_cache.rs:759,768,789` and `819,829,851`) must be emitted **inside `stream_ranges`
  itself**, not in the two collectors — emitting only in the collectors would silently drop both
  metrics from the production HTTP paths, which never touch the collectors. To keep the two
  distinct metric names, `stream_ranges` takes a `caller: StreamRangesCaller` tag and emits
  `range_cache_get_range_error` for `StreamRangesCaller::Range` or
  `range_cache_get_ranges_error` for `StreamRangesCaller::Ranges` both on its upfront `size()`/
  out-of-bounds failures and on a mid-stream window's `fetch_blocks` call returning `Err` (yielded
  into the stream as the terminal item, matching `range_cache.rs:789,851` today); `get_range`/
  `get_range_handler` pass `Range` and `get_ranges`/
  `post_ranges_handler` pass `Ranges`, so both metrics keep firing exactly as today on all four
  call sites — except `get_ranges`'s `if ranges.is_empty() { return
  Ok(vec![]) }` short-circuit (`range_cache.rs:809-811`), which must be kept as-is *before*
  calling `stream_ranges`: `stream_ranges` does its `size()` lookup upfront regardless of
  `ranges`, so dropping the short-circuit would turn an empty-ranges call against a missing or
  failing key from `Ok(vec![])` into an error. Their `Vec<Bytes>`/`Bytes` signatures and error
  behavior are unchanged for library consumers and tests. Contract note: `get_ranges` must still
  return exactly one `Bytes` per input range (including `Bytes::new()` for a degenerate
  `start >= end` range, which today's `get_ranges` emits and its tests assert). Since
  `stream_ranges` yields nothing for a degenerate range (see above), the `get_range`/`get_ranges`
  collectors must special-case `start >= end` themselves and reinsert `Bytes::new()` at that
  position rather than relying on a chunk from the stream — matching current behavior. For
  non-degenerate ranges, `stream_ranges` yields a flat chunk sequence in range order, and the
  collector reconstructs the per-range split from the known input range lengths — so
  `stream_ranges` must preserve range ordering and either delimit ranges or emit them
  contiguously (the handler's framing loop relies on the same property).

**`post_ranges_handler` rework:**

- Keep: key validation, `MAX_RANGES_PER_REQUEST` (bounds real per-request work), inverted-range
  rejection. The request body stays a buffered `Bytes` (≤ 4096 × ~20 bytes ≈ 80 KiB, far below
  the default body limit).
- **Empty-ranges short-circuit:** mirror the `get_ranges` collector guard — if `req.ranges` is
  empty, return the empty `200` directly without calling `stream_ranges`. `stream_ranges` does
  its `size()` lookup upfront regardless of `ranges`, so without this guard a `{"ranges":[]}`
  request against a missing key would flip from today's `200` (empty body) to `404`.
- Delete: `MAX_TOTAL_REQUESTED_BYTES`, the `response_bytes`/`touched_blocks`/`charged_bytes`
  accounting (`handlers.rs:278-322`), and the whole-budget 413.
- **Memory accounting:** acquire `permits_for_bytes(min(framed_response_size, 2 ×
  DEMAND_WINDOW_BLOCKS × block_size))` per streaming request — a proportional charge capped at
  the window (≈16 permits at defaults) — held by the response-body wrapper for the body's full
  lifetime — `PermitBody` generalizes from `Option<Bytes>` to wrapping the framed stream.
  `framed_response_size` is already computed upfront during validation (the `/ranges` path sums
  every range's length while validating; add the fixed per-range framing prefix overhead; the
  single-range GET path knows the span length upfront), so this is a single `min()` over a value
  already on hand, not new per-block accounting. `mem_permits` semantics shift from "assembled
  response bytes" to "in-flight window bytes"; a small (≤1 MiB) read still charges ~1 permit, so
  the default 1024-permit budget still allows ~1024 concurrent small reads, while a large stream
  clamps to the window and `memory_budget_mb` (default 1024) bounds concurrent large streaming
  requests (~64 at defaults) instead of concurrent buffered bytes. Block-granularity caveat: a
  sub-block read transiently holds one whole block (`block_size`), but `permits_for_bytes`
  rounds up (`div_ceil`), so any ≤1 MiB read → 1 permit = 1 MiB budget, which covers the fetched
  block; only pathological scattered sub-block ranges (many tiny ranges in distinct blocks) could
  under-count actual in-flight bytes, and that's bounded by `MAX_RANGES_PER_REQUEST` and
  coalescing. Guard against under-provisioned budgets: at startup, clamp/validate
  `memory_budget_mb` so `mem_permits`'s total is at least the window-sized charge (`2 ×
  DEMAND_WINDOW_BLOCKS × block_size`) — a large read still charges the full window, and
  `Semaphore::acquire_many_owned` never completes (and never errors) if the requested count
  exceeds the semaphore's total permits, so without this floor any deployment configured with a
  smaller `--memory-budget-mb` would hang every large read instead of failing fast.
- **Framing stays byte-identical on the wire:** every range's length is known upfront
  (`end - start`), so the handler emits the 8-byte LE prefix for each range, then forwards that
  range's data chunks, counting bytes to know when the next prefix is due. Old clients read new
  responses unchanged (they never relied on `Content-Length`; the response becomes chunked).
- **Commit-before-stream:** await the stream's first item before building the `Response`, then
  re-chain it (`stream::once(...).chain(rest)`). A dead origin thus still surfaces as `500`
  rather than an aborted `200`. After the first byte is sent, a mid-stream fill error ends the
  stream with an error → hyper aborts the connection → the client sees truncated framing and
  falls back to direct (`client.rs:457-472`) — the existing failure path, not a new error mode.
- Calls `stream_ranges(key, ranges, StreamRangesCaller::Ranges)`, so `range_cache_get_ranges_error`
  keeps firing on upfront `size()`/out-of-bounds failures for this path (see Design above).
- Metrics: all three existing metrics keep today's success-only semantics (`handlers.rs:340-346`
  emits them only from the `Ok` arm; `Err` emits nothing). `object_cache_ranges_requests` and
  `object_cache_ranges_count` are emitted at the same point the response is committed — right
  after the stream's first item is successfully awaited (see commit-before-stream, above) — so a
  `404`/`416` from the upfront `size()`/bounds checks, or a `500` from a dead origin on the first
  window, never increments them, exactly as today. `object_cache_ranges_bytes_served` accumulates
  as chunks are yielded (count in the framing loop) and is emitted when the stream completes.
  Accepted behavior change: a client that aborts mid-stream causes the stream to be dropped before
  completion, so `object_cache_ranges_bytes_served` is skipped for that request — a minor
  observability regression versus today's always-emitted total, accepted as intentional.

**`get_range_handler` rework:** same stream with the single range and no framing, calling
`stream_ranges(key, vec![range], StreamRangesCaller::Range)` so `range_cache_get_range_error`
keeps firing on upfront `size()`/out-of-bounds failures for this path (see Design above). All
existing header logic (206, `Content-Range`, zero-byte and empty-range 200 special cases) is
unchanged and computed upfront; `Content-Length` is still set explicitly (span length is known).
Metrics: mirroring `/ranges`, `object_cache_get_requests` keeps today's success-only semantics
(`handlers.rs:201` emits it only from the `Ok` arm): it's emitted at the same commit point as
`post_ranges_handler` — right after the first window is successfully awaited, before the 206/200
response is built — so a `404`/`416` from the upfront checks, or a `500` from a dead origin on
that first window, never increments it. `object_cache_get_bytes_served` likewise accumulates from
the bytes actually yielded (mirroring `object_cache_ranges_bytes_served`) rather than being
emitted upfront with the full span length; a mid-stream abort under-reports it instead of
over-reporting, the same accepted skew as `/ranges`'s bytes-served regression above, not a new
one. Delete the 512 MiB check (`handlers.rs:175-180`) and the whole-budget check (`handlers.rs:182-190`);
acquire the same proportional charge (`min(span length, window)`). Like `post_ranges_handler`,
the first window is awaited before the 206 (or 200) response is committed — same
commit-before-stream pattern, and the point at which `object_cache_get_requests` fires — so a dead
origin surfaces as `500` here too, not just on the `/ranges` path. **Full-object (unranged) GETs
are a genuinely new mid-stream failure mode, not a pre-existing one:** the handler synthesizes a
full range for unranged requests (`handlers.rs:133-136`), and on the client side those flow
through `get_full_stream` → `stream_get_result` (`client.rs:119-133,220-240`), which streams the
body straight through and maps a mid-stream error to `object_store::Error::Generic` with no
direct-store fallback — unlike the bounded single-range GET path, which buffers via
`resp.bytes()` and falls back at `client.rs:396-403`. This plan keeps that gap explicit rather
than silently introducing it, but the fallback `get_full_stream` gains is narrower than the
bounded-range path's: `get_full_stream` buffers only up to the *first* chunk before handing
anything to the consumer, so if the stream errors before that first chunk is yielded downstream,
it retries the whole object against the origin (safe: zero bytes have reached the consumer yet,
same precondition `get_range_bytes` relies on). Once a chunk has been yielded downstream, a
retry is unsound (it would re-deliver already-emitted prefix bytes from offset 0), so a
mid-stream error after the first chunk simply terminates the stream with an error — no retry.
The two GET paths therefore do not have equivalent mid-stream recovery in general; they agree
only on the pre-first-byte case, which is the one that matters for silently returning wrong data.

**Client:** `object_store::ObjectStore::get_ranges` returns `Vec<Bytes>`, so the client
materializes the response regardless; the win is server-side memory, and truncation handling
already exists for that path. The one required change is `get_full_stream` (full, unranged GET):
add a fallback to the direct store that fires only if the stream errors before its first chunk
is yielded downstream (mirroring the buffered-then-fallback precondition of the bounded-range
path at `client.rs:396-403`); once a chunk has reached the consumer, a mid-stream error must
terminate the stream rather than retry, since the handler rework makes full-object GETs stream
from the same code path as ranged ones (see `get_range_handler` rework above).

## Acceptance Criteria

1. Every 4xx-producing check (key validation, range count, inverted ranges, size lookup,
   out-of-bounds) runs before the first response byte, and the first window is awaited before
   the 200 is committed, so a dead origin still returns 500.
2. A mid-stream fill error aborts the connection and the client's existing truncated-framing
   fallback recovers via the direct store — verified by an integration test.

## Implementation Steps

1. `rust/object-cache/Cargo.toml` and `rust/object-cache-srv/Cargo.toml`: add `async-stream`
   (workspace dep; alphabetical order) — the srv crate needs it too since the framed-stream
   handler rewrite (step 3) uses the `async_stream::try_stream!` macro and `object-cache-srv`'s
   `Cargo.toml` doesn't currently depend on it.
2. `rust/object-cache/src/range_cache.rs`: add `DEMAND_WINDOW_BLOCKS`, `StreamRangesCaller`
   (`Range` / `Ranges`), and `stream_ranges` (upfront validation on `&self`, emitting
   `range_cache_get_range_error`/`range_cache_get_ranges_error` per the `caller` tag on
   validation failure, then windowed `try_stream!` with `buffered(2)` built over an owned
   `self.clone()` so the returned stream is `Send + 'static`, emitting the same `caller`-tagged
   metric if a window's `fetch_blocks` call fails); reimplement `get_range` /
   `get_ranges` over it, passing their respective `StreamRangesCaller` variant.
3. `rust/object-cache-srv/src/handlers.rs`:
   - Generalize `PermitBody` to wrap a stream (permit still dropped with the body).
   - Rewrite `post_ranges_handler`: keep input validation; empty-ranges short-circuit (return
     empty `200` before calling `stream_ranges`, preserving today's behavior for a missing key);
     proportional per-stream permit charge (`min(framed_response_size, window)`); framed stream
     with interleaved prefixes; first-item await before committing the response; delete
     `MAX_TOTAL_REQUESTED_BYTES` and the block-accounting section.
   - Rewrite `get_range_handler` body path on the same stream; delete its size caps.
   - Remove the now-unused `blocks_for_range` import (the block-accounting section being deleted
     was its only call site).
4. `rust/object-cache-srv/src/object_cache_srv.rs`: add a startup clamp/validate check, next to
   the existing `memory_budget_mb == 0` guard (lines 64–71) and before `AppState::new(...)` is
   constructed (line 169), ensuring `mem_permits`'s total is at least the window-sized charge
   (`2 × DEMAND_WINDOW_BLOCKS × block_size`) that a large read still charges in full — otherwise
   a small `--memory-budget-mb` makes `Semaphore::acquire_many_owned` hang forever instead of
   failing fast at startup. This requires two items to be visible outside their defining modules:
   make `DEMAND_WINDOW_BLOCKS` (`range_cache.rs`) `pub`, like `DEFAULT_MAX_COALESCED_GET_BYTES`;
   and expose the bytes→permits conversion (`permits_for_bytes` / `BYTES_PER_MEM_PERMIT`,
   currently private in `handlers.rs`) as a `pub` helper (or `pub const`) shared by both the
   startup floor check and the handler's proportional per-stream charge, so the two compute the
   same value from one formula.
5. `rust/object-cache/src/client.rs`: add mid-stream direct-store fallback to `get_full_stream`
   (full, unranged GET), mirroring the bounded-range fallback at `client.rs:396-403`.
6. Rework `rust/object-cache-srv/tests/memory_budget_tests.rs` (see Testing — several 413
   assertions become "now succeeds" assertions).
7. Docs and doc comments: audit every file under `rust/object-cache-srv/src/` (not just the two
   doc files below) for stale references to `MAX_TOTAL_REQUESTED_BYTES`, the 512 MiB
   total-requested-bytes cap, its `413 Payload Too Large`, and "assembled-response"/
   "concurrently-assembled" budget phrasing — remove or reword each occurrence, not just the ones
   called out below. This includes `rust/object-cache-srv/src/app_state.rs`'s `mem_permits` and
   `memory_budget_mb` doc comments, which currently describe "concurrently-assembled response
   bytes" and a 413 rejection; reword both to the in-flight-window semantics. Concrete examples of
   the same sweep in the two doc files: in `rust/object-cache-srv/README.md`: the endpoints-table
   note (add that `/ranges` responses are chunked/streamed — keep this note), the request-limits
   prose (currently lines 29–31, "A single request is capped at 4096 ranges and 512 MiB of total
   requested bytes..." — drop the 512 MiB / 413 clause, keep the `MAX_RANGES_PER_REQUEST`
   (4096-range) cap mention), and the `--memory-budget-mb` Configuration-table row (line 76,
   reword from "concurrently-assembled response bytes" to bounding concurrent in-flight streaming
   windows). In `mkdocs/docs/admin/object-cache.md`: the Configuration/env-var table row and the
   "Fetch scheduling & memory bounds" section (describe the per-stream window bound; remove 512
   MiB / 413 / assembled-response mentions). `MAX_RANGES_PER_REQUEST` (4096-range cap) is retained
   and should stay documented wherever it's mentioned.

## Files to Modify

- `rust/object-cache/Cargo.toml`
- `rust/object-cache-srv/Cargo.toml`
- `rust/object-cache/src/range_cache.rs`
- `rust/object-cache-srv/src/handlers.rs`
- `rust/object-cache-srv/src/object_cache_srv.rs` (startup clamp/validate `memory_budget_mb`
  floor, next to the existing `memory_budget_mb == 0` guard)
- `rust/object-cache-srv/src/app_state.rs` (reword `mem_permits` / `memory_budget_mb` doc
  comments to the in-flight-window semantics)
- `rust/object-cache/src/client.rs` (mid-stream fallback for `get_full_stream`)
- `rust/object-cache-srv/tests/memory_budget_tests.rs`
- `rust/object-cache/tests/range_cache_tests.rs` (new `stream_ranges` coverage)
- `mkdocs/docs/admin/object-cache.md`
- `rust/object-cache-srv/README.md`
- any other file under `rust/object-cache-srv/src/` found during the doc-comment audit to
  reference the removed 512 MiB cap, `413`, or assembled-response phrasing

## Trade-offs

- **Windowed streaming loses cross-range block dedup and whole-request fetch parallelism:**
  overlapping ranges re-hit the backend (cheap: RAM/SSD or in-flight join, never a duplicate
  origin GET). Cold-read parallelism per request drops from "all coalesced runs at once" to
  `pipeline_depth × window` (2 × 8 MiB at defaults); this is the inherent price of bounding
  per-request memory, and `DEMAND_WINDOW_BLOCKS`/pipeline depth are the tuning knobs. Aggregate
  throughput across concurrent requests is still governed by the fetch scheduler.
- **`get_range`/`get_ranges` reimplemented over the stream (vs kept as a parallel path):** one
  fill path (DRY) at the cost of the same parallelism note above for library consumers; their
  signatures and error contracts are preserved.
- **Proportional per-stream charge capped at the window (vs per-window acquire/release):** the
  charge is `permits_for_bytes(min(framed_response_size, window))`, a single upfront `min()` over
  a size the handler already has, not a per-block accounting scheme. A small (≤1 MiB) read still
  charges ~1 permit, so the default 1024 budget still allows ~1024 concurrent small reads —
  matching today. A large read clamps to the window (≈16 permits at defaults), so per-request
  in-flight memory stays bounded and large-read concurrency gates at ~64 regardless of how much
  bigger the response gets. The remaining trade-off is that this is still an upfront estimate
  held for the whole body lifetime (one acquire, no per-window churn, same `PermitBody` pattern
  that already covers abort-mid-body) rather than metering actual bytes fetched per window; the
  block-granularity caveat above (sub-block reads, pathological scattered-range requests) is the
  residual imprecision this leaves.
- **Mid-stream fill error → connection abort (vs buffering to guarantee status codes):**
  buffering is exactly what this issue removes. Mitigated by upfront validation (all 4xx paths
  precede the first byte) and the first-window await (origin-down still yields 500); the
  residual case degrades to the client's existing truncation → direct fallback for `/ranges` and
  bounded single-range GETs, and to the new `get_full_stream` fallback (added by this plan,
  pre-first-chunk only — a failure after the first chunk has been yielded downstream ends the
  stream in an error with no retry, since bytes already sent can't be un-sent) for full-object
  GETs.

## Documentation

Exhaustive sweep required across every file under `rust/object-cache-srv/src/` plus both doc
files — grep for `MAX_TOTAL_REQUESTED_BYTES`, "512 MiB", "413", and "assembled"/
"concurrently-assembled" to catch every occurrence, not only the ones listed here as concrete
examples:

- `rust/object-cache-srv/src/app_state.rs`: the `mem_permits` doc comment ("Cross-request bound
  on concurrently-assembled response bytes … before assembling a response") and the
  `memory_budget_mb` doc comment ("A request whose assembled size would exceed this outright is
  rejected (413)") — reword both to the in-flight-window semantics (permits bound concurrent
  streaming windows, not assembled response bytes; there is no 413 for size).
- `mkdocs/docs/admin/object-cache.md`: Configuration/env-var table row and the "Fetch scheduling
  & memory bounds" section; remove 512 MiB total-requested-bytes cap, its `413 Payload Too
  Large`, and assembled-response mentions.
- `rust/object-cache-srv/README.md`: endpoints table (add note that `/ranges` responses are
  chunked/streamed; keep this note); request-limits prose (lines 29–31) — drop the 512 MiB /
  413 clause, keep the `MAX_RANGES_PER_REQUEST` (4096-range) cap mention; Configuration table
  `--memory-budget-mb` row (line 76) — reword from "concurrently-assembled response bytes" to
  bounding concurrent in-flight streaming windows.
- Any other file under `rust/object-cache-srv/src/` that references the removed cap, the `413`
  rejection, or assembled-response phrasing.

## Testing Strategy

- `object-cache/tests/range_cache_tests.rs`: `stream_ranges` content correctness across
  multi-window ranges, range boundaries not block-aligned, multiple ranges sharing blocks,
  upfront 404/OutOfBounds errors, mid-stream origin failure surfaces as a stream `Err`.
- `object-cache-srv/tests/memory_budget_tests.rs` rework:
  - `oversize_request_rejected_413` / `scattered_small_ranges_charge_blocks_touched` →
    replaced: requests that previously 413'd now stream successfully with byte-correct framing.
  - `permit_released_on_body_drop` → adapted to the stream-wrapping `PermitBody` (drop the body
    mid-stream, assert permits return).
  - `concurrent_large_reads_gate_on_budget` → reworked: it currently asserts `PARTIAL_CONTENT`
    under a small budget based on per-read-size permit accounting, which the proportional
    `min(response, window)` charge replaces. Since small reads no longer charge the full window,
    gating two concurrent streams now requires two *large* (≥ window) reads, each charging the
    full window; set `memory_budget_mb` between one and two windows (e.g. ~16–31) so exactly two
    large concurrent streams gate. Note small reads will *not* gate at these budgets — that's the
    point of the proportional charge.
  - New: two concurrent large reads gate on `memory_budget_mb` via the window-capped charge;
    origin-down before first byte → 500; origin failure after the first window → truncated
    body, and `CacheClientStore::get_ranges` against the served router falls back to direct
    and returns correct data.
  - Reworked tests must set `memory_budget_mb` (via `make_state`) at or above the window-sized
    charge (e.g. 16–31 to gate exactly two concurrent large streams) — `make_state` constructs
    `AppState` directly and bypasses the startup floor guard in `object_cache_srv.rs`, so a
    budget below the window (the existing tests use 1/2/4) would make `acquire_many_owned` block
    forever for a large read instead of gating.
- Full suite: `cargo test -p micromegas-object-cache -p micromegas-object-cache-srv`, then
  `python3 ../build/rust_ci.py` before the PR.

## Open Questions

1. **`DEMAND_WINDOW_BLOCKS = 8` and pipeline depth 2** are chosen to match one coalesced origin
   run (`max_coalesced_get_bytes` default) with modest overlap. Should the window be a CLI
   flag like the prefetch knobs, or are constants fine until profiling says otherwise?
   (Plan assumes constants.)
2. Whether to also delete `MAX_RANGES_PER_REQUEST` is explicitly **out of scope** per #1218 —
   it bounds real per-request work, not body size.
3. **Resolved:** an earlier draft of this plan charged a fixed ~16-permit window per stream
   regardless of request size, which would have cut max concurrent small (≤1 MiB) reads by
   ~16x versus today (~1024 → ~64 at default `memory_budget_mb`). The plan instead adopts a
   proportional `permits_for_bytes(min(framed_response_size, window))` charge (see Memory
   accounting and Trade-offs above): small reads keep today's ~1024 concurrency, while large
   reads still clamp to the window so per-request memory stays bounded.
