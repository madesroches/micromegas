# Fix `object_cache_get_bytes_served` Structural Zero Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1279

## Overview

`object_cache_get_bytes_served` records a **true, structural zero** on the live `GET /obj/{key}`
path despite hundreds of thousands of successful `206` responses. The metric is emitted from the
`on_complete` callback of `count_bytes_served` (`rust/object-cache-srv/src/handlers.rs:390`), which
only fires once the wrapped body stream drains to a terminal `None`. The `GET` response is framed
with an explicit `Content-Length` header, and a `Content-Length`-framed HTTP body is considered
complete by the server transport (hyper) once the declared byte count has been written — it never
polls the underlying stream one further time for the terminal `None`. So `on_complete` never runs
under real HTTP serving, and the metric never increments.

This plan (1) adds an integration test that drives the handler through *actual* HTTP serving to
confirm the hypothesis, then (2) changes `count_bytes_served` to fire `on_complete` as soon as the
full expected payload has been produced — **before** yielding the final chunk — rather than relying
on a trailing `None` poll that the transport may never perform.

## Current State

### The counting wrapper — `rust/object-cache-srv/src/handlers.rs:84`

```rust
fn count_bytes_served<F>(
    mut inner: BoxStream<'static, Result<Bytes, anyhow::Error>>,
    on_complete: F,
) -> BoxStream<'static, Result<Bytes, anyhow::Error>>
where F: FnOnce(u64) + Send + 'static,
{
    gen_stream! {
        let mut total = 0u64;
        let mut on_complete = Some(on_complete);
        while let Some(item) = inner.next().await {
            let is_err = item.is_err();
            if let Ok(chunk) = &item { total += chunk.len() as u64; }
            yield item;                 // <-- suspends here
            if is_err { return; }
        }
        if let Some(f) = on_complete.take() { f(total); }   // <-- only runs on terminal None
    }.boxed()
}
```

The callback fires **only** after the `while` loop exits, i.e. after `inner.next().await` returns
`None`. Because `async-stream`'s `gen_stream!` resumes execution only when polled again, *any* code
after a `yield` — including the terminal-`None` handling — runs only if the consumer polls the
stream one more time after the last data chunk.

### The two call sites

- **`GET /obj/{key}`** (`handlers.rs:390`): builds a `206 Partial Content` with an explicit
  `Content-Length: requested_bytes` header (`handlers.rs:399-413`), where
  `requested_bytes = byte_range.end - byte_range.start` (`handlers.rs:321`) is exactly the number of
  bytes the wrapped stream yields. Emits `object_cache_get_bytes_served`.
- **`POST /ranges/{key}`** (`handlers.rs:600`): builds a `200 OK` with **no** `Content-Length`
  header (`handlers.rs:605-609`), so the transport uses chunked transfer-encoding. Emits
  `object_cache_ranges_bytes_served`.

### Why the asymmetry matches the observed evidence

Chunked transfer-encoding requires a terminating zero-length chunk, so the transport **must** poll
the body until it yields `None` — the ranges path therefore reaches `on_complete` and
`object_cache_ranges_bytes_served` works. A `Content-Length`-framed body does not need a terminator;
once `requested_bytes` have been written the transport stops polling, the `while` loop's final
`inner.next().await` never runs, and `object_cache_get_bytes_served` never fires. This is exactly
the issue's observation: the `GET` metric is a structural zero while every sibling counter on the
same path (`object_cache_get_requests`, `object_cache_ttfb_ms`) records hundreds of thousands of
`206` samples.

### Test coverage today

`object-cache-srv/tests/telemetry_tests.rs` calls the handlers **directly** and never polls the
returned response body, so `count_bytes_served` is never even entered. `memory_budget_tests.rs`
polls bodies via `axum::body::to_bytes` (an in-process drain that *does* reach the terminal `None`),
which is precisely the "test harness that drains the stream directly in-process" the issue calls
out — it masks the bug. No test drives the `GET` path through real HTTP serving and asserts on the
bytes-served metric.

## Design

### Fix: fire `on_complete` when the expected payload is complete, before the final yield

Pass the known total payload size into `count_bytes_served` and fire `on_complete` as soon as the
running total reaches it — **before** yielding the chunk that completes it — so the callback runs
regardless of whether the transport ever polls for a terminal `None`. Keep the terminal-`None` path
as a fallback for streams with no known length.

