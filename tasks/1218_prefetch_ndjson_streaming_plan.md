# Stream `/prefetch` Input as NDJSON (Issue #1218) Plan

## Overview

Rework `object-cache-srv`'s `/prefetch` request ingestion so items are parsed incrementally as
bytes arrive instead of buffering and deserializing the whole body. This removes the need for a
request-body size ceiling (today: axum's implicit 2 MiB `DefaultBodyLimit`) — resource safety
comes from the existing bounded prefetch queue, and the only remaining ceiling is a cap on one
NDJSON line, a naturally-sized unit. Timing matters: the endpoint landed in #1220 and its only
consumer is `CacheClientStore` in this repo, so the wire format is still free to change.

Phase 1 of the #1218 rework. Phase 2 — streaming the `/ranges` and single-range `GET` responses
— is tracked in #1222 (`tasks/1222_streaming_read_responses_plan.md`).

## Current State

`prefetch_handler` (`rust/object-cache-srv/src/handlers.rs:371-457`) takes `body: Bytes`,
deserializes one `PrefetchRequest { keys: Vec<PrefetchItem> }`
(`rust/object-cache/src/prefetch.rs`), then loops: per item — `validate_key`, range-count cap
(`MAX_RANGES_PER_REQUEST` = 4096), range bounds vs `item.size`, then
`prefetch_tx.try_send(item)` onto the bounded queue (full → `dropped`). Responds `202` with
`PrefetchResponse { accepted, rejected, dropped }`.

Nothing in the loop needs the whole batch resident; the only reason the body is buffered is the
single-document JSON format. Batch size is bounded only by axum's implicit 2 MiB
`DefaultBodyLimit` (`obj_router` in `rust/object-cache-srv/src/object_cache_srv.rs:188-192`
never configures one).

Client side: `CacheClientStore::prefetch` (`rust/object-cache/src/client.rs:156-186`)
serializes `PrefetchRequest` as one JSON body. Best-effort contract: an `Err` means "the warm
didn't happen", callers don't retry.

## Design

**Wire format.** `Content-Type: application/x-ndjson`; one `PrefetchItem` JSON object per
`\n`-terminated line. The `PrefetchRequest` wrapper type is deleted (it exists only for the old
wire format). `PrefetchItem` and `PrefetchResponse` are unchanged; the response is still a
single `202` JSON body after the request stream is fully consumed.

**Handler.** Signature changes from `body: Bytes` to `body: axum::body::Body`; consume
`body.into_data_stream()` into a `BytesMut` line buffer, splitting on `\n`:

- Each complete line: skip if blank/whitespace; else `serde_json::from_slice::<PrefetchItem>`.
  - Parse failure → `rejected += 1`, continue (framing is newline-based, so one bad line cannot
    corrupt the next; this matches the per-item validation semantics).
  - Parse success → existing per-item validation + `try_send`, verbatim from today's loop body.
- **Per-line cap** `MAX_PREFETCH_LINE_BYTES = 1 MiB`: if the buffer exceeds it without a
  newline → `400`. This is the ceiling that remains — a bound on one item, a natural unit (an
  item at the 4096-range cap serializes to ~100 KiB, so 1 MiB is ~10× headroom), unlike a bound
  on "however many items a client batches". Items enqueued before the abort stay enqueued;
  prefetch is best-effort and warming is idempotent, so a partially-processed request is
  harmless.
- Body read error (client disconnect) → abort; final partial line without trailing `\n` is
  processed as a line at end-of-stream.

**Router.** No change needed: axum's `DefaultBodyLimit` only applies to extractors that buffer
the body (`Bytes`, `BytesMut`, `String`); the raw `axum::body::Body` extractor used by
`prefetch_handler` returns the body unchanged and was never subject to that limit. The per-line
`MAX_PREFETCH_LINE_BYTES` cap is the sole remaining guard. `/ranges` keeps using the `Bytes`
extractor, so the default limit still applies there — its input (the ranges list, ≤ 4096 × ~20
bytes ≈ 80 KiB) sits far below 2 MiB and `MAX_RANGES_PER_REQUEST` is the real bound.

**Client.** `CacheClientStore::prefetch` serializes each item followed by `\n` into one buffer
and sends it. No client-side streaming: the caller already holds `Vec<PrefetchItem>` in memory,
so `reqwest::Body::wrap_stream` would add complexity without reducing peak memory.

**Version skew** (both directions degrade to "no warm", acceptable for a best-effort endpoint):
- Old client → new server: the old body `{"keys":[...]}` is one line that fails to parse as
  `PrefetchItem` → `202` with `rejected = 1`.
- New client → old server: NDJSON fails the whole-body JSON parse → `400` → client debug-logs
  and moves on (existing error path).

## Implementation Steps

1. `rust/object-cache/src/prefetch.rs`: delete `PrefetchRequest`; update module/type docs to
   describe the NDJSON wire format.
2. `rust/object-cache/src/client.rs`: `prefetch()` serializes items as NDJSON lines,
   `Content-Type: application/x-ndjson`; drop the `PrefetchRequest` import.
3. `rust/object-cache-srv/src/handlers.rs`: rewrite `prefetch_handler` per the design —
   `Body` extractor, line buffer with `MAX_PREFETCH_LINE_BYTES`, per-line
   parse/validate/`try_send` (loop body unchanged), same `202`/`PrefetchResponse` tail.
4. Update `rust/object-cache-srv/tests/prefetch_tests.rs` (bodies → NDJSON; both the direct
   handler calls and the served-`Router` test at line 602) and add cases listed under Testing.
5. Docs: `mkdocs/docs/admin/object-cache.md` (`POST /prefetch` body section, lines ~105-118 —
   replace the "bounded only by the server's default 2 MiB request-body limit" paragraph with
   the NDJSON/per-line-cap story), `rust/object-cache-srv/README.md` endpoints table and the
   `/prefetch` prose paragraph (lines 33-46, which currently asserts the "default 2 MiB
   request-body limit" — rewrite to describe NDJSON and the per-line cap).

## Files to Modify

- `rust/object-cache/src/prefetch.rs`
- `rust/object-cache/src/client.rs`
- `rust/object-cache-srv/src/handlers.rs`
- `rust/object-cache-srv/tests/prefetch_tests.rs`
- `mkdocs/docs/admin/object-cache.md`
- `rust/object-cache-srv/README.md`

## Trade-offs

- **NDJSON vs raising/keeping the JSON-array body limit:** any whole-body format forces a
  ceiling on batch size; NDJSON shrinks the bounded unit to one item, whose cap (1 MiB) has an
  obvious justification. Cost: a wire-format change — paid now, while the endpoint has one
  in-repo consumer.
- **Malformed NDJSON line → `rejected` + continue (vs 400-abort):** newline framing means a bad
  line can't desynchronize subsequent lines, and reject-and-continue matches how per-item
  validation failures already behave. A systematically-broken client shows up in the `rejected`
  count either way.
- **Client request body stays buffered:** the caller already holds the full `Vec<PrefetchItem>`;
  streaming the upload would save nothing.

## Documentation

- `mkdocs/docs/admin/object-cache.md`: `/prefetch` body format + per-line cap.
- `rust/object-cache-srv/README.md`: endpoints table and the `/prefetch` prose paragraph
  (lines 33-46), which currently claims the "default 2 MiB request-body limit" applies.

## Testing Strategy

`object-cache-srv/tests/prefetch_tests.rs`:
- Existing suite converted to NDJSON bodies (mixed valid/invalid items, queue-full `dropped`,
  closed-queue 503, served-`Router` end-to-end test).
- New: blank lines skipped; final line without trailing `\n` processed; malformed line counted
  `rejected` while later lines still enqueue; line exceeding `MAX_PREFETCH_LINE_BYTES` → 400
  (and earlier items still enqueued); old-format `{"keys":[...]}` body → 202 with `rejected=1`;
  body larger than 2 MiB total (many small items) accepted through the served `Router`, confirming
  no whole-body size ceiling remains on `/prefetch`.
- Client round-trip: `CacheClientStore::prefetch` against the served router returns correct
  counts.
- Full suite: `cargo test -p micromegas-object-cache -p micromegas-object-cache-srv`, then
  `python3 ../build/rust_ci.py` before the PR.

## Open Questions

None — decisions above (malformed-line policy, buffered client body, 1 MiB line cap) are argued
in Trade-offs.
