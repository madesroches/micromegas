# Object-Cache Client Circuit Breaker Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1360

## The invariant

> **No cache failure mode may fail or corrupt real work.**

The object cache is an optimization. Every way it can misbehave — slow, hung, 5xx-ing, hard down,
truncating its own framing — must degrade to a direct object-store read that the caller cannot
distinguish from a normal one. The only errors the client may surface are the direct store's own.

Everything below is downstream of that sentence. It decides which timeout goes where (anywhere the
cache loses, abandon it — there is no position that must be waited out), it makes the circuit breaker
safe to be wrong (a bypass is never worse than a slow cache), and it is the reason this plan starts
with a correctness fix rather than with timeouts.

The second goal, from issue #1360, is that the optimization must not become a stability problem:
`CacheClientStore` already falls back on every read path, but it pays the full 15s budget first, so an
unresponsive cache parks every concurrent read for 15s and exhausts the query process long before any
request reaches its fallback.

## Overview

Three changes, in dependency order:

1. **Close the one hole in the invariant.** `full_stream_with_fallback` (`client.rs:302-348`) silently
   truncates a `GET /obj` body that fails after the first chunk. Fixed by resuming the remainder from
   the direct store at the byte offset already delivered. See "The hole: silent truncation past the
   first chunk".
2. **A much shorter detection budget** — 50ms connect, 500ms `abandon_timeout` on every request phase.
   With the hole closed, every phase is recoverable, so one rule covers all of them: abandon as soon as
   the cache has lost the race against a direct read. See "The rule: abandon everywhere".
3. **A circuit breaker** gating the whole client: after 5 consecutive unresponsive requests, reads and
   prefetches skip the cache entirely (no connection, no timeout cost) for a fixed 3s cooldown, with one
   probe request admitted per cooldown to detect recovery.

## Current State

### Timeouts

`client.rs:20-25` sets the whole client's budget once, at construction:

```rust
const CACHE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const CACHE_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
...
let http = Client::builder()
    .connect_timeout(CACHE_CONNECT_TIMEOUT)
    .timeout(CACHE_REQUEST_TIMEOUT)
    .build()
```

`reqwest`'s `ClientBuilder::timeout` is a **total deadline**: "applied from when the request starts
connecting until the response body has finished" (reqwest 0.12.28,
`async_impl/client.rs:1434-1443`). It therefore covers body streaming, not just the handshake — which
means today's 15s is also a *cumulative* bound: an ~1 GiB body must average roughly 70 MB/s to finish
inside the 15s deadline. That arithmetic models the caller that can actually
issue a request that large: the L1-disabled path, where `CacheClientStore` is used directly and a single
`GET /obj` can span an unbounded range. It is *not* the dominant, L1-fronted caller — in query processes
`l1_wrap` (on by default, `MICROMEGAS_OBJECT_CACHE_L1_MB=200`) coalesces every miss into a run of at most
`DEFAULT_MAX_COALESCED_GET_BYTES` (8 MiB, `l1_store.rs:76-110`), so every `/obj` request that caller
issues is orders of magnitude short of the size this cliff requires. This plan keeps `total_timeout` at
15s, unchanged, so the cliff is not removed for the caller it actually threatens — a large enough
L1-disabled (or `analytics/src/payload.rs:26` block-payload) read still hits it. What Phase 0 changes is
the *consequence*: hitting it now surfaces as a stream error that resumes the remainder from `direct`
(see "The fix: resume from the delivered offset"), not a silent short read. The tighter
`abandon_timeout`/`stall_timeout` bounds below make 15s a rare backstop for a healthy cache, but on that
non-L1-fronted path it stays the binding limit for a large, slowly-but-healthily streaming read — and
hitting it still reports `record_unresponsive` regardless (see "Abandon vs. unresponsive").

### Fallback paths

Every cache-path failure already falls back, with the same three-line bookkeeping
(`range_cache_client_fallback` counter, direct read, `range_cache_client_direct_ms` timing) repeated at
**seven** sites — `client.rs:325`, `498`, `544`, `588`, `600`, `641`, `656`:

- `get_opts` head-only path (`client.rs:486-509`)
- `get_opts` main path — full / bounded / offset / suffix (`client.rs:512-555`)
- `get_ranges`' four arms: the `send()` error, non-2xx, transport-error, and truncated-framing cases
  (`client.rs:586-611`, `640-666`)
- `full_stream_with_fallback`, for a stream error *before the first chunk* (`client.rs:309-348`)

`prefetch` (`client.rs:211-244`) has no fallback and needs none: there is nothing to fall back to, and
nothing depends on it succeeding. It counts `range_cache_client_prefetch_error` and returns `Err`, which
callers treat as "the warm didn't happen" (`rust/ingestion/src/data_lake_connection.rs:70-79`). A failed
prefetch does not fail real work, so it satisfies the invariant as it stands.

### The hole: silent truncation past the first chunk

`full_stream_with_fallback` has two arms (`client.rs:315-347`). A stream error *before* the first chunk
falls back to a full direct read. A stream error *after* it has no arm at all — the `while let` at
`client.rs:320-322` yields items until the stream ends, and an `Err` item ends the generator. The
function's own doc comment states the reasoning (`client.rs:302-308`): retrying from zero would re-emit
an already-delivered prefix, so it "simply ends the stream."

**That is not a clean failure.** In `object_store` 0.13.2, `GetResult::bytes()` (`lib.rs:1656-1672`)
delegates to `collect_bytes(s, Some(len))`, and `collect_bytes` (`util.rs:52-75`) uses that length
**only** as `Vec::with_capacity(size_hint)`. There is no length validation anywhere on the path. A
stream that ends early therefore returns `Ok(short_bytes)`, and what that costs depends on which caller
is on the other end:

- **On the dominant, L1-fronted path** — query processes' parquet/static-table reads, which go through
  `l1_wrap` (`lakehouse_context.rs:75`/`:93`, `static_tables_configurator.rs:76`) → `RangeCache` →
  `origin.get_range` — the short read lands back inside `RangeCache::fetch_blocks`, not the query.
  `fetch.rs:386-400` compares the delivered length against the requested run span, emits
  `range_cache_origin_run_len_mismatch` (documented as a "should be ~0" signal), logs `warn!`, and
  **fails the fetch** instead of under-yielding. So this path is not metrics-invisible — but it is
  *misattributed*: the metric reads as "origin object changed size," not "cache truncated its response,"
  and `L1CacheStore::fallback_get_opts` then retries through the same `CacheClientStore`, so a repeat
  truncation can still reach the consumer short rather than surfacing as an honest error.
- **On the non-L1-fronted callers** — L1 disabled (`MICROMEGAS_OBJECT_CACHE_L1_MB=0`), or the
  `blobs/...` block-payload path via `BlobStorage::read_blob` — there is no intermediate length check at
  all, so the caller gets a plain success with fewer bytes than it asked for and nothing in the metrics
  distinguishes it from a healthy read. For a parquet footer or data page this surfaces as a misparse
  attributed to the *data*; for a block payload it surfaces as a CBOR decode error instead.

So the client can currently corrupt or misattribute a read whenever the cache's `/obj` body stalls or
drops mid-stream, and today's 15s total deadline is what triggers it. This violates the invariant, is
independent of the circuit breaker, and is fixed first (Phase 0). It is arguably worth its own issue; the
plan keeps it here because the timeout work below is what makes it fire more often, and because the fix
reshapes the same function the breaker has to report from.

#### The fix: resume from the delivered offset

The "re-emitting a delivered prefix is unsound" objection applies only to restarting from **zero**. The
helper knows how many bytes it has yielded, and the resolved absolute byte range is already in hand — it
is the `range` field of the `GetResult` built at `client.rs:294-299`. So on *any* stream error it can
read the remainder from `direct`:

```
resume_start = resolved_range.start + bytes_yielded
remainder    = resume_start .. resolved_range.end
```

and keep yielding. The consumer observes one continuous byte stream and never learns the cache was
involved.

**This collapses the function rather than growing it.** At `bytes_yielded == 0` the remainder *is* the
whole requested range, so the pre-first-chunk fallback is the degenerate case of the same formula. The
two-arm match becomes one path: yield chunks, count bytes, and on `Err` resume the remainder from
`direct`. The `Some(Err(e))` arm at `client.rs:324-346` disappears into it.

Details that matter for correctness:

- **An empty (or over-delivered) remainder ends the stream cleanly, not with a read.** If `resume_start
  >= resolved_range.end` (a stream error arrives after every requested byte has already been yielded, or
  after more bytes than requested), the operation is done — end the stream successfully and never call
  `direct.get_opts` with `GetRange::Bounded(x..x)` or an inverted range. That call is a hard error in
  `object_store` 0.13.2 (`GetRange::is_valid`, `util.rs:225-239`, rejects `Bounded(r)` when `r.end <=
  r.start`), and both cases are reachable: `get_range_stream` accepts a 206 on `Content-Range` alone
  (`client.rs:110-130`) with no `Content-Length` cross-check, so a chunked body that delivers every byte
  and then stalls before its terminating chunk trips `read_timeout` with `bytes_yielded == range length`
  (the `==` case), and a cache that over-delivers relative to its own declared `Content-Range` can push
  `bytes_yielded` past it (the `>` case). Without this guard a cache failure at or past the last byte
  would either fail a read that had already fully succeeded or construct an inverted `Bounded` range —
  precisely what the invariant forbids.
- **Use the resolved range, not the original `GetOptions::range`.** Re-passing a `Suffix` or `Offset`
  range would re-resolve against the object's current size; the resolved `Bounded` remainder is
  unambiguous. Clone the options and replace `range` with `GetRange::Bounded(remainder)`.
- **Preconditions are already excluded.** `get_opts` short-circuits preconditioned requests to `direct`
  before ever touching the cache, so the options reaching this helper carry none, and there is no
  if-match/if-modified interaction to reason about.
- **A full read resumes as `Bounded(0..size)`** rather than as an unranged GET. Byte-identical payload;
  the origin answers 206 instead of 200.
- **If the resumed direct read fails, yield its error.** That is a direct-store failure, which the
  invariant permits — the invariant constrains *cache* failure modes.
- After a resume the read is served entirely by `direct`, so there is no second cache stall to handle.

With this in place there is **no position in the client where giving up costs the caller anything but
one direct read.**

### Where the origin work actually lands

Earlier drafts split the budget on "which phases do origin work," on the premise that `head_size` and
the `/ranges` header phase do none. That premise is false: **every** read endpoint blocks on the origin
in its header phase.

- **`GET /obj`** commits before streaming: `get_range_handler_inner` resolves `size()`, waits for a
  memory-budget permit (`handlers.rs:349-359`), then awaits the first chunk — the per-block origin GET —
  before building the response (`handlers.rs:282-395`, and `389-395` for the commit-before-stream
  await). Time-to-headers therefore *contains* an origin GET, and is what the client already measures as
  `range_cache_client_roundtrip_ms`.
- **`HEAD /obj`** does too. `head_handler_inner` calls `state.cache.size(&key)`
  (`handlers.rs:215-238`), and `RangeCache::size` (`rust/object-cache/src/range_cache/mod.rs:217-265`)
  returns from the backend only on a `meta:{ns}:{key}` hit; otherwise it goes through
  `scheduler.own_or_join` (which can block on another in-flight HEAD for the same key) and issues
  `origin.head(&path)` — a real S3/GCS round trip, instrumented as `range_cache_origin_head` /
  `range_cache_origin_head_latency`. That is every key after a cache restart. It leaks into `/obj`
  too: `get_opts`'s Suffix path (`client.rs:531`) and `get_range_stream`'s no-`Content-Range` path
  (`client.rs:137`) both call `head_size`.
- **`POST /ranges`** does the same origin HEAD in its header phase — `stream_ranges`'s first act is
  `self.size(key)` (`range_cache/mod.rs:347-359`), as the handler's own comment notes
  (`handlers.rs:528-533`: "`stream_ranges` always does a `size()` lookup up front").
