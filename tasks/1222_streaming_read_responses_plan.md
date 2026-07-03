# Stream `/ranges` and Single-Range GET Responses (Issue #1222) Plan

## Overview

Rework `object-cache-srv`'s read path so response bytes are written to the socket as they are
fetched instead of being assembled whole in memory. This removes `MAX_TOTAL_REQUESTED_BYTES`
(512 MiB), both handlers' whole-budget rejections, and the fragile block-accounting in
`post_ranges_handler` that duplicates `get_ranges`'s dedup math. Per-request memory becomes a
fixed window (independent of request size) charged against the existing `mem_permits` budget.

Phase 2 of the #1218 rework; phase 1 (`/prefetch` NDJSON ingestion, #1218,
`tasks/1218_prefetch_ndjson_streaming_plan.md`) is independent and should land first.

## Current State

- `post_ranges_handler` (`rust/object-cache-srv/src/handlers.rs:239-364`): validates, then
  charges `mem_permits` for `max(framed response size, distinct blocks touched × block_size)` —
  the `touched_blocks` computation (`handlers.rs:298-306`) duplicates `get_ranges`'s block-dedup
  logic (`range_cache.rs:824-842`) to estimate peak retention. Rejects > 512 MiB
  (`MAX_TOTAL_REQUESTED_BYTES`) with 413. Then `cache.get_ranges()` materializes every range,
  and the handler assembles all of them into one `BytesMut` with 8-byte LE length prefixes,
  returned as a one-shot `PermitBody` that holds the permits for the body's lifetime.
- `get_range_handler` (`handlers.rs:93-232`): same disease — buffers the whole span, carries its
  own 512 MiB rejection (`handlers.rs:168-174`) and whole-budget check.
- `RangeCache::get_range` / `get_ranges` (`rust/object-cache/src/range_cache.rs:755-870`):
  size lookup → out-of-bounds validation → one `fetch_blocks(Demand)` call for **all** touched
  blocks (held simultaneously) → assemble.

Client side: `CacheClientStore::get_ranges` (`rust/object-cache/src/client.rs:403-472`) buffers
the response with `resp.bytes()`, walks the length-prefixed frames, and **already falls back to
the direct store on truncated framing** (`client.rs:452-467`) — this is what makes mid-stream
aborts safe.

Existing streaming precedent: the prefetch fill worker
(`rust/object-cache-srv/src/prefetch_queue.rs`) already streams the block-index space in lazy
`WINDOW_BLOCKS = 64` windows with `buffered(WINDOW_CONCURRENCY)` — the read path adopts the
same shape. `async-stream = "0.3"` is already a workspace dependency (`rust/Cargo.toml:37`).

## Design

**New core: `RangeCache::stream_ranges`** (in `range_cache.rs`):

```
pub async fn stream_ranges(&self, key: &str, ranges: Vec<Range<u64>>)
    -> Result<impl Stream<Item = Result<Bytes>> + Send>
```

- **Upfront (before the stream exists, so errors keep proper status codes):** `size()` lookup
  (→ 404 via `is_not_found`), out-of-bounds validation of every range (→ 416 via
  `RangeError::OutOfBounds`). Mirrors today's `get_ranges` prologue.
- **Stream body** (via `async_stream::try_stream!`): iterate ranges in request order; per
  range, iterate lazy block windows of `DEMAND_WINDOW_BLOCKS = 8` (8 MiB at the default 1 MiB
  block size — one coalesced origin run at the default `max_coalesced_get_bytes`); per window,
  `fetch_blocks(Demand)` → assemble the window∩range slice → yield `Bytes`. Windows are
  pipelined with `buffered(2)` (order-preserving) so the next window fetches while the previous
  flushes to the socket.