```rust
/// Wrap `inner` so that `on_complete` is called exactly once with the total
/// bytes yielded, as soon as the payload is fully produced. When
/// `expected_total` is known, the callback fires immediately BEFORE yielding
/// the chunk that completes it — a `Content-Length`-framed HTTP body is
/// considered complete by the transport once the declared byte count is
/// written and is never polled again for a terminal `None`, so firing after
/// the final `yield` (or on the terminal `None`) would never run in practice.
/// A mid-stream `Err`, or a stream that ends before reaching `expected_total`,
/// skips the callback — preserving the accepted under-reporting on truncation.
/// When `expected_total` is `None` (length genuinely unknown up front), there
/// is no "before completion" point to detect, so the callback instead fires
/// on the terminal `None`, with whichever total was accumulated by then.
fn count_bytes_served<F>(
    mut inner: BoxStream<'static, Result<Bytes, anyhow::Error>>,
    expected_total: Option<u64>,
    on_complete: F,
) -> BoxStream<'static, Result<Bytes, anyhow::Error>>
where F: FnOnce(u64) + Send + 'static,
{
    gen_stream! {
        let mut total = 0u64;
        let mut on_complete = Some(on_complete);
        while let Some(item) = inner.next().await {
            match &item {
                Ok(chunk) => {
                    total += chunk.len() as u64;
                    // Fire BEFORE the final yield: the transport may never
                    // poll us again once Content-Length is satisfied.
                    if let Some(expected) = expected_total
                        && total >= expected
                        && let Some(f) = on_complete.take()
                    {
                        f(total);
                    }
                    yield item;
                }
                Err(_) => {
                    yield item;
                    return;             // mid-stream error: skip the callback
                }
            }
        }
        // Fallback for streams with no known expected length: fire on the
        // terminal `None` with whatever total was accumulated. When
        // `expected_total` is `Some`, a stream that ends early (without an
        // error) must NOT fire here — that's the accepted under-reporting
        // case documented above, not a second chance to fire.
        if expected_total.is_none()
            && let Some(f) = on_complete.take()
        {
            f(total);
        }
    }.boxed()
}
```

`on_complete.take()` guarantees the callback fires **exactly once** even if both branches are
reached (they cannot both fire for a single response, but the `Option` makes that structural rather
than incidental).

### Why fire *before* the yield, not after

`async-stream`'s generator only advances past a `yield` when polled again. If the transport stops
polling after the last data chunk (the `Content-Length` case), no code placed *after* that
`yield` — including any post-yield "did we finish?" check — ever runs. Detecting completion from the
chunk's own bytes *before* handing it to the transport is the only placement that is robust to the
transport not polling again. This is the crux of the fix.

### Call-site changes

- **GET** (`handlers.rs:390`): pass `Some(requested_bytes)`. This equals the `Content-Length` header
  and the exact byte count the stream yields, so the callback fires precisely when the last chunk is
  produced.
- **POST ranges** (`handlers.rs:600`): pass `Some(framed_response_bytes)`, reusing the value already
  computed at `handlers.rs:525` as `total_requested.saturating_add(8 * req.ranges.len() as u64)`
  (each range is preceded by an 8-byte little-endian length prefix — see `frame_ranges_stream`,
  `handlers.rs:116`). This is numerically the framed total (`total_requested` is the sum of the
  requested range lengths), is still in scope and unmoved at the `count_bytes_served` call site, and
  its `saturating_add` guards overflow. The ranges path works today via the terminal-`None` fallback
  (chunked encoding polls to `None`), but passing the known total makes both paths robust to the same
  failure mode and keeps the two call sites symmetric.

### Edge cases (unchanged semantics)

- **Mid-stream origin error**: the `Err` arm yields the error then `return`s without firing — the
  under-report the doc comment already documents, preserved.
- **Consumer disconnect before the final chunk**: the running total never reaches `expected_total`
  and the terminal `None` never arrives, so the callback is skipped — matches the accepted
  early-disconnect under-reporting. (A disconnect *exactly at* the final chunk would now count the
  response as served, since we fire just before handing off the last chunk; this is a negligible,
  one-response-per-disconnect over-count on the produced-but-maybe-undelivered final chunk, and is
  the correct trade for making the whole-path metric usable.)
- **Zero-length responses** (`file_size == 0`, or open-ended range at EOF): short-circuited earlier
  in `get_range_handler` (`handlers.rs:271`, `:306`) with `Body::empty()` and never reach
  `count_bytes_served`, so they are unaffected.

## Implementation Steps

### Phase 1 — Confirm the hypothesis with a failing integration test

1. In `object-cache-srv/tests/telemetry_tests.rs` (or a small new sibling module), add a test that
   serves the `GET` handler over a **real HTTP** connection, following the established pattern in
   `memory_budget_tests.rs:527-554` / `:578-587`:
   - `init_in_memory_tracing()` for the in-memory sink; mark the test `#[serial]` (the sink is
     process-global, like the other telemetry tests).
   - Keep the default `#[tokio::test]` current-thread runtime, matching the existing tests in this
     file — this is not required for correctness, since the metrics stream is a single
     process-global `Mutex<MetricsStream>` on the global `Dispatch` (`rust/tracing/src/dispatch.rs`):
     `int_metric` and `flush_metrics_buffer` both lock that same global stream regardless of which
     thread the handler runs on, so thread affinity doesn't affect whether the sample is captured.
     Determinism instead comes from the fix itself: `on_complete` (and the metric record) fires
     before the final chunk is handed to the transport, so by the time the client has fully read the
     response body the sample is guaranteed to already be recorded. Do **not** add
     `flavor = "multi_thread"` — there's no need to, and it would only add noise relative to the
     existing single-threaded tests.
   - Bind an ephemeral `TcpListener`, `tokio::spawn(axum::serve(...))` with a router mounting
     `get_range_handler`, and drive a real ranged `GET` through `CacheClientStore` (as in the
     existing tests) or a raw `reqwest` client, fully reading the response body.
   - After the client has read the full body, call
     `micromegas_tracing::dispatch::flush_metrics_buffer()` and assert `object_cache_get_bytes_served`
     recorded **≥ 1** sample whose value equals the requested byte count.