- **`POST /prefetch`** is the only endpoint that never blocks on the origin (`prefetch_handler`,
  `handlers.rs:710-779`, explicitly "never blocks on an origin fetch and never acquires a
  `mem_permit`") — it parses lines and `try_send`s them to a queue. Its time-to-headers is a function
  of payload size, not cache health. In production that payload is one item:
  `DataLakeConnection::warm_object` (`rust/ingestion/src/data_lake_connection.rs:57-80`) calls
  `prefetch.prefetch(vec![item])` from a detached `spawn_with_context` task and logs failures at
  `debug`. At batch size 1 the header phase *is* a cache-health signal, so `prefetch` takes the same
  budget as everything else; the only place batch size matters is one oversized existing test (see
  Testing Strategy → Regression).

`POST /ranges` has one further property that matters for constraint (a) below:
`post_ranges_handler_inner` awaits `framed.next()` (`handlers.rs:~598`), but `frame_ranges_stream`
(`handlers.rs:147-169`) yields the first range's 8-byte little-endian length prefix before it ever
polls the underlying block stream, and `stream_ranges_inner` (`range_cache/mod.rs:381-414`) is a lazy
`try_stream!` that fetches nothing until first polled. So the *block* fetches on `/ranges` all happen
after the response headers have been committed — a header-only bound there could never detect
"answered headers, then stuck on the origin." The `/ranges` **body** must be bounded too.

### Single instance per process

`make_cache` (`rust/ingestion/src/data_lake_connection.rs:86-108`) is the only construction site: one
`Arc<CacheClientStore>` per process, shared as both `Arc<dyn ObjectStore>` and
`Arc<dyn ObjectPrefetch>`. Breaker state therefore lives on the struct — no statics, no per-endpoint
sharding.

## Design

### The rule: abandon everywhere

Issue #1360 states the principle the budget hangs on, verbatim:

> Grounded in production measurements of origin-fetch latency (`range_cache_origin_get_ms`): p50
> ~36ms, p90 ~152ms, p99 ~311ms, max ~575ms over a 5-minute sample under load. A slow cache response
> shouldn't be treated as a "failure" — past this budget it's simply no longer an optimization, since
> going direct is likely comparable or faster.

The ~575ms tail is the cost of the **direct path** — the thing the cache is racing. The budget is
calibrated **at** that distribution, not above it. Once the cache has taken longer than a direct read
would have, the cache has stopped being an optimization and abandoning it is the *correct* outcome, not
a false abort. Raising the budget above the origin tail defeats the purpose: it makes the client wait
out a cache that has already lost the race.

Because Phase 0 closes the truncation hole, this applies **uniformly**:

> **Abandon at the direct-path cost, everywhere. There is no phase that has to be waited out.**

Earlier drafts of this plan carried an exception — the `/obj` body past the first chunk, which had to
be given a larger "genuine liveness" bound because aborting it was unrecoverable. That exception is
gone: aborting there now costs one ranged direct read, exactly like every other phase.

| Phase | Budget | On expiry |
|---|---|---|
| Connect (all endpoints) | 50ms `connect_timeout` | reqwest error → unresponsive → fallback |
| All five `send()` sites (`head_size`, `prefetch`, `get_full_stream`, `get_range_stream`, `get_ranges`): request → response headers | 500ms `abandon_timeout` (`tokio::time::timeout` around `send()`) | abandoned → fallback |
| `get_ranges` body reassembly (`read_framed_ranges`/`pull_exact`) | 3s `stall_timeout` *per chunk* | `RangesReadError::Stalled` → fallback |
| `GET /obj` response body | `read_timeout` = `stall_timeout` + `abandon_timeout` (3.5s), per response frame via `ClientBuilder::read_timeout` — deliberately looser than the `/ranges` row so its explicit wrap always fires first, see "`ClientBuilder::read_timeout` on the one client" | stream error → **resume remainder from `direct`**; reports `record_unresponsive` |
| Whole request (all endpoints) | 15s `total_timeout` (`ClientBuilder::timeout`, unchanged) | For `/obj` body: same stream error → resume as the row above (reports `record_unresponsive`); elsewhere a rare backstop behind the tighter bounds above → fallback, also reporting `record_unresponsive` — see "Abandon vs. unresponsive" |

Every "on expiry" column entry is a degradation, never an error. That is the invariant, mechanically.

Two constraints the table has to satisfy explicitly:

- **(a) `/ranges` commits headers before any origin block fetch.** A header-only bound there could never
  detect "answered headers, then stuck on the origin," so the `/ranges` *body* carries its own
  per-chunk `stall_timeout` via `pull_exact` (`client.rs:419-443`) — the one place that path awaits
  a chunk — surfacing as a new `RangesReadError::Stalled` variant that `read_framed_ranges` propagates
  exactly as it already propagates `Transport`. `RangesReadError` and the `send_ranges` helper that
  surfaces it as `RangesSendError` are made `pub` (step 9) precisely so this classification is directly
  assertable from the cross-crate integration test, without inspecting logs or metrics. For this
  explicit wrap to be the one that actually fires on a real response body, it has to be strictly tighter
  than the client-level `read_timeout` racing it underneath — see "`ClientBuilder::read_timeout` on the
  one client" for why `read_timeout` is deliberately set looser, not equal.
- **(b) `/ranges` body size is unbounded.** Total bytes have no cap (`client.rs:613-624`; the server
  caps range *count* at `MAX_RANGES_PER_REQUEST` = 4096, not bytes), so the budget must be per chunk.
  A flat deadline would abort healthy large reads on cumulative size.

**What this buys.** A hard-down cache is detected per request in ~50ms (connect refused/timed out) or
~500ms (accepted but silent) instead of 15s, and after `failure_threshold` such requests the breaker
removes even that cost. Under concurrent load the trip happens roughly one detection budget in, since
the in-flight requests fail together.

#### `stall_timeout` (3s) is now a cost knob, not a correctness bound

The `/obj` body — and, per constraint (a), `/ranges`' per-chunk body too — keeps a looser bound than the
header phase, but the reason has changed completely. It is no longer protecting queries from an
unrecoverable abort — it is limiting **wasted origin traffic**, because a mid-stream abandon re-reads the
entire remainder of the object (or the ranges not yet delivered) from the origin.

**Which caller this bounds depends on whether L1 is in front.** In query processes the lakehouse/static
stores are wrapped by `l1_wrap` (on by default, `MICROMEGAS_OBJECT_CACHE_L1_MB=200`), whose `RangeCache`
coalesces every miss into a run of at most `DEFAULT_MAX_COALESCED_GET_BYTES` (8 MiB, `l1_store.rs:76-110`)
and fetches it as one `cache.origin.get_range(&path, byte_start..byte_end)` call
(`range_cache/fetch.rs:109`, `:321`). So the dominant caller's `GET /obj` request carries **at most one**
8 MiB demand window, and the server's commit-before-stream already awaits that window's origin GET before
writing headers (`handlers.rs:389-395`) — the whole body is already resolved before streaming starts. For
that caller there is no inter-frame origin gap to bound at all, and a mid-stream abandon costs at most the
8 MiB already in flight, not the multi-hundred-MiB remainder the arithmetic below assumes. The
multi-window exposure below instead models the caller that can issue a `/obj` request spanning more than
one 8 MiB window: the L1-disabled path (`MICROMEGAS_OBJECT_CACHE_L1_MB=0`), where `CacheClientStore` is
used directly and a single request can span an unbounded range. (The other non-L1 traffic —
`analytics/src/payload.rs:26` block-payload reads, a few MB each, and `HEAD /obj` — is too small for this
exposure to matter.) `POST /ranges` is likewise reached only from this non-L1-fronted position:
`L1CacheStore::get_ranges` serves from its own cache, and its misses also go out as `get_range`, not
`get_ranges` (`l1_store.rs:239-256`) — so `/ranges` only runs with L1 disabled or on an L1 cache error.

`read_timeout` is sampled once per response **frame**, and on the non-L1-fronted path a frame is large:

- One yielded chunk is one *demand window* — `DEMAND_WINDOW_BLOCKS` (8) × `DEFAULT_BLOCK_SIZE` (1 MiB)
  = **8 MiB** (`range_cache/mod.rs:25-33`, `stream_ranges_inner` at `mod.rs:401-413`).
- A window is fetched as one coalesced origin GET — 8 MiB is exactly
  `DEFAULT_MAX_COALESCED_GET_BYTES` (`mod.rs:42`).
- `stream_demand_windows` pipelines with `buffered(2)` (`mod.rs:435-446`), i.e. one window of lookahead.
  That hides the fetch only while the client drains slower than the origin delivers; a local query
  process drains faster, so in a large sequential read the gap is essentially exposed.

So, on a `/obj` request that spans multiple windows, **every inter-frame gap is a full coalesced origin
GET**, drawn from the distribution above — whose observed max (575ms in a 5-minute sample) already
exceeds 500ms. Frame count then multiplies the exposure. Taking P(gap > 500ms) ≈ 0.5% (between the 311ms
p99 and the 575ms max):

| Read size (non-L1-fronted `/obj`, or `/ranges`) | 8 MiB window boundaries | P(at least one gap > 500ms) |
|---|---|---|
| 64 MiB | 8 | ~4% |
| 256 MiB | 32 | ~15% |
| 1 GiB | 128 | ~47% |

(`read_timeout` itself resets per hyper response frame, not per window — client-side frames come from
socket reads and there are far more than 8 of them in a 64 MiB read. The gap that matters occurs once
per 8 MiB demand-window boundary, which is the quantity this table counts.)

At 500ms, a 1 GiB non-L1-fronted read would resume-from-`direct` about half the time, and the average
remainder is ~512 MiB — so the tail re-read costs far more than the abandon saves. At 3s the same
arithmetic is negligible (5.2x the observed max), and the bound reads as a throughput floor: 8 MiB per 3s
≈ 2.7 MB/s. A cache delivering less than that has stopped streaming.

This structure is not unique to `/obj`: `/ranges`' body is produced by the same `stream_ranges_inner`
windows and forwarded unchanged by `frame_ranges_stream` (`handlers.rs:147-169`), so `pull_exact`'s
per-chunk read faces the identical inter-frame gap distribution whenever `/ranges` is reached at all (see
above — only on the non-L1-fronted position). It therefore carries the same 3s `stall_timeout` bound, not
the 500ms `abandon_timeout` used for every phase's header check (see constraint (a)).

Note the asymmetry this rests on: `abandon_timeout` is sampled once per *request* and its worst case is
one direct read the client would have done anyway. A per-frame bound is sampled once per 8 MiB and its
worst case is re-reading everything not yet delivered. Same "abandon when the cache loses" rule; the
unit of work it is applied to is different, so the number is different. Because both outcomes are
degradations, getting this number somewhat wrong costs throughput, never correctness — which is the
whole point of doing Phase 0 first.

The converse merge — 3s everywhere — is wrong for the opposite reason: it makes hard-down detection 6x
slower on every header phase, which is what issue #1360 exists to fix.

**Honest caveat on 3s.** For the dominant, L1-fronted caller this section's arithmetic barely applies:
exposure is already near zero because the single window is resolved before headers, so 3s there is
essentially unused headroom, not a calibrated cost knob. Its calibration is really load-bearing only for
the non-L1-fronted path (L1 disabled, or an L1 cache error), and this plan has no production read-size
distribution for *that* path the way it does for `range_cache_origin_get_ms`. 3s is kept because it is
conservative relative to the observed origin-latency tail and because, per the invariant, getting this
number wrong costs only throughput — but if that path turns out to serve mostly small reads in practice, a
tighter value would waste less origin traffic without giving up meaningful detection speed. Worth
revisiting once `range_cache_client_stream_resumed` has production data (see "Metrics and logging").

#### `ClientBuilder::read_timeout` on the one client

The store keeps building exactly **one** `reqwest::Client`, now with
`read_timeout(stall_timeout + abandon_timeout)` alongside `connect_timeout` and the unchanged total
`timeout`. In reqwest 0.12.28, `read_timeout` is a per-frame timeout on the response body
(`async_impl/body.rs:287-340` — `ReadTimeoutBody` resets its sleep per frame) plus a non-resetting bound
on the header phase (`async_impl/client.rs:3053-3059`).

**Why not set it to `stall_timeout` exactly.** `/ranges` wraps `pull_exact`'s `stream.next()` in its own
`tokio::time::timeout(stall_timeout, ...)` (constraint (a)), racing the same underlying poll that
`ReadTimeoutBody` is timing at the client level. `ReadTimeoutBody` clears its sleep on every ready frame
and re-arms it lazily on the *next* poll, checking elapsed time before polling the inner body
(`async_impl/body.rs:336-360`) — and `pull_exact`'s `tokio::time::timeout` does the same (`Timeout::poll`
polls the inner future first). If both deadlines are set to the same `stall_timeout`, they land
microseconds apart and, in practice, in the same tick, so which one fires on a real stall is not
determined by the design: `RangesReadError::Stalled` might never surface, and the stall would report as
plain `Transport` instead — silently defeating the log-granularity reason for keeping `Stalled` at all.
Giving `read_timeout` a margin above `stall_timeout` makes `pull_exact`'s explicit wrap unambiguously the
first to fire. The margin is expressed in existing config rather than as a new knob: `abandon_timeout`
(500ms) is already the shortest meaningful gap this design reasons about, safely larger than any
scheduling jitter between the two timers, so `read_timeout` is `stall_timeout + abandon_timeout` — 3.5s —
with no dedicated constant of its own.