- Peak memory per stream ≈ `pipeline_depth × DEMAND_WINDOW_BLOCKS × block_size` — independent
  of request size. No cross-range block dedup (unlike today's `get_ranges`): a block shared by
  two ranges is re-requested, but the second request is a backend (RAM/SSD) hit or joins the
  in-flight fill via `own_or_join`, never a second origin GET.
- `get_range` and `get_ranges` are reimplemented as collectors over `stream_ranges` (one fill
  path, deletes their duplicated prologue). Their `Vec<Bytes>`/`Bytes` signatures and error
  behavior are unchanged for library consumers and tests.

**`post_ranges_handler` rework:**

- Keep: key validation, `MAX_RANGES_PER_REQUEST` (bounds real per-request work), inverted-range
  rejection. The request body stays a buffered `Bytes` (≤ 4096 × ~20 bytes ≈ 80 KiB, far below
  the default body limit).
- Delete: `MAX_TOTAL_REQUESTED_BYTES`, the `response_bytes`/`touched_blocks`/`charged_bytes`
  accounting (`handlers.rs:272-316`), and the whole-budget 413.
- **Memory accounting:** acquire a fixed `permits_for_bytes(2 × DEMAND_WINDOW_BLOCKS ×
  block_size)` per streaming request (≈16 permits at defaults), held by the response-body
  wrapper for the body's full lifetime — `PermitBody` generalizes from `Option<Bytes>` to
  wrapping the framed stream. `mem_permits` semantics shift from "assembled response bytes" to
  "in-flight window bytes"; `memory_budget_mb` (default 1024) then bounds concurrent streaming
  requests (~64 at defaults) instead of concurrent buffered bytes.
- **Framing stays byte-identical on the wire:** every range's length is known upfront
  (`end - start`), so the handler emits the 8-byte LE prefix for each range, then forwards that
  range's data chunks, counting bytes to know when the next prefix is due. Old clients read new
  responses unchanged (they never relied on `Content-Length`; the response becomes chunked).
- **Commit-before-stream:** await the stream's first item before building the `Response`, then
  re-chain it (`stream::once(...).chain(rest)`). A dead origin thus still surfaces as `500`
  rather than an aborted `200`. After the first byte is sent, a mid-stream fill error ends the
  stream with an error → hyper aborts the connection → the client sees truncated framing and
  falls back to direct (`client.rs:452-467`) — the existing failure path, not a new error mode.
- Metrics: `object_cache_ranges_bytes_served` accumulates as chunks are yielded (count in the
  framing loop) and is emitted when the stream completes.

**`get_range_handler` rework:** same stream with the single range and no framing. All existing
header logic (206, `Content-Range`, zero-byte and empty-range 200 special cases) is unchanged
and computed upfront; `Content-Length` is still set explicitly (span length is known). Delete
the 512 MiB check (`handlers.rs:168-174`) and the whole-budget check; acquire the same fixed
window permits.

**Client:** no changes required. `object_store::ObjectStore::get_ranges` returns `Vec<Bytes>`,
so the client materializes the response regardless; the win is server-side memory. Truncation
handling already exists.

## Acceptance Criteria

1. Every 4xx-producing check (key validation, range count, inverted ranges, size lookup,
   out-of-bounds) runs before the first response byte, and the first window is awaited before
   the 200 is committed, so a dead origin still returns 500.
2. A mid-stream fill error aborts the connection and the client's existing truncated-framing
   fallback recovers via the direct store — verified by an integration test.

## Implementation Steps

1. `rust/object-cache/Cargo.toml`: add `async-stream` (workspace dep; alphabetical order).
2. `rust/object-cache/src/range_cache.rs`: add `DEMAND_WINDOW_BLOCKS` and `stream_ranges`
   (upfront validation + windowed `try_stream!` with `buffered(2)`); reimplement `get_range` /
   `get_ranges` over it.
3. `rust/object-cache-srv/src/handlers.rs`:
   - Generalize `PermitBody` to wrap a stream (permit still dropped with the body).
   - Rewrite `post_ranges_handler`: keep input validation; fixed window permits; framed stream
     with interleaved prefixes; first-item await before committing the response; delete
     `MAX_TOTAL_REQUESTED_BYTES` and the block-accounting section.
   - Rewrite `get_range_handler` body path on the same stream; delete its size caps.
4. Rework `rust/object-cache-srv/tests/memory_budget_tests.rs` (see Testing — several 413
   assertions become "now succeeds" assertions).
5. Docs: `mkdocs/docs/admin/object-cache.md` ("Fetch scheduling & memory bounds" — describe the
   per-stream window bound; remove 512 MiB mentions), `rust/object-cache-srv/README.md`
   (endpoints table note that `/ranges` responses are chunked/streamed).

## Files to Modify

- `rust/object-cache/Cargo.toml`
- `rust/object-cache/src/range_cache.rs`
- `rust/object-cache-srv/src/handlers.rs`
- `rust/object-cache-srv/tests/memory_budget_tests.rs`
- `rust/object-cache/tests/range_cache_tests.rs` (new `stream_ranges` coverage)
- `mkdocs/docs/admin/object-cache.md`
- `rust/object-cache-srv/README.md`

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
- **Fixed per-stream permit charge (vs per-window acquire/release):** slightly conservative
  (a stream near completion still holds its full window charge) but avoids permit churn per
  window and keeps the `PermitBody` lifetime pattern that already covers abort-mid-body.
- **Mid-stream fill error → connection abort (vs buffering to guarantee status codes):**
  buffering is exactly what this issue removes. Mitigated by upfront validation (all 4xx paths
  precede the first byte) and the first-window await (origin-down still yields 500); the
  residual case degrades to the client's existing truncation → direct fallback.

## Documentation

- `mkdocs/docs/admin/object-cache.md`: memory-bounds section; remove 512 MiB / assembled-response
  mentions.
- `rust/object-cache-srv/README.md`: endpoints table (chunked `/ranges` responses).

## Testing Strategy

- `object-cache/tests/range_cache_tests.rs`: `stream_ranges` content correctness across
  multi-window ranges, range boundaries not block-aligned, multiple ranges sharing blocks,
  upfront 404/OutOfBounds errors, mid-stream origin failure surfaces as a stream `Err`.
- `object-cache-srv/tests/memory_budget_tests.rs` rework:
  - `oversize_request_rejected_413` / `scattered_small_ranges_charge_blocks_touched` →
    replaced: requests that previously 413'd now stream successfully with byte-correct framing.
  - `permit_released_on_body_drop` → adapted to the stream-wrapping `PermitBody` (drop the body
    mid-stream, assert permits return).
  - New: concurrent streams gate on `memory_budget_mb` via the fixed per-stream charge;
    origin-down before first byte → 500; origin failure after the first window → truncated
    body, and `CacheClientStore::get_ranges` against the served router falls back to direct
    and returns correct data.
- Full suite: `cargo test -p micromegas-object-cache -p micromegas-object-cache-srv`, then
  `python3 ../build/rust_ci.py` before the PR.

## Open Questions

1. **`DEMAND_WINDOW_BLOCKS = 8` and pipeline depth 2** are chosen to match one coalesced origin
   run (`max_coalesced_get_bytes` default) with modest overlap. Should the window be a CLI
   flag like the prefetch knobs, or are constants fine until profiling says otherwise?
   (Plan assumes constants.)
2. Whether to also delete `MAX_RANGES_PER_REQUEST` is explicitly **out of scope** per #1218 —
   it bounds real per-request work, not body size.