2. Add a helper mirroring `tagged_status_prefix_pairs` that collects `IntegerMetricEvent` values for
   an untagged integer metric by name (`object_cache_get_bytes_served` carries no `status`/`prefix`
   tags, so the existing tagged helper does not apply). Sum/collect the samples for the assertion.
3. Run the test and confirm it **fails** against the current code (zero samples) — this is the
   reproduction the issue asks for.

### Phase 2 — Apply the fix

4. Change `count_bytes_served`'s signature to take `expected_total: Option<u64>` and restructure the
   generator to fire `on_complete` before the completing chunk's `yield`, per the Design section.
   Update the doc comment (`handlers.rs:84`) to describe the new "fires when the payload is fully
   produced" semantics and why firing must precede the final `yield`.
5. Update the GET call site (`handlers.rs:390`) to pass `Some(requested_bytes)`.
6. Update the ranges call site (`handlers.rs:600`) to pass `Some(framed_response_bytes)`, the value
   already computed at `handlers.rs:525` — no recomputation or move-ordering change needed.
7. Confirm the reproduction test from Phase 1 now **passes**.

### Phase 3 — Guard the ranges path and finalize

8. Add (or extend) a real-HTTP-serving test asserting `object_cache_ranges_bytes_served` still fires
   with the correct total, so the symmetric change is covered and can't silently regress.
9. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and the full
   `object-cache-srv` test suite (`cargo test -p micromegas-object-cache-srv`), then
   `python3 build/rust_ci.py`.

## Files to Modify

- `rust/object-cache-srv/src/handlers.rs` — `count_bytes_served` signature + generator logic + doc
  comment; both call sites (`:390`, `:600`).
- `rust/object-cache-srv/tests/telemetry_tests.rs` — new real-HTTP-serving tests for
  `object_cache_get_bytes_served` and `object_cache_ranges_bytes_served`, plus an untagged-integer
  metric collection helper.

## Trade-offs

- **Chosen: fire on reaching a known expected total, before the final yield.** Preserves the "one
  sample = one full response's byte count" semantics, is exact on the success path (no under/over
  report), keeps both call sites symmetric, and is robust to the transport not polling a terminal
  `None`. Requires threading a known length into the wrapper — trivial, since both call sites already
  know it.
- **Rejected: drop the `Content-Length` header on GET so it uses chunked encoding** (which would
  poll to `None` and fix the metric as a side effect). This degrades a genuinely useful HTTP
  affordance — clients lose up-front size/progress information on `206` responses, and the metric
  fix would be an incidental consequence of a client-facing behavior change. Worse fix.
- **Rejected: emit a metric sample per chunk (incremental counting).** Changes the metric's
  semantics from one-sample-per-response to many-small-samples, multiplies sample volume, turns the
  single per-response `debug!` log into per-chunk spam, and diverges from the ranges path. Sums stay
  correct but everything else about the signal gets noisier for no benefit.

## Documentation

`mkdocs/docs/admin/object-cache.md:212` documents `object_cache_get_bytes_served` /
`object_cache_ranges_bytes_served` as "Bytes served to clients over the wire." That description
becomes accurate again once the fix lands. Extend the row with a one-line note that the metric fires
once per fully-produced response and excludes responses cut short by a mid-stream error or an early
consumer disconnect — this sets correct expectations for anyone building throughput dashboards on it
and documents why the metric can slightly under-count relative to raw egress.

## Testing Strategy

- **Reproduction (must fail before, pass after)**: real-HTTP `GET` served via `axum::serve` on an
  ephemeral port, driven by a real client, asserting `object_cache_get_bytes_served` records the
  requested byte count. Default current-thread runtime (matching existing tests; the metrics sink is
  a single process-global stream, so thread affinity doesn't affect capture) + `#[serial]`, since
  that global sink is shared across all tests and concurrent tests would otherwise observe each
  other's samples. Determinism comes from the fix firing `on_complete` before the final chunk is
  handed off, not from thread affinity.
- **Regression guard for ranges**: analogous real-HTTP `POST /ranges` test asserting
  `object_cache_ranges_bytes_served` still fires with the correct framed total.
- **Existing suites**: `memory_budget_tests.rs` (which drains bodies in-process) must still pass —
  the in-process drain reaches the terminal-`None` fallback, and the byte-correctness assertions
  there are unaffected by the callback timing change.
- Full `cargo clippy --workspace -- -D warnings` and `python3 build/rust_ci.py`.

## Open Questions

None — both prior questions are resolved:
- **Ranges call site**: pass `Some(framed_response_bytes)`, reusing the existing value (robust and
  symmetric with the GET path).
- **Doc note**: add the "fires once per fully-produced response; excludes mid-stream errors and
  early disconnects" clarification to `object-cache.md:212`.