Set to 3.5s, `read_timeout` is:

- the **operative** bound in two places — the `/obj` response body, and `prefetch`'s `resp.json()` read
  (`client.rs:233`), which takes no explicit wrap of its own. `/obj`'s real per-frame bound is therefore
  3.5s, not the 3s `stall_timeout` used for the throughput-floor arithmetic in "`stall_timeout` (3s) is
  now a cost knob" — negligibly looser (8 MiB per 3.5s ≈ 2.3 MB/s), so that section's reasoning still
  holds;
- an inert **backstop** everywhere else, because every other phase already carries a strictly tighter
  500ms `tokio::time::timeout`.

An earlier draft rejected `read_timeout` on the grounds that it would require a dedicated `/ranges`
client and split the connection pools. That reason was wrong and is dropped. The real trade-offs are:

- **Error-classification granularity on `/obj`.** A body stall arrives as a generic `reqwest::Error`
  rather than a distinct "stalled" variant, so `/obj` stalls and `/obj` transport errors are
  indistinguishable in logs. Both are liveness signals, both resume from `direct`, and both report
  `record_unresponsive`, so nothing downstream changes — only diagnosis is coarser. `/ranges`' body has
  its own tighter `stall_timeout` (3s) wrap, deliberately below `read_timeout` (3.5s) for the margin
  reason above, so `read_timeout` never actually fires on `/ranges` — the explicit wrap always wins.
  `/ranges` keeps that explicit `Stalled` variant for the same reason an earlier draft gave a different
  justification for: not because `read_timeout` alone would be too loose to protect throughput (both
  numbers are within the same cost-knob ballpark), but because `send_ranges`'s typed `RangesSendError`
  (public, re-exported — see step 9) is directly assertable from the cross-crate integration test without
  inspecting logs, and a named variant keeps `/ranges`' four failure arms
  (non-2xx/`Transport`/`Stalled`/`Truncated`) distinguishable in logs the way a bare `reqwest::Error`
  cannot — and, with the margin in place, that variant is now reliably the one that actually surfaces on
  a real stall.
- **The request-upload phase is folded into the header bound.** `read_timeout`'s header-phase bound
  spans request upload as well as time-to-headers, but `prefetch` also goes through `send`'s
  `abandon_timeout` wrap around the same `req.send()` call, so in practice the upload is bounded at
  500ms, not 3.5s — `read_timeout`'s 3.5s is a looser backstop that only matters if `abandon_timeout`
  somehow didn't apply. Irrelevant in production (batches are one item) and only visible in one existing
  test (see Testing Strategy → Regression).

### Abandon vs. unresponsive

Two different facts arrive at the breaker, and the plan deliberately maps both onto the same input:

1. **Abandoned** — an `abandon_timeout` expiry. Means "the cache did not beat the direct path." It is
   *not* by itself evidence that the cache is broken: a cold cache after a restart loses that race
   legitimately. Emits `range_cache_client_abandoned`.
2. **Unresponsive** — a connect failure, a transport error, or a `stall_timeout` expiry (3s with no
   frame). Means "the cache is not answering at all." Emits `range_cache_client_unresponsive`.
3. **A `total_timeout` expiry** (15s cumulative, `ClientBuilder::timeout`) is folded into the same
   `record_unresponsive` report as case 2, even though it can be hit by a body that was streaming
   perfectly healthily but slowly (see "Current State → Timeouts"). It arrives as an ordinary
   `reqwest::Error`, indistinguishable at the error level from a stall, so there is no signal to classify
   it differently on. This is deliberate, not an oversight: by the time it fires the read has already
   taken 15s — roughly three orders of magnitude past the origin-fetch latencies (`range_cache_origin_get_ms`
   p50 ~36ms) the direct path is racing against — so treating it as anything but unresponsive would be
   arguing over which flavor of "lost the race badly" this is. And per the invariant, a wrong
   classification here costs only the optimization for one cooldown, never correctness.

All three call `CircuitBreaker::record_unresponsive`, because the question the breaker answers is not "is
the cache alive?" but "is routing through the cache still worth its cost?" — and by issue #1360's own
rule, a cache that has lost five consecutive races against the direct path is not, whatever the reason.
Bypassing it is then a latency *win* for those reads, not a degradation, and the probe schedule
re-tests continuously. The two counters stay separate so an operator can still tell a slow cache from a
dead one on a dashboard.

The invariant is what makes this fusion safe. If bypassing could fail work, conflating "slow" with
"dead" would be dangerous; because a bypass is just a direct read, the worst case of a wrong breaker
decision is losing the optimization for one cooldown.

**What does not feed the breaker as a failure:** a non-2xx status on a demand-read endpoint (`/obj`,
`HEAD`, `/ranges`), the two malformed-response arms that follow a full 2xx response — a missing
`Content-Length` in `get_full_stream` (`client.rs:169-171`) and a missing/unparseable `Content-Length` in
`head_size` (`client.rs:200-204`) — and `RangesReadError::Truncated`. All of these mean a full HTTP
response arrived cheaply, so per the "any HTTP response on a demand-read endpoint counts as responsive"
rule they report `record_responsive` — at the status check in each caller for `/obj`/`HEAD`
(`client.rs:106-108`, `166-168`, `197-199`), at the two malformed-header arms alongside it, and in
`send_ranges` for `/ranges` (already specified below), each before falling back to `direct`. `Truncated`
keeps its `warn!` as a protocol violation from our own cache, not a health signal; the two
malformed-header arms are the same kind of protocol violation from our own cache and are treated
identically. `prefetch`'s `resp.json::<PrefetchResponse>()` (`client.rs:233`) belongs to the same family
when it fails with a decode error (`reqwest::Error::is_decode()`): the response arrived as a full, cheap
2xx, and the body simply doesn't parse as a `PrefetchResponse` — a protocol violation from our own cache,
not evidence the demand path is unresponsive — so it reports nothing, exactly like the other two
malformed-response arms, rather than `record_unresponsive`. A `resp.json()` failure that is instead a
body/transport/timeout error (`is_body()`/`is_timeout()`) is a genuine liveness signal and still reports
`record_unresponsive` (see "One outcome per logical operation"). `prefetch`'s non-2xx status
(`client.rs:230-232`) is deliberately **not** covered by this rule: per "Prefetch does not close the
circuit" it reports nothing, on any admission. Folding it into the general rule would let a write-time
warm's non-2xx response hold a 503-ing cache's circuit closed — exactly the failure mode that section
exists to prevent.

**One outcome per logical operation.** A single read can touch the cache twice — `get_opts`'s Suffix arm
issues `head_size` and then `get_range_stream` (`client.rs:531`). Reporting `record_responsive` at both
time-to-headers would zero `consecutive` on every such request, so a failure in the later
`get_range_stream` phase could never accumulate to `failure_threshold` and the breaker could never trip
on a body stall at all. So:

- **`send()` reports failures only** (abandon / transport / connect). Those are terminal for the
  operation, so reporting them immediately is correct.
- **Success is reported once, by whoever completes the logical operation.** Never by an intermediate
  step.

| Entry point | Where the single success report happens |
|---|---|
| `get_opts` head-only path (`client.rs:486-509`) | Its own call site, immediately after `head_size` returns a successfully parsed size — `head_size`'s completion *is* the whole operation here. (`head_size`'s own two after-2xx failure arms — non-2xx and malformed-`Content-Length` — report `record_responsive` internally instead, terminal for every caller of `head_size`; see "Wiring into `CacheClientStore`" → `get_opts`.) |
| `get_opts` main path (full / bounded / offset / suffix) | `full_stream_with_fallback`, when the body stream ends without error **and no resume occurred** — not the intermediate `head_size`, and not `send()`'s headers. A resumed operation reports `record_unresponsive` only, from the resume path, and nothing else |
| `get_range_stream`'s no-`Content-Range` branch (`client.rs:132-143`) | Its own call site, immediately after `head_size` returns a successfully parsed size — no stream follows, so this is the whole operation. (`head_size`'s internal failure-arm reports are the same shared ones as above.) |
| `get_ranges` | `send_ranges`, once the framed body fully resolves (already specified below) |
| `prefetch` | see "Prefetch does not close the circuit" |

`prefetch`'s `resp.json()` (`client.rs:233`) is the tail of its own operation. A body/transport/timeout
error there (`reqwest::Error::is_body()`/`is_timeout()`) is a failure report; a decode error
(`reqwest::Error::is_decode()`) instead reports nothing, per "What does not feed the breaker as a
failure" — the same treatment as `get_full_stream`'s and `head_size`'s malformed-response arms, since a
`PrefetchResponse` that fails to parse still arrived as a full, cheap 2xx. Its success is governed by the
prefetch rule below.

### Prefetch does not close the circuit

`POST /prefetch` is the one endpoint that never touches the origin — it parses NDJSON and `try_send`s to
a queue (`handlers.rs:710-779`). Its success therefore says the accept loop is alive, which is *not* the
question the breaker answers ("is routing reads through the cache still worth its cost?").

If prefetch success reported `record_responsive`, every write-time warm — `warm_object` fires one per
written object from a detached task (`data_lake_connection.rs:57-80`) — would zero `consecutive`. A
write-active, read-light process could then keep the circuit closed while every demand read abandons.

So:

- **Prefetch never sees a `Probe` at all.** It calls `admit_bypass_only()` instead of `admit()` — a
  query that only ever returns `Allow`/`Bypass` and, critically, never re-arms `open_until` the way
  `admit_at`'s cooldown-elapsed arm does. Calling plain `admit()` here would let a `prefetch` burn the
  single per-cooldown probe slot, pushing every demand read's recovery out another full cooldown;
  `admit_bypass_only()` removes that possibility structurally rather than by convention. On `Bypass` it skips the cache and reports
  nothing; on `Allow` it uses the cache and still reports nothing on success. Only a demand read
  (`get_opts`/`get_ranges`) that itself receives `Probe` from `admit_at` and completes a cache request can
  report `record_responsive` and close the circuit.
- **Prefetch's non-2xx status (`client.rs:230-232`) reports nothing**, for the same reason as success: a
  full response having arrived cheaply is no more evidence the *demand* path is healthy than a 2xx one
  is, so it stays out of the general "any HTTP response counts as responsive" rule regardless of
  admission.
- **Prefetch failures still report** (`record_unresponsive` via abandon / transport / connect). A cache
  that cannot even accept a one-item POST is real evidence of unresponsiveness.

An earlier draft had a `Probe`-admitted prefetch report `record_responsive`, on the premise that "a
process that only ever prefetches can still recover its circuit." That process doesn't exist: the only
production caller of `prefetch` is `warm_object` (`rust/ingestion/src/data_lake_connection.rs:57-80`),
called from `rust/analytics/src/lakehouse/write_partition.rs:746` — i.e. from the analytics/maintenance
processes, which are also the heaviest demand-read callers; `telemetry-ingestion-srv` never calls
`warm_object` at all. Letting a `Probe`-admitted prefetch close the circuit in a write-active process
would let accept-loop liveness reopen the demand path on the fixed cooldown cadence regardless of whether
demand reads are actually recovering — reintroducing, on a ~3s cycle, a fraction of exactly the
parked-task exhaustion this plan exists to remove. Gating `prefetch` on `admit_bypass_only()` — which
never hands out a `Probe` and never re-arms `open_until` — removes that path entirely, rather than
merely discarding a `Probe` after receiving one: `admit()`'s cooldown-elapsed arm re-arms the window as
part of handing out the `Probe`, so calling `admit()` and then discarding the result would already have
burned the slot. As a side effect this also means `prefetch` never has to distinguish `Probe` from
`Allow` at all — it only ever needs "cache or don't."

Note this removes an accidental mitigation: prefetch traffic was implicitly holding the circuit closed
during cold periods. That does not argue for exempting `prefetch` from the admission gate, though.

**Cold-cache tripping.** The only production caller of `prefetch` is `warm_object`
(`rust/ingestion/src/data_lake_connection.rs:57-80`), called from
`rust/analytics/src/lakehouse/write_partition.rs:746` immediately after a partition file is written — so
prefetch traffic warms only *freshly written* objects. After a cache restart the cold working set is the
*existing* partitions, which write-time warming never touches. Exempting `prefetch` from the admission
gate would therefore warm nothing that went cold, and cannot deliver the rewarming benefit an exemption
would be proposed for. Demand-driven rewarming instead happens through the fixed 3s probe cadence (see
"Why the cooldown is fixed"), and a warm skipped while the circuit is open is already declared acceptable
by `warm_object`'s own doc comment ("a failed warm just means the first read is a cold miss") — it would
have failed against a down cache anyway. So `prefetch` stays gated like every other entry point, and
only ever bypasses or uses the cache silently — it never probes.

### The breaker

New module `rust/object-cache/src/circuit_breaker.rs`, deliberately free of any cache-specific naming
so it stays reusable (and so `imetric!`'s literal-name requirement doesn't leak into it): state
transitions are *returned* to the caller, which owns the metrics and logs.

```rust
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Consecutive unresponsive requests that trip the breaker. `0` disables it.
    pub failure_threshold: u32, // 5
    /// How long the circuit stays open before one probe is admitted. Fixed, not
    /// backed off — see "Why the cooldown is fixed". The client passes its
    /// `stall_timeout`, reusing it rather than adding a second knob; an
    /// occasional probe overlapping the next admitted request is harmless,
    /// since there is no doubling for it to corrupt.
    pub cooldown: Duration, // 3s (= CacheClientConfig::stall_timeout)
}

/// What a caller may do with the guarded resource right now.
#[derive(Debug, PartialEq)]
pub enum Admission {
    /// Closed: use it normally.
    Allow,
    /// Open, cooldown elapsed: this one request probes for recovery.
    Probe,
    /// Open: skip it entirely — no connection, no timeout cost.
    Bypass,
}

/// A state change worth reporting, returned so the caller emits its own
/// metrics/logs and the breaker stays domain-agnostic.
#[derive(Debug, PartialEq)]
#[must_use]
pub enum Transition {
    None,
    Opened { cooldown: Duration },
    Closed,
}

#[derive(Debug)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    /// Consecutive unresponsive requests while closed; any response resets it.
    consecutive: u32,
    /// `Some(t)` => open: bypass until `t`, then admit one probe.
    open_until: Option<Instant>,
}
```

Two fields, and that is the whole state. The earlier draft carried a mutable `cooldown` plus a
`backoff_applied` flag to keep an exponential doubling idempotent within an open window; both existed
solely to serve the doubling. See "Why the cooldown is fixed".

Public API — each method has an `_at(now: Instant)` form (the real logic) plus a wrapper that passes
`Instant::now()`, so the state machine is unit-testable with a synthetic clock and no sleeps:

```rust
impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self;
    pub fn admit(&self) -> Admission;
    pub fn admit_at(&self, now: Instant) -> Admission;
    /// Same read of state as `admit`, but never returns `Probe` and never
    /// mutates `open_until` — for a caller (`prefetch`) that must not be able
    /// to consume the single per-cooldown probe slot a demand read would
    /// otherwise receive.
    pub fn admit_bypass_only(&self) -> Admission;
    /// The resource completed an operation (any HTTP status counts — it's alive).
    pub fn record_responsive(&self) -> Transition;
    pub fn record_responsive_at(&self, now: Instant) -> Transition;
    /// The resource lost: an abandon-budget expiry, a stall, a connect
    /// failure, or a transport error (see "Abandon vs. unresponsive").
    pub fn record_unresponsive(&self) -> Transition;
    pub fn record_unresponsive_at(&self, now: Instant) -> Transition;
}
```

Logic:

```
admit_at(now):
    if failure_threshold == 0 { return Allow }          // breaker disabled
    match open_until:
        None                 => Allow
        Some(t) if now < t   => Bypass
        Some(_)              => {
            open_until = Some(now + cooldown);          // re-arm before probing
            Probe
        }

admit_bypass_only():
    if failure_threshold == 0 { return Allow }
    match open_until:
        None    => Allow
        Some(_) => Bypass    // never Probe; reads state only, mutates nothing

record_responsive_at(_):
    let was_open = open_until.take().is_some();
    consecutive = 0;
    if was_open { Closed } else { None }

record_unresponsive_at(now):
    if open_until.is_some() { return None }             // already open: a failed
        // probe needs no action (admit_at re-armed the window when it handed the
        // probe out), and a stale pre-trip report is a pure no-op.
    if failure_threshold == 0 { return None }           // breaker disabled; never accumulate
    consecutive = consecutive.saturating_add(1);
    if consecutive >= failure_threshold {
        open_until = Some(now + cooldown);
        Opened { cooldown }
    } else { None }
```

Three properties worth calling out:

- **`admit_at` re-arms the window when it hands out a `Probe`**, instead of tracking a "probe in
  flight" flag. A failed probe therefore needs no bookkeeping at all — the window is already extended,
  and the next `admit_at` past it admits the next probe. This also keeps the breaker robust to a
  dropped or cancelled probe future (a cancelled query): the circuit can never get stuck open waiting
  for a result that will never be reported.
- **Reports arriving while open are no-ops.** Every request admitted before the trip is still in
  flight when the circuit opens and reports `unresponsive` shortly after; those stale reports cannot
  perturb anything, because there is no per-window state left for them to corrupt.
- **A plain `std::sync::Mutex`**, never held across an `await`. Contention is a few nanoseconds
  against a network round trip; a lock-free atomics encoding would need a CAS loop for the same
  semantics and no measurable gain.
- **`admit_bypass_only` never re-arms `open_until`.** Only `admit_at`'s cooldown-elapsed arm does
  that, and only because it is handing out the one `Probe` that arm exists to gate. A caller that can
  never usefully receive a `Probe` (`prefetch`) must not trigger that re-arm just by asking, or a burst
  of such calls while open would keep pushing the next demand read's probe further out — see "Prefetch
  does not close the circuit".

#### Why the cooldown is fixed

Issue #1360 prescribes "exponential backoff cooldown starting at 100ms, doubling on each failed probe,
capped at 30s". This plan uses a fixed 3s cooldown instead. The backoff was paying for itself with a
large fraction of the state machine, and buying very little:

- **What it saved.** Fewer probes during a long outage — over 30 minutes, ~60 probes instead of 600.
  But at most *one* probe is outstanding at a time, and a probe costs exactly one request a
  stall-then-fallback penalty while every other request bypasses for free. 600 probes over 30 minutes
  is 0.33 req/s from a query process. That is not the load this gate exists to prevent (the failure
  mode is client-side resource exhaustion from parked tasks).
- **What it cost.** A mutable `cooldown`, a `max_cooldown` knob, a `probe_budget` knob, an
  `initial_cooldown` knob with a "must be >= `probe_budget`" invariant, a
  `window() = max(cooldown, probe_budget)` floor to keep probes from overlapping, and a
  `backoff_applied` flag whose *only* job was to stop stale pre-trip reports from compounding the
  doubling straight to the cap. Every one of those exists to serve the doubling.
- **The floor's premise was also not quite true.** `read_timeout` resets per response frame, so a
  slow-but-progressing `/obj` body can keep an admitted request alive up to `total_timeout` (15s), not
  a 3s probe budget — so the floor never strictly guaranteed one probe at a time anyway. With no
  doubling to corrupt, an occasional overlapping probe is simply harmless.
- **It made cold-cache rewarming slower.** Under backoff, a cache that trips after a restart gets
  retried on a schedule decaying toward 30s, so demand-driven rewarming (the fixed probe cadence — see
  "Cold-cache tripping" in "Prefetch does not close the circuit") stalls for longer. A fixed 3s cooldown
  re-probes steadily instead.

So the cooldown is one value, fixed, and the client passes its `stall_timeout` (3s) for it — reusing an
existing number instead of adding a second knob. This does not guarantee one probe at a time (the floor
never did, per the point above); it doesn't need to, since an overlapping probe is simply harmless with
no doubling left to corrupt. `Transition::Backoff` is gone with it.

### Wiring into `CacheClientStore`

**One send helper** — every cache request already goes through `.send()` at five sites
(`get_range_stream`, `get_full_stream`, `head_size`, `get_ranges`, `prefetch`). All five take the same
`abandon_timeout`, so the helper needs no per-caller budget parameter and the timeout-wrap plus the
failure bookkeeping exist exactly once:

```rust
/// Send a request to the cache, bounding time-to-headers with
/// `config.abandon_timeout` and reporting *failures* to the circuit breaker.
/// Success is deliberately NOT reported here: a single logical read can issue
/// two requests (Suffix does `head_size` then `get_range_stream`), and
/// reporting responsive at each time-to-headers would zero `consecutive` so a
/// later-phase failure could never trip the breaker. See "One outcome per
/// logical operation". Every call site is recoverable — dropping the future
/// cancels the request and lands in the existing fallback — which is what
/// makes one budget correct for all of them.
async fn send(&self, req: reqwest::RequestBuilder, what: &str) -> Result<reqwest::Response> {
    let budget = self.config.abandon_timeout;
    match tokio::time::timeout(budget, req.send()).await {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => { self.report_unresponsive(what); Err(e).with_context(|| format!("sending {what} to cache")) }
        Err(_) => { self.report_abandoned(what); Err(anyhow!("cache {what} did not beat the direct path within {budget:?}")) }
    }
}
```

`report_abandoned` and `report_unresponsive` differ only in which counter they emit
(`range_cache_client_abandoned` vs `range_cache_client_unresponsive`); both feed
`breaker.record_unresponsive()` (see "Abandon vs. unresponsive").

**`get_opts`.** `full_stream_with_fallback` takes an `Arc<CircuitBreaker>` (cloned from `self.breaker`,
since the `'static` stream can't borrow `&self`), the resolved byte range, and the `direct` store. It:

- counts bytes yielded;
- on a stream error at any position, emits `range_cache_client_stream_resumed`, reports
  `record_unresponsive`, and resumes the remainder from `direct` per "The fix: resume from the
  delivered offset". This is the *only* report a resumed operation makes — see "One outcome per
  logical operation" — so it never also reports `record_responsive` once the resumed remainder ends
  cleanly;
- on a clean end of stream **with no resume**, reports `record_responsive` — the single success report
  for this operation.

Being a free function it cannot call `self.report` or `self.report_unresponsive`; a free
`report_transition(t: Transition)` helper (mirroring the `direct_get_opts_with_metrics` split below)
does the metrics/logging that `#[must_use] Transition` requires, and a free `report_unresponsive(what:
&str, breaker: &CircuitBreaker)` helper emits `range_cache_client_unresponsive`, calls
`breaker.record_unresponsive()`, and feeds the resulting `Transition` to `report_transition` — the
resume path calls this directly, since it is the one place a free function needs to report unresponsive
at all. `CacheClientStore::report`/`report_unresponsive` are thin methods over these same two free
helpers, so both call sites (the free function and the struct) agree on what fires. No `Duration` is
threaded in: the body's per-frame bound is `read_timeout(stall_timeout + abandon_timeout)` on the client
itself, so a stall surfaces through the same `Err` item as any other transport error.

`head_size` (`client.rs:190-205`) is one function shared by three call sites (the head-only path, the
no-`Content-Range` path, and the Suffix path below), and it reports `record_responsive` internally only
on its own two after-2xx failure arms — non-2xx status and malformed/missing `Content-Length`
(`client.rs:197-199`, `200-204`) — because both are terminal for every caller: whichever site called
`head_size`, a failure there always falls back to `direct` without going any further. Those two internal
reports therefore fire identically no matter which of the three sites is calling. The *success* report —
`head_size` returning a parsed size — is not made inside `head_size` at all; it is made by whichever call
site owns the whole logical operation. The head-only path (`client.rs:486-509`) reports
`record_responsive` itself on a successful `head_size` return, since nothing follows it.
`get_range_stream`'s no-`Content-Range` path (`client.rs:132-143`) is the same shape: the server answered
a plain 200 (no `Content-Range`) for a zero-byte object or an EOF-starting open range, and `head_size`
there is the tail of the operation too — it returns a buffered empty `GetResult` built directly from the
size, with no stream that follows. It reports `record_responsive` at its own call site on a successful
`head_size` return, exactly like the head-only path. Only the **Suffix** path's `head_size`
(`client.rs:531`) is a genuine intermediate step — it feeds a `get_range_stream` call whose stream is
what completes the operation — so its call site adds no success report of its own; `head_size`'s internal
failure-arm reports still fire there exactly as they do for the other two callers, since a `head_size`
failure on the Suffix path also falls back to `direct` before `get_range_stream` is ever reached.

**`get_ranges`** does not reuse `send` for the whole operation: `send` collapses every header-phase
outcome into one `Result<reqwest::Response>`, but `get_ranges` has to keep four failure arms distinct —
non-2xx status, `RangesReadError::Transport`, the new `RangesReadError::Stalled`, and
`RangesReadError::Truncated` — each with its own `debug!`/`warn!` and its own responsive/unresponsive
classification (see "Abandon vs. unresponsive"). It also can't report success at the header phase at
all: `read_framed_ranges` exposes nothing to the caller until it fully resolves, so the single success
report for this operation belongs there, not at `send`'s boundary. It calls a `send_ranges` variant that
reuses `send` itself for the header phase — so the `abandon_timeout` wrap and its abandoned/unresponsive
reporting aren't duplicated — then drives `read_framed_ranges` to completion with `pull_exact`'s single
`stream.next()` wrapped in `tokio::time::timeout(stall_timeout, ...)` — per chunk (the same cost-knob
justification as `/obj`'s `read_timeout`, see "`stall_timeout` (3s) is now a cost knob", but a strictly
tighter deadline: `stall_timeout` (3s) here against `read_timeout`'s `stall_timeout + abandon_timeout`
(3.5s) on the client, so this explicit wrap — not `read_timeout` — is the one that actually fires on a
real stall, see "`ClientBuilder::read_timeout` on the one client"), so cumulative body size is never
bounded (constraint (b)). `send_ranges` returns a typed `Result<Vec<Bytes>, RangesSendError>` that keeps
those four failure arms distinct instead of folding them into one. `RangesSendError` is `send_ranges`'s
own enum, not a re-export: a `Send` arm wrapping the header-phase failure `send` already produces
(timeout/connect error), a `Status` arm carrying a non-2xx response, and a `Body(RangesReadError)` arm
carrying `read_framed_ranges`'s existing `Transport`/`Truncated` plus the new `Stalled` — so all five
failure kinds (header failure, non-2xx, `Transport`, `Stalled`, `Truncated`) each have a named home. Only
a `send()` timeout/connect error or `RangesReadError::{Transport,Stalled}` reports a failure; non-2xx
status and `Truncated` report `record_responsive` per the "any HTTP response counts as responsive" rule,
and `Truncated` keeps its `warn!`. All the failure kinds are exactly as safe to abort as dropping
`send()`'s future, since `read_framed_ranges` exposes nothing to the caller until it fully resolves, and
every arm falls back through the same existing `get_ranges` fallback path. The whole call is otherwise
bounded only by the unchanged 15s total deadline, so a healthy multi-megabyte read is never aborted on
size alone.

**One admission gate per public entry point** — `get_opts`, `get_ranges`, `prefetch`. Preconditioned
requests keep short-circuiting to `direct` before the gate (they never use the cache anyway):

```rust
if matches!(self.breaker.admit(), Admission::Bypass) {
    imetric!("range_cache_client_circuit_bypassed", "count", 1_u64);
    debug!("cache circuit open, reading {location} direct");
    return self.direct_get_opts(location, options).await;
}
```

`get_opts` and `get_ranges` use this `admit()` gate, where a `Probe` admission behaves exactly like
`Allow`. `prefetch` instead gates on `self.breaker.admit_bypass_only()`, which only ever returns
`Allow`/`Bypass` (see "Prefetch does not close the circuit") — so none of the three entry points keeps
the admission value around afterward, each only ever needs "cache or don't," and `prefetch` structurally
cannot consume the single per-cooldown probe slot a demand read would otherwise receive.

For `prefetch`, a `Bypass` admission (the only non-`Allow` outcome `admit_bypass_only()` can produce)
returns `Ok(PrefetchResponse { accepted: 0, rejected: 0, dropped: items.len() })` rather than `Err` — it
is semantically a load-shed, and callers already log `dropped` at debug
(`data_lake_connection.rs:71-74`). This deliberately avoids inflating
`range_cache_client_prefetch_error` with bypasses.

**Factor the duplicated fallback bookkeeping** while adding the eighth and ninth callers, so the
counter/timing pair stays in one place (mirroring `L1CacheStore::fallback_get_opts`,
`rust/object-cache/src/l1_store.rs:118-127`):

```rust
async fn direct_get_opts(&self, location: &Path, options: GetOptions) -> object_store::Result<GetResult>;
async fn direct_get_ranges(&self, location: &Path, ranges: &[Range<u64>]) -> object_store::Result<Vec<Bytes>>;
```

Both bump `range_cache_client_fallback` and time `range_cache_client_direct_ms`. `direct_get_opts`
delegates to a free `direct_get_opts_with_metrics(direct: &Arc<dyn ObjectStore>, ...)` so
`full_stream_with_fallback` (a free function) shares it for its resume read. Each call site keeps its
own `debug!` line before calling, so no diagnostic detail is lost. Counting the seven existing sites,
the two bypass gates are the eighth and ninth callers and the new `Stalled` arm makes a tenth.

Note that a circuit-bypassed read **does** count as `range_cache_client_fallback`. That metric is
documented as the primary "cache unhealthy" alert; if bypasses didn't count it, the alert would fall
silent precisely during an outage. `range_cache_client_circuit_bypassed` tells you *why* the fallbacks
are happening.

### Configuration

```rust
/// Tunables for `CacheClientStore`. `Default` carries the production values,
/// `from_env` applies operator overrides, and tests construct one directly
/// (short timeouts, near-zero or very long cooldown) instead of sleeping.
#[derive(Debug, Clone)]
pub struct CacheClientConfig {
    pub connect_timeout: Duration,  // 50ms
    /// The direct-path race budget: every phase where the cache can lose,
    /// which after Phase 0 is all of them. Applied at the header (time-to-
    /// headers) phase of the five `send()` sites.
    pub abandon_timeout: Duration,  // 500ms
    /// Per-chunk bound on `get_ranges`' `pull_exact` read. Looser than
    /// `abandon_timeout` because a mid-stream abandon re-reads the ranges
    /// not yet delivered — a cost knob, not a correctness bound. Also backs
    /// the `GET /obj` response body's per-frame bound, but *not* directly:
    /// the client's `ClientBuilder::read_timeout` is set to `stall_timeout +
    /// abandon_timeout` (looser by a margin), so `/ranges`' own explicit
    /// `pull_exact` wrap — set to `stall_timeout` exactly — is always the
    /// one that fires first on a real stall, never `read_timeout`; see
    /// "`ClientBuilder::read_timeout` on the one client". Reused as the
    /// breaker's `cooldown`.
    pub stall_timeout: Duration,    // 3s
    pub total_timeout: Duration,    // 15s (unchanged)
    pub breaker: CircuitBreakerConfig,
}
```

`CacheClientStore::new(url, api_key, direct)` keeps its signature and delegates to a new
`with_config(url, api_key, direct, CacheClientConfig::from_env())`, so `make_cache` and every existing
test but one are untouched — see Testing Strategy → Regression for the one exception.

50ms is calibrated for the only deployment this client documents: `mkdocs/docs/admin/object-cache.md`'s
**Client opt-in** section sets `MICROMEGAS_OBJECT_CACHE_URL=http://object-cache:8080` — plaintext,
intra-VPC — and its **Authentication** section requires the cache be bound to a private network
(security group / `NetworkPolicy`) with no public role at all; there is no TLS or cross-zone deployment
anywhere in-tree for this client. The budget isn't TCP-handshake-only, though: in reqwest 0.12.28,
`connect_timeout` wraps the whole connector service (`connect.rs:141-160`, `904-955`), which includes
hyper-util's blocking DNS resolution, not just the handshake — and a new connect (hence a new resolve)
happens on every cold pool slot against the documented `object-cache` hostname. A TLS or cross-zone
deployment, or DNS hiccups / resolver contention under a clustered nameserver, are all reasons to raise
the budget via `MICROMEGAS_OBJECT_CACHE_CLIENT_CONNECT_TIMEOUT_MS`.

#### Calibrating `abandon_timeout` (500ms)

Taken directly from issue #1360, which supplies both the data and the rule: origin-fetch latency
(`range_cache_origin_get_ms`) measured in production over a 5-minute sample under load is p50 ~36ms,
p90 ~152ms, p99 ~311ms, max ~575ms — and *"a slow cache response shouldn't be treated as a 'failure' —
past this budget it's simply no longer an optimization, since going direct is likely comparable or
faster."*

So 500ms is calibrated **at** that distribution deliberately, not above it. It is the point past which
the cache has already lost the race against the direct read the client would otherwise have done. This
is the reasoning a future reviewer is most likely to invert: the ~575ms tail is the *fallback's* cost,
not a healthy-cache latency the budget must accommodate. Sizing `abandon_timeout` above it would make
the client wait out a cache that has already lost.

**`range_cache_origin_get_ms` doesn't cover the whole header phase, though.** Per its own documentation
(`mkdocs/docs/admin/object-cache.md`), it measures the origin `get_range` call itself "once a permit was
held" — it excludes both queueing stages that also sit inside the `GET /obj` header phase this budget
bounds: the memory-budget permit wait (`object_cache_mem_permit_wait_ms`, `handlers.rs:349-359`) and,
before the origin call is even attempted, the fetch-permit wait (`range_cache_fetch_permit_wait_ms`,
`rust/object-cache/src/range_cache/fetch.rs`, emitted ahead of `origin.get_range`). Both distributions
grow under exactly the concurrent load a 500ms header bound gets exercised against, so the p50/p90/p99/max
figures above understate the phase they're anchored to. This is not a reason to raise the budget: queueing
inside the cache is part of what the client is racing against the direct path, and a header phase that
loses *because* the cache is saturated is exactly the case `abandon_timeout` is meant to shed — the cache
has stopped being an optimization for that request, permit contention or not. But the percentiles still
need checking so the default isn't accidentally shedding load under ordinary, non-saturated conditions.

One signal to add before merge: sanity-check 500ms against `object_cache_mem_permit_wait_ms` and
`range_cache_fetch_permit_wait_ms`, not just `range_cache_origin_get_ms`, for the reason above. Separately,
the header phases of `HEAD /obj` and `POST /ranges` contain an origin **HEAD**, not an origin GET (see
"Where the origin work actually lands"), so also check against `range_cache_origin_head_latency`. The rule
is unchanged in every case — a header phase that outruns the budget has equally stopped being an
optimization — but the percentiles differ per signal and the env override exists so the default can be
corrected without a redeploy.

#### The constants that are not env vars

Two: `stall_timeout` (3s) and `total_timeout` (15s), plus the breaker `cooldown` that is simply
`stall_timeout` rather than a value of its own. They stay named constants backing `Default`, not env
vars, mirroring `L1_TOTAL_FETCH_PERMITS` / `L1_DEMAND_RESERVED_FETCH_PERMITS` in `l1_store.rs:36-40`:
that tier exposes only its one operator-meaningful knob (`MICROMEGAS_OBJECT_CACHE_L1_MB`) and keeps its
secondary tuning private. Here the operator-meaningful knobs are the abandon budget, the connect
budget, and the threshold (`..._BREAKER_THRESHOLD=0` is the escape hatch if the breaker misbehaves in
production). Tests construct `CacheClientConfig` / `CircuitBreakerConfig` directly rather than needing
the env path.

That leaves five configured values in total — `connect_timeout`, `abandon_timeout`, `stall_timeout`,
`total_timeout`, `failure_threshold` — of which three are env-overridable.

| Variable | Default | Effect |
|---|---|---|
| `MICROMEGAS_OBJECT_CACHE_CLIENT_ABANDON_TIMEOUT_MS` | `500` | `abandon_timeout` — the direct-path race budget, applied at the header phase of every request |
| `MICROMEGAS_OBJECT_CACHE_CLIENT_CONNECT_TIMEOUT_MS` | `50` | Connect budget (raise for TLS / cross-zone / clustered DNS) |
| `MICROMEGAS_OBJECT_CACHE_CLIENT_BREAKER_THRESHOLD` | `5` | Consecutive failures to trip; `0` disables the breaker |

Parsed with the `warn`-and-default pattern from `l1_store.rs:49-57`, factored into a small private
`env_millis`/`env_u32` helper rather than repeated three times.

### Metrics and logging

State transitions log once (not once per request), so an outage doesn't flood:

| Metric | Emitted on | Log |
|---|---|---|
| `range_cache_client_abandoned` | An `abandon_timeout` expiry — the cache lost the race against the direct path | `debug!` |
| `range_cache_client_unresponsive` | Connect failure, transport error, a `stall_timeout` expiry, or a `total_timeout` expiry — the cache is not answering | `debug!` |
| `range_cache_client_stream_resumed` | A `/obj` body error at any position (including before the first chunk); the remainder — possibly the whole range — was read from `direct` | `debug!` with the resume offset |
| `range_cache_client_circuit_opened` | `Transition::Opened` | `warn!` with the cooldown |
| `range_cache_client_circuit_closed` | `Transition::Closed` | `info!` |
| `range_cache_client_circuit_bypassed` | Each read/prefetch that skipped the cache | `debug!` |

`abandoned` and `unresponsive` are mutually exclusive and both call
`CircuitBreaker::record_unresponsive`; they stay separate metrics so a dashboard can distinguish a slow
cache from a dead one (see "Abandon vs. unresponsive").

`range_cache_client_stream_resumed` fires on a `/obj` body error at any position, including at
`bytes_yielded == 0` — the collapsed pre-first-chunk fallback (today's `client.rs:324-346`) is the same
code path — so a bare occurrence count mixes plain fallbacks (nothing delivered, nothing re-read) with
genuine mid-stream resumes. Only occurrences with a **non-zero resume offset** mean the remainder of an
object was fetched twice and quantify wasted origin traffic; the `debug!` already logs that offset, so if
*those* climb on healthy caches, `stall_timeout` is too tight (see "`stall_timeout` (3s) is now a cost
knob").

There is no per-probe-failure transition to report: with a fixed cooldown a failed probe changes no
state, so a sustained outage emits `circuit_opened` once and then only `bypassed` volume until
recovery. Probe failures stay visible as the `abandoned` / `unresponsive` counters ticking at roughly
one per cooldown.

## Implementation Steps

### Phase 0 — close the invariant hole

1. In `client.rs`, rewrite `full_stream_with_fallback` to count yielded bytes and resume the remainder
   from `direct` on a stream error at **any** position, per "The fix: resume from the delivered offset".
   It takes the resolved byte range (from the `GetResult` built at `client.rs:294-299`) in addition to
   its current arguments. The existing two-arm match collapses to one path — the pre-first-chunk
   fallback becomes `bytes_yielded == 0`. An empty-or-negative remainder (`resume_start >=
   resolved_range.end`: all requested bytes already delivered, or a cache that over-delivered past its
   own declared range) ends the stream cleanly instead of issuing an empty or inverted `Bounded` read,
   which `object_store` rejects as an error.
2. Add unit/integration coverage that a mid-stream failure yields **byte-identical** data to a direct
   read (see Testing Strategy → Resume correctness). As a standalone PR, ahead of Phase 2, induce the
   failure by aborting the connection / dropping the response body outright — an immediate transport
   error needing no sleep and no `stall_timeout` override, since `with_config`/`CacheClientConfig` don't
   exist yet. The stall-based variants of these same tests (a handler that hangs past a short
   `stall_timeout` override) land once Phase 2 makes that overridable. With the abort-based tests, this
   step is independently valuable and could ship as its own PR ahead of the rest.

### Phase 1 — the breaker

3. Add `rust/object-cache/src/circuit_breaker.rs` with `CircuitBreakerConfig` (+`Default`),
   `Admission`, `Transition`, and `CircuitBreaker` with the `_at(now)` API above.
4. Register `pub mod circuit_breaker;` in `rust/object-cache/src/lib.rs` (public — the `tests/`
   directory is a separate crate and needs it) and re-export `CircuitBreaker`/`CircuitBreakerConfig`
   alongside the existing `pub use`s. Also re-export `CacheClientConfig` alongside the existing
   `pub use client::CacheClientStore` — the cross-crate integration test in
   `rust/object-cache-srv/tests/client_circuit_breaker_tests.rs` builds its client via `with_config`,
   so both `with_config` and `CacheClientConfig` must be `pub`.
5. Add `rust/object-cache/tests/circuit_breaker_tests.rs` (see Testing Strategy).

### Phase 2 — client timeouts and config

6. In `client.rs`, replace the two timeout constants with `CacheClientConfig` (+ `Default`,
   `from_env`, private env-parse helper). Keep the `Duration` values as named consts backing `Default`.
7. Add `with_config`; make `new` delegate to it. Store `config` and `breaker: Arc<CircuitBreaker>` on
   `CacheClientStore` (an `Arc` so `full_stream_with_fallback`'s `'static` stream can hold its own
   clone without borrowing `&self`). `CacheClientConfig::default`/`from_env` set
   `breaker.cooldown = stall_timeout`, so the two never drift; a test that overrides `stall_timeout`
   sets the cooldown it wants explicitly. Build the **one** `reqwest::Client` with
   `connect_timeout(config.connect_timeout)`, `read_timeout(config.stall_timeout +
   config.abandon_timeout)` and `timeout(config.total_timeout)` — no second client. The `read_timeout`
   margin (`abandon_timeout` above `stall_timeout`) exists so `/ranges`' own `pull_exact` wrap, set to
   `stall_timeout` exactly (step 9), is unambiguously the first of the two to fire on a real stall — see
   "`ClientBuilder::read_timeout` on the one client".
8. Add the `send(&self, req, what)` helper (no budget parameter; **failure reporting only**) and route
   `head_size`, `prefetch`, `get_range_stream` and `get_full_stream` through it. Wire the single
   success report per the "One outcome per logical operation" table: the head-only path's call site
   reports on a successful `head_size` return; the main `get_opts` paths report from
   `full_stream_with_fallback` on clean stream end **with no resume** — a resumed operation reports
   `record_unresponsive` only, from the resume path, and nothing else; the Suffix path's call site adds
   no success report of its own at its intermediate `head_size` call; the no-`Content-Range` path inside
   `get_range_stream` reports `record_responsive` at its own call site on a successful `head_size`
   return, since no stream follows there. (`head_size`'s own two after-2xx failure arms report
   `record_responsive` internally instead, identically for all three callers — see below and "Wiring
   into `CacheClientStore`" → `get_opts`.) Also report `record_responsive` at each caller's non-2xx
   status check
   (`client.rs:106-108`, `166-168`, `197-199`) before falling back to `direct`, per "Abandon vs.
   unresponsive" — a non-2xx there is a full response having arrived cheaply, the same rule `get_ranges`
   applies to its own non-2xx arm. Extend the same rule to the two malformed-header arms that follow a
   full 2xx response — `get_full_stream`'s missing-`Content-Length` check (`client.rs:169-171`) and
   `head_size`'s missing/unparseable-`Content-Length` check (`client.rs:200-204`) — reporting
   `record_responsive` there too before falling back, since a full response has already arrived; see
   "What does not feed the breaker as a failure". `prefetch`'s non-2xx status check (`client.rs:230-232`)
   instead reports nothing on any admission, per "Prefetch does not close the circuit". Give
   `full_stream_with_fallback` its `Arc<CircuitBreaker>` and
   have its resume path report `record_unresponsive` via the free `report_unresponsive` helper. Wire
   `prefetch`'s `resp.json()` read (`client.rs:233`) to `report_unresponsive` on error, but only for a
   body/transport/timeout error (`reqwest::Error::is_body()`/`is_timeout()`); on a decode error
   (`reqwest::Error::is_decode()`) report nothing, since a `PrefetchResponse` that fails to parse still
   arrived as a full, cheap 2xx — the same "malformed response after a cheap 2xx is not a failure" rule
   applied to `get_full_stream`'s and `head_size`'s malformed-`Content-Length` arms (see "What does not
   feed the breaker as a failure"). Both arms still get the `range_cache_client_prefetch_error` bump they
   already do. `send`
   calls `report_abandoned`/`report_unresponsive`, which step 12 adds; this step lands with a
   temporarily-uncompiled `send` until step 12's helpers exist (or do step 12 first — either ordering is
   fine, this is the one place the two steps are interdependent).
9. Add the `send_ranges` variant for `get_ranges`: reuse `send` (`abandon_timeout`) for the header
   phase, then drive `read_framed_ranges` to completion, with `pull_exact`'s `stream.next()` wrapped in
   `tokio::time::timeout(stall_timeout, ...)` — deliberately tighter than the client's `read_timeout`
   (`stall_timeout + abandon_timeout`, step 7) so this explicit wrap, not `read_timeout`, is the one that
   fires on a real stall — and a new `RangesReadError::Stalled` variant for the elapsed case. Report
   `record_responsive` only once the framed body fully resolves. `send_ranges` returns a typed
   `Result<Vec<Bytes>, RangesSendError>` — `Send`/`Status`/`Body(RangesReadError)`, see the `get_ranges`
   paragraph above — keeping the four failure arms distinct — non-2xx status, `Transport`, `Stalled`,
   `Truncated` — rather than folding them into one `Result<Vec<Bytes>>`; `get_ranges` keeps matching all
   four (plus success) with their current `debug!`/`warn!` logs, and only the
   timeout/connect/`Transport`/`Stalled` arms report a failure. Make `send_ranges` `pub` — it becomes
   reachable via the already-exported `CacheClientStore`, not via a `lib.rs` re-export, since it is an
   inherent method rather than a free-standing type — and re-export `RangesReadError`/`RangesSendError`
   from `rust/object-cache/src/lib.rs` alongside the `CacheClientConfig` export (step 4). Give both enums
   a `thiserror` `Display`/`Error` impl now that they're public API surface, not just internal detail.
   The cross-crate integration test in `rust/object-cache-srv/tests/client_circuit_breaker_tests.rs`
   calls `send_ranges` directly (in addition to the fallback-observing `get_ranges` call) so it can match
   on `RangesSendError::Body(RangesReadError::Stalled)`, since `get_ranges`'s `ObjectStore`-trait return
   type erases that classification into a fallback. `pull_exact` itself stays private; its behavior is
   exercised only indirectly, through `send_ranges`.

### Phase 3 — the gate

10. Add `direct_get_opts` / `direct_get_ranges` (+ the free `direct_get_opts_with_metrics`) and
    collapse the seven existing fallback blocks onto them.
11. Add the `Admission` gate to `get_opts` (after the precondition short-circuit) and `get_ranges` via
    `self.breaker.admit()`, where `Probe` behaves like `Allow`. Gate `prefetch` on
    `self.breaker.admit_bypass_only()` instead — a query that only ever returns `Allow`/`Bypass` and never
    re-arms `open_until` (see "Prefetch does not close the circuit") — so `prefetch` needs only the
    cache-or-not decision and cannot consume the single per-cooldown probe slot a demand read would
    otherwise receive.
12. Add the transition reporting helper as a free `report_transition(t: Transition)` (metrics/logs
    above), a free `report_unresponsive(what: &str, breaker: &CircuitBreaker)` (emits
    `range_cache_client_unresponsive`, calls `breaker.record_unresponsive()`, and feeds the resulting
    `Transition` to `report_transition`), plus thin `CacheClientStore::report`/`report_unresponsive`
    methods that delegate to those two free helpers, and the `report_abandoned` method (called only from
    `send`, which is already a method, so it needs no free form). `full_stream_with_fallback` calls the
    free `report_unresponsive` and `report_transition` directly, since it can't call `self.report` or
    `self.report_unresponsive`.

### Phase 4 — tests and docs

13. Add `rust/object-cache-srv/tests/client_circuit_breaker_tests.rs`.
14. Update `mkdocs/docs/admin/object-cache.md`, `mkdocs/docs/architecture/caching.md`, and
    `CHANGELOG.md`.
15. Before merge, sanity-check `abandon_timeout` (500ms) against production
    `object_cache_mem_permit_wait_ms` and `range_cache_fetch_permit_wait_ms` samples in addition to
    `range_cache_origin_get_ms` — both permit-wait stages sit inside the same header phase the budget
    bounds but are excluded from that origin figure (see "Calibrating `abandon_timeout` (500ms)"). Also
    check `range_cache_origin_head_latency` samples, since the `HEAD /obj` and `POST /ranges` header
    phases block on an origin HEAD, not a GET (see "Where the origin work actually lands"), and the two
    distributions can differ. Adjust the default via
    `MICROMEGAS_OBJECT_CACHE_CLIENT_ABANDON_TIMEOUT_MS` if any of them don't support it.
16. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 ../build/rust_ci.py`.

## Files to Modify

| File | Change |
|---|---|
| `rust/object-cache/src/client.rs` | Resume-from-offset, config, `send` helper, admission gate, factored fallbacks |
| `rust/object-cache/src/circuit_breaker.rs` | **New** — the state machine |
| `rust/object-cache/src/lib.rs` | Register/export the module |
| `rust/object-cache/tests/circuit_breaker_tests.rs` | **New** — synthetic-clock unit tests |
| `rust/object-cache-srv/tests/client_circuit_breaker_tests.rs` | **New** — resume correctness, trip/bypass/recover |
| `rust/object-cache-srv/tests/prefetch_tests.rs` | `body_larger_than_2mib_total_accepted_via_router` builds its client with a relaxed budget (see Testing Strategy → Regression) |
| `mkdocs/docs/admin/object-cache.md` | Client env vars, fast-fail section, 6 new metrics, amended `fallback` row |
| `mkdocs/docs/architecture/caching.md` | Note the fast-fail gate on the fallback edge |
| `CHANGELOG.md` | Entries under Unreleased → **Caching:** (the truncation fix, and the fast-fail gate) |

No changes needed in `rust/ingestion/src/data_lake_connection.rs`: `new` keeps its signature and
`from_env` reads the overrides.

## Trade-offs

- **The invariant is the design.** Making every position recoverable (Phase 0) is what lets one rule —
  abandon at the direct-path cost — apply everywhere, and what makes every subsequent threshold a
  performance decision rather than a stability one. The cost is one extra concept in
  `full_stream_with_fallback` (a byte counter and a resolved range), against which it deletes that
  function's second arm.
- **A mid-stream abandon re-reads the object's remainder.** That is real wasted origin traffic, which
  is why `stall_timeout` (3s) is looser than `abandon_timeout` (500ms) — see "`stall_timeout` (3s) is
  now a cost knob". `range_cache_client_stream_resumed` measures it, so the number can be corrected
  from production data.
- **Two deviations from issue #1360's literal wording.** (i) A flat 500ms `ClientBuilder::timeout`
  would be a *total* deadline and would abort every legitimate multi-megabyte read on cumulative size;
  the 500ms is applied as a per-phase abandon budget instead, targeting the same latency the issue
  measured while leaving throughput alone. (ii) The prescribed "exponential backoff starting at 100ms,
  doubling, capped at 30s" is replaced by a fixed 3s cooldown — see "Why the cooldown is fixed".
- **Slow-but-alive is treated the same as dead by the breaker.** Both feed `record_unresponsive` (see
  "Abandon vs. unresponsive"). Justified by the issue's own rule — a cache that consistently loses to
  the direct path is not worth routing through — and made safe by the invariant, since a bypass is just
  a direct read. It does mean a cold cache can be bypassed while warming, retried on the fixed 3s probe
  cadence rather than an exponential schedule — see "Cold-cache tripping" in "Prefetch does not close the
  circuit". The separate `abandoned`/`unresponsive` counters keep the two distinguishable operationally
  even though the breaker doesn't distinguish them.
- **Prefetch never closes the circuit, and structurally cannot consume a `Probe`.** Gating `prefetch` on
  `admit_bypass_only()` instead of `admit()` prevents accept-loop liveness from masking demand-path
  slowness — the oscillation a shared probe slot would otherwise allow: a write-active process's frequent
  prefetch calls winning the single per-cooldown probe and reopening the demand path on a ~3s cadence
  regardless of whether reads are actually recovering, echoing the parked-task exhaustion this plan
  removes. The cost is removing an accidental mitigation for cold-cache tripping (see "Cold-cache
  tripping"); the benefit, beyond avoiding that oscillation, is that neither `prefetch` nor its callers
  ever have to reason about `Probe` at all — it only ever gets "cache or don't."
- **A fixed cooldown instead of exponential backoff.** Costs more probe requests during a long outage
  (~600 vs ~60 over 30 minutes, at one outstanding probe at a time); buys a two-field state machine, no
  `max_cooldown`/`probe_budget`/`initial_cooldown` knobs, no window floor, no
  stale-report-compounds-the-doubling hazard, and steadier re-probing for a cold cache.
- **A stale success can close the circuit early.** A request admitted before the trip, completing
  successfully after it, reports `Closed`. Tracking a probe epoch would prevent that, but a fresh
  response is genuine liveness evidence, and the cost of acting on it is one more detection cycle.
  This is the only residual staleness effect: a stale failure while open is a plain no-op.
- **`ClientBuilder::read_timeout` on the one client, plus an explicit `tokio::time::timeout` on
  `/ranges` for error classification.** `read_timeout` is a single per-client value, set to
  `stall_timeout + abandon_timeout` (3.5s): the operative bound on the `/obj` body (along with
  `prefetch`'s `resp.json()`), and an inert backstop everywhere the explicit 500ms `abandon_timeout` wrap
  already applies. `/ranges` keeps its own explicit `stall_timeout` (3s) wrap around `pull_exact`
  anyway — deliberately tighter than `read_timeout`, not equal to it, because both timers clear and
  re-arm around the same poll and a tie would leave which one fires undetermined. The margin is what
  makes `pull_exact`'s wrap the one that actually surfaces on a real stall, not merely a value that would
  agree with `read_timeout` if it ever won the race. It also still buys error-classification granularity
  (a named `Stalled` variant instead of a bare `reqwest::Error`) and direct unit-testability with no
  server needed — reasons that hold regardless of which timer fires, but only matter in practice because
  the margin now guarantees `pull_exact`'s does. The remaining cost is the request-upload phase being
  folded into the header bound, and `/obj`'s real per-frame bound sitting at 3.5s rather than the 3s used
  in the cost-knob arithmetic (negligible difference). Not the "second hyper connection pool" cost an
  earlier draft claimed — that claim was wrong; one client covers both paths.
- **Any HTTP response counts as "responsive".** A 5xx-ing cache stays in circuit, because it answers
  cheaply and doesn't cause the resource exhaustion this gate exists to prevent; reads still fall back
  per-request as they do today. The gate is about *responsiveness*, not correctness.
- **`Mutex` over atomics** — correctness and readability over an unmeasurable win.
- **Hand-rolled, not a crate.** `failsafe`/`circuitbreaker-rs` would add a dependency for ~80 lines,
  and neither offers the injected-clock testability this needs.
- **Rejected: a bulkhead semaphore** capping concurrent in-flight cache requests. It bounds the damage
  but every request still queues and waits; the breaker removes the wait entirely. The two compose if a
  bulkhead is later wanted.
- **Rejected: hedged requests** (start the cache read, race a direct read after ~150ms). Strictly
  better latency, but it doubles origin traffic during every cold period and is a much larger change.

## Documentation

- `mkdocs/docs/admin/object-cache.md`
  - **Client opt-in**: add the three `MICROMEGAS_OBJECT_CACHE_CLIENT_*` variables as a table under the
    existing two, noting they are set in the *client's* environment, and that the connect budget spans
    DNS resolution as well as the TCP/TLS handshake — clustered DNS (search-path expansion, resolver
    contention) is a reason to raise `..._CONNECT_TIMEOUT_MS`, not just TLS/cross-zone.
  - New subsection after **What gets cached** — "Failing fast when the cache is unresponsive": lead
    with the invariant (no cache failure mode fails or corrupts a read), then the abandon rule, the trip
    condition, the fixed-3s probe schedule, and how to read the new metrics during an incident
    (`abandoned` vs `unresponsive` to tell slow from dead, `stream_resumed` for wasted origin traffic,
    `circuit_opened` once, `bypassed` climbing, `fallback` climbing with it, `circuit_closed` on
    recovery), plus the `..._BREAKER_THRESHOLD=0` escape hatch.
  - **Monitoring** table: the six new client metrics; amend the `range_cache_client_fallback` row to
    say it includes circuit-bypassed reads.
  - **Health and readiness**: the existing sentence about a cache outage surfacing as elevated
    client-side fallback traffic still holds; add that it now also surfaces as `circuit_opened`.
- `mkdocs/docs/architecture/caching.md`: the "any error (fallback)" edge (line 31) and the
  transparent-fallback paragraph get a sentence that the L2 hop is additionally gated by a fast-fail
  breaker, and that fallback now covers mid-stream failures too. Its "Configuration summary" table
  (`caching.md:170-177`, currently listing `MICROMEGAS_OBJECT_CACHE_URL`/`_API_KEY`/`_L1_MB`) gets a row
  pointing to the admin page's new `MICROMEGAS_OBJECT_CACHE_CLIENT_*` table.
- `CHANGELOG.md`: two bullets under Unreleased → **Caching:** — the silent-truncation fix and the
  fast-fail gate — referencing #1360.

## Testing Strategy

### Resume correctness — the invariant's own tests

These are the tests the invariant lives or dies on, and they belong with Phase 0 (in
`client_circuit_breaker_tests.rs`, or its own file if Phase 0 ships separately). The `direct` store
holds the **same** bytes as the cache path here (unlike the breaker tests below), because the assertion
is byte-for-byte equality with a healthy read.

**How the failure is induced depends on which phase has landed.** In a standalone Phase 0 —
before `with_config`/`CacheClientConfig` exist — the handler aborts the connection or drops the response
body outright: an immediate transport error, needing no sleep and no timeout override, that still
exercises the same resume arithmetic. Once Phase 2 lands and `stall_timeout` is overridable, each of
these tests gains a stall-based variant (the same handler instead hangs past a short `stall_timeout`
override) so the resume path is also exercised against a genuine stall, not just a hard abort:

- **Mid-stream failure yields complete, correct data.** An `/obj` handler that writes headers and some
  body chunks, then fails — aborts the connection (Phase 0) or hangs past `stall_timeout`, short override
  (Phase 2 variant). The read must return the **full** object, byte-identical to a direct read — not a
  short buffer. Run it at several truncation points (first chunk, middle, last-but-one) so the resume
  offset arithmetic is exercised, not just the happy splice.
- **Resume offset is right for every range shape.** The same failing handler, driven through a full
  read, a `Bounded` range, an `Offset` range, and a `Suffix` range. Each must return exactly the bytes
  the corresponding direct read returns. This is where a bug would silently corrupt data, so assert on
  content, never on length alone.
- **Regression guard for the old behavior.** A test that would have passed before Phase 0 — a
  mid-stream failure returning `Ok` with short bytes — must now fail. Concretely: assert
  `result.bytes().await?.len() == expected_len`, which is exactly the check `collect_bytes`
  (`object_store` `util.rs:52-75`) does not perform.
- **A failing resume surfaces an error.** Fail the cache mid-stream (abort in Phase 0, stall in the
  Phase 2 variant) *and* point `direct` at a store that errors; the caller must see the direct store's
  error, not a silent short read.
- **A stream error exactly at the last byte ends cleanly.** An `/obj` handler that delivers the full
  requested range and only then errors (e.g. the connection drops, or in the Phase 2 variant hangs before
  its terminating chunk). The read must complete successfully with exactly the requested bytes and must
  not call `direct` at all — this also guards against ever constructing `GetRange::Bounded(x..x)`, which
  `object_store` rejects as `InvalidGetRange::Inconsistent`.

### Unit — `rust/object-cache/tests/circuit_breaker_tests.rs`

Synthetic clock (`let base = Instant::now()` then `base + Duration::from_millis(n)`) through the
`_at` API — fully deterministic, zero sleeps:

- Below threshold stays `Allow`; a `record_responsive` resets the consecutive counter (4 failures,
  one success, 4 more failures → still closed).
- Trips at exactly `failure_threshold` consecutive failures; `Transition::Opened` reported once.
- `Bypass` for the whole cooldown; at `open_until` exactly one `Probe`, and an immediate second
  `admit_at` at the same instant returns `Bypass`.
- Failed probe → `Transition::None`, and the next `Probe` is admitted exactly one `cooldown` later
  (not sooner, and not on a doubled schedule) — the fixed-cadence property.
- Any number of `record_unresponsive_at` calls while open are no-ops (`Transition::None`) and do not
  move `open_until` — covering stale in-flight pre-trip reports draining after the trip.
- Successful probe → `Closed`, `Allow` afterwards, and re-tripping opens with the same `cooldown` as
  the first trip.
- `failure_threshold: 0` → always `Allow`, never opens.
- Cancelled probe (admit a `Probe`, never report) → next `admit_at` after the extended window returns
  `Probe` again, i.e. no permanently-stuck circuit.
- `admit_bypass_only` never returns `Probe` and never mutates `open_until`: while open, past the
  cooldown instant, any number of calls to it return `Bypass` and leave the state such that the very
  next `admit_at` call still returns `Probe` — i.e. a burst of `admit_bypass_only` calls cannot consume
  or delay the demand path's probe slot.

### Integration — `rust/object-cache-srv/tests/client_circuit_breaker_tests.rs`

An axum server whose handler increments an `AtomicUsize` and then parks on a
`tokio::sync::watch::Receiver::changed()` — a controllable hang, released by the test rather than by
elapsed time. Client built with `with_config` (`abandon_timeout`/`stall_timeout` both overridden to
short values, e.g. ~50ms and ~200ms, so the test is fast and doesn't wait out the production defaults);
`read_timeout` needs no override of its own — it's built from `config.stall_timeout +
config.abandon_timeout` (step 7), so overriding the two inputs keeps the same margin and ordering as
production automatically;
`breaker.cooldown` is set per test — long where the test asserts a bypass, near-zero where it wants an
immediate probe. The two "slow-but-progressing body stays closed" tests below are the exception: they
override `stall_timeout` to something generous instead (e.g. >= 1s, with chunks every ~100ms), because
the property under test — that the bound resets per chunk rather than bounding cumulative time — is
scale-independent, so headroom there costs no coverage, and a tight margin would make the test a
wall-clock race the plan's own flakiness standard (see Regression) would reject. Following the precedent
in
`rust/object-cache-srv/tests/memory_budget_tests.rs:589-594`, the `direct` store holds *different* bytes
from the cache path, so cache-vs-direct service is observable in the returned data.

- **Trip and bypass**: with a 60s cooldown, issue `threshold` sequential reads against the hung
  server — each returns the direct bytes (never an error). Snapshot the server's request counter,
  issue three more reads, assert the counter is unchanged: the cache was skipped without a connection.
  No sleeps.
- **Probe and recovery**: with a near-zero cooldown, trip the breaker, release the hang, then read
  again — the next read is admitted as a probe, returns the *cache's* bytes, and subsequent reads keep
  using the cache path (counter climbing again).
- **`get_ranges`** is gated too: same trip, then a `get_ranges` call returns correct direct data with
  no new server request.
- **`get_opts` slow header abandons but stays recoverable**: an `/obj` handler held past the short
  `abandon_timeout` override before writing headers. Asserts every such read still returns correct data
  (from `direct`, never an error) and that repeating it `failure_threshold` times trips the breaker.
- **`get_opts` mid-body stall trips the breaker**: the mid-stream stalling handler from "Resume
  correctness", which — unlike the rest of the breaker tests below — points `direct` at the *same* bytes
  as the cache path, for the same reason as the resume-correctness tests: the assertion here is
  byte-identical resumed data, not cache-vs-direct service, so cache involvement is instead observed
  through the server's request counter. The handler must set `Content-Length` explicitly (or answer as a
  206 with `Content-Range`, matching the real `get_range_handler`) — `get_full_stream`/`get_range_stream`
  require one of those to start streaming at all (`client.rs:169-171`, `client.rs:110`), and a plain
  `Body::from_stream` response sets neither, which would fail at the header stage and fall back instead
  of stalling mid-body. Each call returns complete correct data (resumed from `direct`) *and* reports
  `record_unresponsive`, so repeating it `failure_threshold` times trips the breaker — a subsequent
  `/obj` read is served from `direct` with no new server request. This is the test that would have been
  impossible under the old header-phase success reporting (see "One outcome per logical operation").
- **Suffix read trips the breaker**: an `/obj` handler that answers `HEAD` promptly but hangs on `GET`,
  driven through a `Suffix` read `failure_threshold` times. Asserts the breaker opens — the case where
  reporting responsive at the intermediate `head_size` would have pinned `consecutive` at zero forever.
  Parquet footer reads are suffix reads, so this is the production-relevant instance.
- **`get_opts` slow-but-progressing body stays closed**: built with a generous `stall_timeout` override
  (e.g. >= 1s) rather than the ~200ms used above, the handler emits a chunk every ~100ms for well longer
  than `stall_timeout` in total. Asserts the read completes with the cache's bytes, no resume occurs, and
  the breaker never opens — confirming `read_timeout` resets per frame and never bounds cumulative size,
  with enough absolute margin that the assertion isn't a wall-clock race under CI contention.
- **`get_ranges` read-stall**: a `/ranges` handler that writes the framed length-prefix header, then
  hangs on the same `watch::Receiver::changed()` mechanism past `stall_timeout` before the test
  releases it. Asserts a single such call still returns correct direct-store bytes (the stall is
  recoverable — `read_framed_ranges` exposes nothing until it fully resolves), and that repeating it
  `failure_threshold` times trips the breaker. Because `read_timeout` is configured with its margin
  above `stall_timeout` (`stall_timeout + abandon_timeout`, see "`ClientBuilder::read_timeout` on the one
  client"), this hang deterministically surfaces as `RangesReadError::Stalled`, not `Transport`. Since
  `get_ranges`'s public `ObjectStore` return type erases that classification into a fallback, assert it by
  calling the now-`pub` `send_ranges` directly against the same handler and matching
  `RangesSendError::Body(RangesReadError::Stalled)`, alongside the `get_ranges` call above — not just
  "some failure was reported". This is the test for constraint (a).
- **`get_ranges` slow-but-progressing body stays closed**: same generous `stall_timeout` override as the
  `get_opts` version above (e.g. >= 1s, chunks every ~100ms), for a total well past it — `read_timeout`
  follows automatically at its derived margin (see above), keeping the two ordered the same way as
  production. Asserts the call returns the cache's bytes and the breaker never opens — confirming the
  `pull_exact` wrap is per chunk, not cumulative (constraint (b)), with the same wall-clock headroom and
  for the same reason.
- **`get_ranges` non-2xx / truncated stays closed**: a `/ranges` handler returning a 500, and separately
  one that truncates the framed body mid-range (mirroring `FailAtOffsetStore` / the mid-stream
  truncation case in `memory_budget_tests.rs:505-552`), each called past `failure_threshold`.
  `get_ranges` falls back to direct bytes every time, but the server's request counter keeps climbing on
  every subsequent call (no `Bypass`, breaker never opens) — confirming non-2xx and `Truncated` are
  classified as responsive, not folded in with the read-stall/transport-error path.
- **`prefetch` success does not close the circuit**: with the breaker at `failure_threshold - 1`
  consecutive failures, a successful `prefetch` must leave `consecutive` where it was — one more read
  failure still trips. Then, with a long cooldown, trip the breaker and issue several `prefetch` calls
  while open — each returns `Ok` with `dropped == items.len()` and issues no server request, confirming
  `prefetch` stays gated (`Bypass`, no cache access) rather than closing the circuit on success. (A real
  clock can't discriminate `admit()` from `admit_bypass_only()` here — both leave the probe slot
  untouched on a fast-forgotten near-zero cooldown by the time the next call runs. That property —
  that a burst of `prefetch` calls cannot consume or re-arm the demand path's probe slot — is what the
  `admit_bypass_only` synthetic-clock unit test above covers instead.)
- **`prefetch` while open** returns `Ok` with `dropped == items.len()`, `accepted == 0`, and issues no
  request.
- **Breaker disabled** (`failure_threshold: 0`): the counter keeps climbing on every read against the
  hung server, confirming the escape hatch.

### Regression

Existing `rust/object-cache-srv/tests/{memory_budget,telemetry,prefetch}_tests.rs` exercise the happy
cache path through `CacheClientStore::new` and must keep passing with the tighter defaults. A local
loopback server answers in single-digit milliseconds on every one of them except one:

`prefetch_tests.rs:865-910` (`body_larger_than_2mib_total_accepted_via_router`) posts **40,000** items
(>2 MiB of NDJSON) through the real router via `CacheClientStore::prefetch`, and measures ~0.29s
consistently on an idle dev machine. That is within ~2x of the 500ms `abandon_timeout` — the same order
of magnitude, and close enough to be flaky under CI contention. It is not a design problem: the handler
consumes and parses the whole body before writing headers (`handlers.rs:721-779`), so its
time-to-headers scales with payload size, and 40k items is nowhere near production shape. The only
production caller is `DataLakeConnection::warm_object` (`data_lake_connection.rs:57-80`), which posts a
batch of **one** item from a detached `spawn_with_context` task and logs failures at `debug`; at that
size the header phase really is a cache-health signal, so `prefetch` keeps the tight default rather than
being exempted.

Fix it in the test, not the design: have that one test build its client via `with_config` with a relaxed
`abandon_timeout` (and `stall_timeout`, since `read_timeout`'s header bound also spans the upload), so
it exercises the oversized-body behavior it was written for without being gated on the new default.
`python3 build/rust_ci.py` for the workspace.
