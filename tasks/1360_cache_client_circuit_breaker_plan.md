# Object-Cache Client Circuit Breaker Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1360

## Overview

`CacheClientStore` (`rust/object-cache/src/client.rs`) already degrades gracefully when the
object-cache server misbehaves: every read path falls back to the direct object store on error. What
it does *not* do is fail fast. Each request pays the full timeout budget (2s connect / 15s total)
before that fallback runs, so an unresponsive cache turns every concurrent read into a 15s-parked
task. During the outage this exhausted the query process's resources long before any request reached
its fallback.

This plan adds two things:

1. **A much shorter detection budget** — 50ms connect, then a 500ms `abandon_timeout` on every phase
   where giving up lands in the existing fallback, and a 3s `stall_timeout` on the one phase where
   giving up is a hard query failure instead (the `GET /obj` response body once bytes have been
   delivered). See "The rule: abandon where falling back is free" for why the boundary sits there and
   nowhere else.
2. **A circuit breaker** gating the whole client: after 5 consecutive unresponsive requests, reads
   and prefetches skip the cache entirely (no connection, no timeout cost) for an
   exponentially-backing-off cooldown, with exactly one probe request admitted per cooldown to detect
   recovery.

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
`async_impl/client.rs:1434-1443`). It therefore covers body streaming, not just the handshake.

### Fallback paths

Every cache-path failure already falls back, with the same three-line bookkeeping repeated at six
sites (`range_cache_client_fallback` counter, direct read, `range_cache_client_direct_ms` timing):

- `get_opts` head-only path (`client.rs:486-509`)
- `get_opts` main path — full / bounded / offset / suffix (`client.rs:512-555`)
- `get_ranges` non-2xx, transport-error, and truncated-framing arms (`client.rs:586-611`, `640-666`)
- `full_stream_with_fallback`, for a stream error *before the first chunk* (`client.rs:309-350`)

`prefetch` (`client.rs:211-244`) has no fallback (there is nothing to fall back to); it counts
`range_cache_client_prefetch_error` and returns `Err`, which callers treat as "the warm didn't
happen" (`rust/ingestion/src/data_lake_connection.rs:70-79`).

### Where the origin work actually lands

Earlier drafts of this plan split the budget on "which phases do origin work," on the premise that
`head_size` and the `/ranges` header phase do none. That premise is false, and the split it supports
does not exist: **every** read endpoint blocks on the origin in its header phase.

- **`GET /obj`** commits before streaming: `get_range_handler_inner` resolves `size()`, waits for a
  memory-budget permit, then awaits the first chunk — the per-block origin GET — before building the
  response (`rust/object-cache-srv/src/handlers.rs:282-395`, "Commit-before-stream"). Time-to-headers
  therefore *contains* an origin GET, and is what the client already measures as
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

### Why the `/obj` body's tail position is different

The fallback inside `full_stream_with_fallback` is only sound **before the first chunk** has been
yielded: once bytes have reached the consumer, a retry would re-emit an already-delivered prefix, so
the helper deliberately just ends the stream (`client.rs:302-308`) rather than retrying. A mid-stream
abort is therefore a **hard query failure** for the caller, not a degradation.

That is the one position in the whole client where giving up is not free, and it is the only reason
this plan needs a second, larger budget at all (see "The rule: abandon where falling back is free").

### Single instance per process

`make_cache` (`rust/ingestion/src/data_lake_connection.rs:86-108`) is the only construction site: one
`Arc<CacheClientStore>` per process, shared as both `Arc<dyn ObjectStore>` and
`Arc<dyn ObjectPrefetch>`. Breaker state therefore lives on the struct — no statics, no per-endpoint
sharding.

## Design

### The rule: abandon where falling back is free

**This is the one place the timeout split is decided. Read it before proposing a different one.**

Issue #1360 states the principle the whole budget hangs on, verbatim:

> Grounded in production measurements of origin-fetch latency (`range_cache_origin_get_ms`): p50
> ~36ms, p90 ~152ms, p99 ~311ms, max ~575ms over a 5-minute sample under load. A slow cache response
> shouldn't be treated as a "failure" — past this budget it's simply no longer an optimization, since
> going direct is likely comparable or faster.

The ~575ms tail is the cost of the **direct path** — the thing the cache is racing. The budget is
calibrated **at** that distribution, not above it. Once the cache has taken longer than a direct read
would have, the cache has stopped being an optimization and abandoning it is the *correct* outcome,
not a false abort. Raising the budget above the origin tail defeats the purpose: it makes the client
wait out a cache that has already lost the race.

So the split is **not** "phases that do origin work vs. phases that don't" (every phase does — see
"Where the origin work actually lands"), and it is **not** "headers vs. body." It is:

> **Abandon at the direct-path cost wherever abandoning falls back to the direct store. Use a larger
> liveness bound only where abandoning is unrecoverable.**

There is exactly one unrecoverable position in this client: the `GET /obj` response body *after* the
first chunk has been yielded downstream (`client.rs:302-308` — see "Why the `/obj` body's tail
position is different"). Everywhere else, giving up costs one direct read, which is what the client
would have paid anyway.

| Phase | Budget | Why | On expiry |
|---|---|---|---|
| Connect (all endpoints) | 50ms `connect_timeout` | Recoverable | reqwest error → unresponsive → fallback |
| `head_size`, `prefetch`, `get_full_stream`, `get_range_stream`, `get_ranges`: request → response headers | 500ms `abandon_timeout` (`tokio::time::timeout` around `send()`) | Recoverable: dropping the future cancels the request and lands in the existing fallback | abandoned → fallback (see "Abandon vs. unresponsive") |
| `get_ranges` body reassembly (`read_framed_ranges`/`pull_exact`) | 500ms `abandon_timeout` *per chunk* (`tokio::time::timeout` around the one `stream.next()`) — resets every chunk, never bounds cumulative size | Recoverable: `read_framed_ranges` exposes nothing to the caller until it fully resolves, so any point is safe to abandon. Constraint (b): total bytes are unbounded (`client.rs:613-624`; the server caps range *count* at 4096, not bytes), so no flat deadline is possible | `RangesReadError::Stalled` → fallback |
| `GET /obj` response body (both positions) | 3s `stall_timeout`, applied per response frame by `ClientBuilder::read_timeout` | **Unrecoverable past the first chunk** — a mid-stream abort is a hard query failure, so this bound must be genuine-liveness-sized, not race-sized | stream error → pre-first-chunk falls back; post-first-chunk ends the stream. Both report `record_unresponsive` |
| Whole request (all endpoints) | 15s `total_timeout` (`ClientBuilder::timeout`, unchanged) | Backstop only | reqwest error → fallback |

The `/obj` body row covers *both* positions at 3s even though the pre-first-chunk position is
recoverable, because commit-before-stream means there is almost nothing in that window: the server
already resolved the first chunk before it wrote the headers, so the gap between headers and first
byte is a socket write of bytes it already holds. Splitting the two positions onto different budgets
would add a wrap to buy nothing.

This deviates from the issue's literal wording ("total request timeout: 500ms") in one respect only:
a 500ms `ClientBuilder::timeout` is a *total* deadline and would kill every legitimate multi-megabyte
read on cumulative size. The 500ms is applied as a per-phase abandon budget instead, which targets the
same latency the issue measured while leaving throughput alone.

**What this buys.** A hard-down cache is detected per request in ~50ms (connect refused/timed out) or
~500ms (accepted but silent) instead of 15s, and after `failure_threshold` such requests the breaker
removes even that cost. Under concurrent load the trip happens roughly one detection budget in, since
the in-flight requests fail together. The one phase that still takes up to 3s to report is a `/obj`
body that stalls after delivering bytes — and that request was already unrecoverable, so the 3s buys
correct classification rather than latency.

Two constraints the table has to satisfy explicitly:

- **(a) `/ranges` commits headers before any origin work.** A header-only bound there could never
  detect "answered headers, then stuck on the origin," so the `/ranges` *body* carries its own
  per-chunk `abandon_timeout` via `pull_exact` (`client.rs:419-443`) — the one place that path awaits
  a chunk — surfacing as a new `RangesReadError::Stalled` variant that `read_framed_ranges` propagates
  exactly as it already propagates `Transport`. Directly unit-testable, no server needed.
- **(d) A `/obj` body stall must be bounded and reported**, not left to the 15s deadline. It is
  bounded by `read_timeout` and reported by giving `full_stream_with_fallback` an
  `Arc<CircuitBreaker>`: a stream error before the first chunk falls back *and* reports
  `record_unresponsive`; after the first chunk it still just ends the stream, but the breaker hears
  about it. `full_stream_with_fallback` needs no `Duration` threaded into it — `read_timeout` does the
  bounding.

#### `ClientBuilder::read_timeout` on the one client

The store keeps building exactly **one** `reqwest::Client`, now with
`read_timeout(stall_timeout)` alongside `connect_timeout` and the unchanged total `timeout`. In
reqwest 0.12.28, `read_timeout` is a per-frame timeout on the response body (`async_impl/body.rs:287-340`
— `ReadTimeoutBody` resets its sleep per frame) plus a non-resetting bound on the header phase
(`async_impl/client.rs:3053-3059`). Set to `stall_timeout` (3s) it is:

- the **operative** bound in the one place that needs it — the `/obj` response body — with no second
  client, no split hyper pool, and no hand-rolled `tokio::time::timeout` around `first.next()`;
- an inert **backstop** everywhere else, because every other phase already carries a strictly tighter
  500ms `tokio::time::timeout`.

The earlier draft rejected `read_timeout` on the grounds that it would require a dedicated `/ranges`
client and split the connection pools. That reason was wrong and is dropped. The real trade-offs of
using it are:

- **Error-classification granularity on `/obj`.** A body stall arrives as a generic `reqwest::Error`
  rather than a distinct "stalled" variant, so `/obj` stalls and `/obj` transport errors are
  indistinguishable in logs. Both are liveness signals and both are handled identically, so nothing
  downstream changes — only diagnosis is coarser. `/ranges` keeps its explicit `Stalled` variant
  because it needs the tighter 500ms bound anyway, which a single per-client `read_timeout` cannot
  express alongside the 3s `/obj` bound.
- **The request-upload phase is folded into the header bound.** `read_timeout`'s header-phase bound
  spans request upload as well as time-to-headers, so a `prefetch` upload is bounded at 3s rather than
  15s. Irrelevant in production (batches are one item) and only visible in one existing test (see
  Testing Strategy → Regression).

### Abandon vs. unresponsive

Two different facts arrive at the breaker, and it is worth being explicit that the plan deliberately
maps both onto the same input:

1. **Abandoned** — an `abandon_timeout` expiry. Means "the cache did not beat the direct path." It is
   *not* by itself evidence that the cache is broken: a cold cache after a restart loses that race
   legitimately. Emits `range_cache_client_abandoned`.
2. **Unresponsive** — a connect failure, a transport error, or a `stall_timeout` expiry (3s with no
   frame). Means "the cache is not answering at all." Emits `range_cache_client_unresponsive`.

Both call `CircuitBreaker::record_unresponsive`, because the question the breaker answers is not "is
the cache alive?" but "is routing through the cache still worth its cost?" — and by issue #1360's own
rule, a cache that has lost five consecutive races against the direct path is not, whatever the
reason. Bypassing it is then a latency *win* for those reads, not a degradation, and the probe
schedule re-tests continuously. The two counters stay separate so an operator can still tell a slow
cache from a dead one on a dashboard.

The one real cost of folding them together is that a cold cache can be bypassed while it is still
warming, which stalls demand-driven warming — that is the "Cold-cache tripping" open question below,
and it is the reason that question stays open.

What does **not** feed the breaker: a non-2xx status and `RangesReadError::Truncated`. Both mean a
full HTTP response arrived cheaply, so per the "any HTTP response counts as responsive" rule below
they report `record_responsive`; `Truncated` keeps its `warn!` as a protocol violation from our own
cache, not a health signal.

### The breaker

New module `rust/object-cache/src/circuit_breaker.rs`, deliberately free of any cache-specific naming
so it stays reusable (and so `imetric!`'s literal-name requirement doesn't leak into it): state
transitions are *returned* to the caller, which owns the metrics and logs.

```rust
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Consecutive unresponsive requests that trip the breaker. `0` disables it.
    pub failure_threshold: u32,     // 5
    /// Longest an admitted request can run before it reports an outcome — the
    /// client's `stall_timeout`. Every open window is at least this long (see
    /// `admit_at`), which is what guarantees at most one probe in flight.
    pub probe_budget: Duration,     // 3s (= CacheClientConfig::stall_timeout)
    pub initial_cooldown: Duration, // 3s (must be >= probe_budget to be meaningful)
    pub max_cooldown: Duration,     // 30s
}

/// What a caller may do with the guarded resource right now.
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
#[must_use]
pub enum Transition {
    None,
    Opened { cooldown: Duration },
    /// A probe failed; cooldown doubled.
    Backoff { cooldown: Duration },
    Closed,
}

pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Mutex<State>,
}

struct State {
    /// Consecutive unresponsive requests while closed; any response resets it.
    consecutive: u32,
    /// Current cooldown; doubles per failed probe, capped at `max_cooldown`.
    cooldown: Duration,
    /// `Some(t)` => open: bypass until `t`, then admit one probe.
    open_until: Option<Instant>,
    /// Whether the *current* open window has already doubled the cooldown
    /// once. Every request admitted before the trip is still in flight when
    /// it opens, and each one reports `unresponsive` shortly after; without
    /// this flag each of those stale reports would be read as its own failed
    /// probe and the cooldown would compound straight to `max_cooldown`
    /// before the real probe is ever admitted. Reset to `false` whenever
    /// `admit_at` opens a fresh window. Because every window is at least
    /// `probe_budget` long, the drain of stale pre-trip reports concentrates
    /// in the first window or two rather than spreading across dozens of
    /// them. See the note below the pseudocode.
    backoff_applied: bool,
}
```

Public API — each method has an `_at(now: Instant)` form (the real logic) plus a wrapper that passes
`Instant::now()`, so the state machine is unit-testable with a synthetic clock and no sleeps:

```rust
impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self;
    pub fn admit(&self) -> Admission;
    pub fn admit_at(&self, now: Instant) -> Admission;
    /// The resource answered (any HTTP status counts — it's alive).
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
// Every open window lasts at least as long as an admitted request can take to
// report an outcome. Without this floor the breaker admits a new probe every
// `cooldown` while the previous one is still parked for up to `probe_budget`
// (with a 100ms cooldown and a 3s budget: ~30 probes in flight at once), and
// because each admission re-arms `backoff_applied`, each of those failures
// doubles the cooldown — straight from 100ms to the 30s cap in ~8 steps, for a
// cache that was only down for a second.
window(): max(cooldown, probe_budget)

admit_at(now):
    if failure_threshold == 0 { return Allow }          // breaker disabled
    match open_until:
        None                 => Allow
        Some(t) if now < t   => Bypass
        Some(_)              => {
            open_until = Some(now + window());
            backoff_applied = false;                    // fresh probe window
            Probe
        }

record_responsive_at(_):
    let was_open = open_until.is_some();
    consecutive = 0; cooldown = initial_cooldown; open_until = None;
    backoff_applied = false;
    if was_open { Closed } else { None }

record_unresponsive_at(now):
    if open_until.is_some() {
        if backoff_applied {
            None                                         // stale report, already handled this window
        } else {
            cooldown = min(cooldown * 2, max_cooldown);
            open_until = Some(now + window());
            backoff_applied = true;
            Backoff { cooldown }
        }
    } else {
        consecutive += 1;
        if failure_threshold > 0 && consecutive >= failure_threshold {
            cooldown = initial_cooldown;
            open_until = Some(now + window());
            backoff_applied = false;                     // opening trip itself doesn't count as the probe's backoff
            Opened { cooldown }
        } else { None }
    }
```

The flag makes the cooldown doubling idempotent *within* a single open window, and the
`max(cooldown, probe_budget)` floor makes windows long enough that this is close to a whole-trip
guarantee. The *first* unresponsive report arriving while `open_until.is_some()` doubles the cooldown
(whether it is a genuine probe failure or a stale pre-trip request still draining); every other report
before the next probe is admitted is a no-op. Because a window is at least `probe_budget` long — the
longest an admitted request can run before reporting — stale pre-trip reports land in the first window
or two rather than spreading across dozens of them, so the drain costs at most a doubling or two
instead of escalating straight to `max_cooldown`. A probe-epoch/generation counter (tag each open
window, ignore reports tagged with an older one) would close the residual gap outright; deferred as a
refinement, not required for correctness.

Two properties worth calling out:

- **Probe admission extends `open_until` by `max(cooldown, probe_budget)`**, instead of tracking a
  "probe in flight" flag. Since that is at least as long as an admitted request can take to report,
  at most one probe is outstanding at a time, and only a genuine probe failure doubles the cooldown.
  Extending the window rather than holding a flag also keeps the breaker robust to a dropped or
  cancelled probe future (a cancelled query): the next window simply admits another probe, and the
  circuit can never get stuck open waiting for a result that will never be reported.
- **A plain `std::sync::Mutex`**, never held across an `await`. Contention is a few nanoseconds
  against a network round trip; a lock-free atomics encoding would need a CAS loop for the same
  semantics and no measurable gain.

Cooldown sequence after a trip: 3s → 6 → 12 → 24 → 30s (capped), reset to 3s on recovery. Setting
`initial_cooldown` below `probe_budget` has no effect on the probe cadence — the floor dominates — so
the default starts it *at* `probe_budget` rather than at an inert smaller value.

### Wiring into `CacheClientStore`

**One send helper** — every cache request already goes through `.send()` at five sites
(`get_range_stream`, `get_full_stream`, `head_size`, `get_ranges`, `prefetch`). All five take the same
`abandon_timeout`, so the helper needs no per-caller budget parameter and the timeout-wrap plus the
breaker bookkeeping exist exactly once:

```rust
/// Send a request to the cache, bounding time-to-headers with
/// `config.abandon_timeout` and reporting the outcome to the circuit breaker.
/// Any HTTP response counts as responsive, whatever its status: a 404 or 500
/// means the server is alive and answering cheaply, which is not the failure
/// mode this gate exists for. Every call site is recoverable — dropping the
/// future cancels the request and lands in the existing fallback — which is
/// what makes one budget correct for all of them (see "The rule: abandon
/// where falling back is free").
async fn send(&self, req: reqwest::RequestBuilder, what: &str) -> Result<reqwest::Response> {
    let budget = self.config.abandon_timeout;
    match tokio::time::timeout(budget, req.send()).await {
        Ok(Ok(resp)) => { self.report(self.breaker.record_responsive()); Ok(resp) }
        Ok(Err(e)) => { self.report_unresponsive(what); Err(e).with_context(|| format!("sending {what} to cache")) }
        Err(_) => { self.report_abandoned(what); Err(anyhow!("cache {what} did not beat the direct path within {budget:?}")) }
    }
}
```

`report_abandoned` and `report_unresponsive` differ only in which counter they emit
(`range_cache_client_abandoned` vs `range_cache_client_unresponsive`); both feed
`breaker.record_unresponsive()` (see "Abandon vs. unresponsive").

`get_opts`'s data-fetching paths report a second, later signal too: `full_stream_with_fallback` takes an
`Arc<CircuitBreaker>` (cloned from `self.breaker`, since the `'static` stream can't borrow `&self`) and
calls `record_unresponsive` on a stream error in *either* position — before the first chunk (where it
also falls back, exactly as today) or after (where it still just ends the stream, but the breaker hears
about it). No `Duration` is threaded in: the body's per-frame bound is `read_timeout(stall_timeout)` on
the client itself, so a stall surfaces through the same `Some(Err(_))` arm as any other transport
error. This is the only body-phase breaker feedback `get_opts` needs; the `send()` wrap above covers its
header phase.

`get_ranges` is the one caller that does **not** use `send` as-is: as established above, its response
headers are committed before any origin work, so reporting `record_responsive` the moment `send()`
resolves would call the cache healthy before the part that actually fails has even started. It instead
calls a `send_ranges` variant that wraps `req.send()` in `tokio::time::timeout(abandon_timeout, ...)`,
then drives `read_framed_ranges` to completion with `pull_exact`'s single `stream.next()` also wrapped
in `tokio::time::timeout(abandon_timeout, ...)` — per chunk, so cumulative body size is never bounded
(constraint (b)). `send_ranges` returns a typed `Result<Vec<Bytes>, RangesSendError>` that keeps the
existing failure arms distinct (non-2xx status, `RangesReadError::Transport`,
`RangesReadError::Truncated`) plus the new `RangesReadError::Stalled` (the per-chunk budget elapsing)
instead of folding them into one. Only a `send()` timeout/connect error or
`RangesReadError::{Transport,Stalled}` reports a failure to the breaker; non-2xx status and `Truncated`
report `record_responsive` per the "any HTTP response counts as responsive" rule, and `Truncated` keeps
its `warn!`. All the failure kinds are exactly as safe to abort as dropping `send()`'s future, since
`read_framed_ranges` exposes nothing to the caller until it fully resolves, and every arm falls back
through the same existing `get_ranges` fallback path. The whole call is otherwise bounded only by the
unchanged 15s total deadline, so a healthy multi-megabyte read is never aborted on size alone.

**One admission gate per public entry point** — `get_opts`, `get_ranges`, `prefetch`. Preconditioned
requests keep short-circuiting to `direct` before the gate (they never use the cache anyway):

```rust
if matches!(self.breaker.admit(), Admission::Bypass) {
    imetric!("range_cache_client_circuit_bypassed", "count", 1_u64);
    debug!("cache circuit open, reading {location} direct");
    return self.direct_get_opts(location, options).await;
}
```

A `Probe` admission behaves exactly like `Allow`; nothing downstream needs to know which it was,
since the breaker's own state decides how the outcome is interpreted.

For `prefetch`, a bypass returns `Ok(PrefetchResponse { accepted: 0, rejected: 0, dropped: items.len() })`
rather than `Err` — it is semantically a load-shed, and callers already log `dropped` at debug
(`data_lake_connection.rs:71-74`). This deliberately avoids inflating
`range_cache_client_prefetch_error` with bypasses.

**Factor the duplicated fallback bookkeeping** while adding the seventh and eighth caller, so the
counter/timing pair stays in one place (mirroring `L1CacheStore::fallback_get_opts`,
`rust/object-cache/src/l1_store.rs:118-127`):

```rust
async fn direct_get_opts(&self, location: &Path, options: GetOptions) -> object_store::Result<GetResult>;
async fn direct_get_ranges(&self, location: &Path, ranges: &[Range<u64>]) -> object_store::Result<Vec<Bytes>>;
```

Both bump `range_cache_client_fallback` and time `range_cache_client_direct_ms`. `direct_get_opts`
delegates to a free `direct_get_opts_with_metrics(direct: &Arc<dyn ObjectStore>, ...)` so
`full_stream_with_fallback` (a free function) shares it. Each call site keeps its own `debug!` line
before calling, so no diagnostic detail is lost.

Note that a circuit-bypassed read **does** count as `range_cache_client_fallback`. That metric is
documented as the primary "cache unhealthy" alert; if bypasses didn't count it, the alert would fall
silent precisely during an outage. `range_cache_client_circuit_bypassed` tells you *why* the
fallbacks are happening.

### Configuration

```rust
/// Tunables for `CacheClientStore`. `Default` carries the production values,
/// `from_env` applies operator overrides, and tests construct one directly
/// (short timeouts, zero/huge cooldowns) instead of sleeping.
#[derive(Debug, Clone)]
pub struct CacheClientConfig {
    pub connect_timeout: Duration,  // 50ms
    /// The direct-path race budget: every phase where giving up falls back to
    /// the direct store (all five `send()` sites plus `pull_exact`'s per-chunk
    /// read). Past this, the cache is no longer an optimization.
    pub abandon_timeout: Duration,  // 500ms
    /// Genuine-liveness bound for the one phase where giving up is a hard query
    /// failure: the `GET /obj` response body. Applied via
    /// `ClientBuilder::read_timeout`, and reused as the breaker's `probe_budget`.
    pub stall_timeout: Duration,    // 3s
    pub total_timeout: Duration,    // 15s (unchanged)
    pub breaker: CircuitBreakerConfig,
}
```

`CacheClientStore::new(url, api_key, direct)` keeps its signature and delegates to a new
`with_config(url, api_key, direct, CacheClientConfig::from_env())`, so `make_cache` and the existing
tests are untouched.

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

One signal to add before merge: the header phases of `HEAD /obj` and `POST /ranges` contain an origin
**HEAD**, not an origin GET (see "Where the origin work actually lands"), so the budget should be
sanity-checked against `range_cache_origin_head_latency` as well as `range_cache_origin_get_ms`. The
rule is unchanged either way — a HEAD that outruns the budget has equally stopped being an
optimization — but the percentiles differ and the env override exists so the default can be corrected
without a redeploy.

#### The constants that are not env vars

`stall_timeout` (3s), `probe_budget` (= `stall_timeout`), `initial_cooldown` (= `probe_budget`) and
`max_cooldown` (30s) stay named constants backing `Default`, not env vars, mirroring
`L1_TOTAL_FETCH_PERMITS` / `L1_DEMAND_RESERVED_FETCH_PERMITS` in `l1_store.rs:36-40`: that tier exposes
only its one operator-meaningful knob (`MICROMEGAS_OBJECT_CACHE_L1_MB`) and keeps its secondary tuning
private. Here the operator-meaningful knobs are the abandon budget, the connect budget, and the
threshold (`..._BREAKER_THRESHOLD=0` is the escape hatch if the breaker misbehaves in production);
`stall_timeout` is a liveness bound an order of magnitude above any observed origin latency and the
cooldown schedule is a tuning-not-a-decision surface. Tests construct `CacheClientConfig` /
`CircuitBreakerConfig` directly (short timeouts, zero/huge cooldowns) rather than needing the env path.

Three operator overrides, parsed with the `warn`-and-default pattern from
`l1_store.rs:49-57` (factored into a small private `env_millis`/`env_u32` helper rather than repeated
three times):

| Variable | Default | Effect |
|---|---|---|
| `MICROMEGAS_OBJECT_CACHE_CLIENT_TIMEOUT_MS` | `500` | `abandon_timeout` — the direct-path race budget, applied at every recoverable phase |
| `MICROMEGAS_OBJECT_CACHE_CLIENT_CONNECT_TIMEOUT_MS` | `50` | Connect budget (raise for TLS / cross-zone) |
| `MICROMEGAS_OBJECT_CACHE_CLIENT_BREAKER_THRESHOLD` | `5` | Consecutive failures to trip; `0` disables the breaker |

### Metrics and logging

State transitions log once (not once per request), so an outage doesn't flood:

| Metric | Emitted on | Log |
|---|---|---|
| `range_cache_client_abandoned` | An `abandon_timeout` expiry — the cache lost the race against the direct path | `debug!` |
| `range_cache_client_unresponsive` | Connect failure, transport error, or a `stall_timeout` expiry — the cache is not answering | `debug!` |
| `range_cache_client_circuit_opened` | `Transition::Opened` | `warn!` with the cooldown |
| `range_cache_client_circuit_closed` | `Transition::Closed` | `info!` |
| `range_cache_client_circuit_bypassed` | Each read/prefetch that skipped the cache | `debug!` |

`abandoned` and `unresponsive` are mutually exclusive and both call
`CircuitBreaker::record_unresponsive`; they stay separate metrics so a dashboard can distinguish a slow
cache from a dead one (see "Abandon vs. unresponsive").

`Transition::Backoff` logs at `debug!` (bounded to one per cooldown, no separate counter — the
`opened`/`closed` pair plus `bypassed` volume is enough to reconstruct it).

## Implementation Steps

### Phase 1 — the breaker

1. Add `rust/object-cache/src/circuit_breaker.rs` with `CircuitBreakerConfig` (+`Default`),
   `Admission`, `Transition`, and `CircuitBreaker` with the `_at(now)` API above.
2. Register `pub mod circuit_breaker;` in `rust/object-cache/src/lib.rs` (public — the `tests/`
   directory is a separate crate and needs it) and re-export `CircuitBreaker`/`CircuitBreakerConfig`
   alongside the existing `pub use`s.
3. Add `rust/object-cache/tests/circuit_breaker_tests.rs` (see Testing Strategy).

### Phase 2 — client timeouts and config

4. In `client.rs`, replace the two timeout constants with `CacheClientConfig` (+ `Default`,
   `from_env`, private env-parse helper). Keep the `Duration` values as named consts backing
   `Default`.
5. Add `with_config`; make `new` delegate to it. Store `config` and `breaker: Arc<CircuitBreaker>` on
   `CacheClientStore` (an `Arc` so `full_stream_with_fallback`'s `'static` stream can hold its own clone
   without borrowing `&self`). Build the **one** `reqwest::Client` with
   `connect_timeout(config.connect_timeout)`, `read_timeout(config.stall_timeout)` and
   `timeout(config.total_timeout)` — no second client.
6. Add the `send(&self, req, what)` helper (no budget parameter — `abandon_timeout` for all of them) and
   route `head_size`, `prefetch`, `get_range_stream` and `get_full_stream` through it. Give
   `full_stream_with_fallback` an `Arc<CircuitBreaker>` and have it call `record_unresponsive` on a
   stream error in either the pre- or post-first-chunk position (only the pre-first-chunk one also falls
   back); no `Duration` is threaded in, since `read_timeout` supplies the per-frame bound. Add the
   `send_ranges` variant for `get_ranges`: wrap `req.send()` in `abandon_timeout`, then drive
   `read_framed_ranges` to completion, with `pull_exact`'s `stream.next()` wrapped in
   `tokio::time::timeout(abandon_timeout, ...)` and a new `RangesReadError::Stalled` variant for the
   elapsed case. Report `record_responsive` only once the framed body fully resolves. `send_ranges`
   returns a typed `Result<Vec<Bytes>, RangesSendError>` that keeps the four failure arms distinct —
   non-2xx status, `RangesReadError::Transport`, `RangesReadError::Stalled`, `RangesReadError::Truncated`
   — rather than folding them into one `Result<Vec<Bytes>>`; `get_ranges` keeps matching all four (plus
   success) with their current `debug!`/`warn!` logs, and only the timeout/connect/`Transport`/`Stalled`
   arms report a failure to the breaker.

### Phase 3 — the gate

7. Add `direct_get_opts` / `direct_get_ranges` (+ the free `direct_get_opts_with_metrics`) and
   collapse the six existing fallback blocks onto them.
8. Add the `Admission::Bypass` gate to `get_opts` (after the precondition short-circuit),
   `get_ranges`, and `prefetch`.
9. Add the transition reporting helper (`fn report(&self, t: Transition)`) plus the two outcome
   helpers (`report_abandoned` / `report_unresponsive`) with the metrics/logs above.

### Phase 4 — tests and docs

10. Add `rust/object-cache-srv/tests/client_circuit_breaker_tests.rs`.
11. Update `mkdocs/docs/admin/object-cache.md`, `mkdocs/docs/architecture/caching.md`, and
    `CHANGELOG.md`.
12. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 ../build/rust_ci.py`.

## Files to Modify

| File | Change |
|---|---|
| `rust/object-cache/src/circuit_breaker.rs` | **New** — the state machine |
| `rust/object-cache/src/client.rs` | Config, `send` helper, admission gate, factored fallbacks |
| `rust/object-cache/src/lib.rs` | Register/export the module |
| `rust/object-cache/tests/circuit_breaker_tests.rs` | **New** — synthetic-clock unit tests |
| `rust/object-cache-srv/tests/client_circuit_breaker_tests.rs` | **New** — end-to-end trip/bypass/recover |
| `rust/object-cache-srv/tests/prefetch_tests.rs` | `body_larger_than_2mib_total_accepted_via_router` builds its client with a relaxed budget (see Testing Strategy → Regression) |
| `mkdocs/docs/admin/object-cache.md` | Client env vars, fast-fail section, 5 new metrics, amended `fallback` row |
| `mkdocs/docs/architecture/caching.md` | Note the fast-fail gate on the fallback edge |
| `CHANGELOG.md` | Entry under Unreleased → **Caching:** |

No changes needed in `rust/ingestion/src/data_lake_connection.rs`: `new` keeps its signature and
`from_env` reads the overrides.

## Trade-offs

- **Abandon at the direct-path cost; a larger bound only where abandoning is unrecoverable.** As
  specified literally, a flat 500ms `ClientBuilder::timeout` would abort every read whose body takes
  longer than half a second on cumulative size. Applying the same 500ms as a per-phase abandon budget
  targets the latency the issue measured without touching throughput. The one exception is the `/obj`
  response body: past the first chunk, aborting is a hard query failure rather than a fallback, so the
  "going direct is comparable or faster" reasoning does not apply there and it gets a genuine 3s
  liveness bound instead. The cost is a slower fast-fail specifically on that one phase.
- **Slow-but-alive is treated the same as dead by the breaker.** Both feed `record_unresponsive` (see
  "Abandon vs. unresponsive"). Justified by the issue's own rule — a cache that consistently loses to
  the direct path is not worth routing through — but it does mean a cold cache can be bypassed while
  warming — the "Cold-cache tripping" open question. The separate `abandoned`/`unresponsive` counters keep the two
  distinguishable operationally even though the breaker doesn't distinguish them.
- **`ClientBuilder::read_timeout` on the one client, plus explicit `tokio::time::timeout` where a
  tighter bound is wanted.** `read_timeout` is a single per-client value, so it cannot express both the
  3s `/obj` body bound and the 500ms `/ranges` body bound; it is set to the looser of the two, where it
  is the operative bound, and is an inert backstop everywhere the explicit 500ms wrap already applies.
  The costs are error-classification granularity on `/obj` (a body stall arrives as a generic
  `reqwest::Error`, not a distinct variant) and the request-upload phase being folded into the
  header bound (`prefetch` uploads bounded at 3s rather than 15s). Neither is the "second hyper
  connection pool" cost an earlier draft claimed — that claim was wrong; one client covers both paths.
- **Any HTTP response counts as "responsive".** A 5xx-ing cache stays in circuit, because it answers
  cheaply and doesn't cause the resource exhaustion this gate exists to prevent; reads still fall back
  per-request as they do today. The gate is about *responsiveness*, not correctness.
- **A stale success can close the circuit early.** A request admitted before the trip, completing
  successfully after it, reports `Closed`. Tracking a probe epoch would prevent that, but a fresh
  response is genuine liveness evidence, and the cost of acting on it is one more detection cycle.
  Symmetrically, a stale failure while open can trigger a `Backoff` it didn't strictly earn.
  `backoff_applied` makes the doubling idempotent within an open window, and the
  `max(cooldown, probe_budget)` floor makes windows at least as long as any admitted request can run,
  so the pre-trip drain concentrates in the first window or two instead of spreading across dozens of
  100ms windows and escalating to `max_cooldown`. A probe-epoch/generation counter (tag each open
  window, ignore reports tagged with an older one) would close the residual gap outright at the cost of
  one more field; deferred as a refinement, not required for correctness.
- **`Mutex` over atomics** — see above; correctness and readability over an unmeasurable win.
- **Hand-rolled, not a crate.** `failsafe`/`circuitbreaker-rs` would add a dependency for ~80 lines,
  and neither offers the injected-clock testability this needs.
- **Rejected: a bulkhead semaphore** capping concurrent in-flight cache requests. It bounds the
  damage but every request still queues and waits; the breaker removes the wait entirely. The two
  compose if a bulkhead is later wanted.
- **Rejected: hedged requests** (start the cache read, race a direct read after ~150ms). Strictly
  better latency, but it doubles origin traffic during every cold period and is a much larger change.

## Documentation

- `mkdocs/docs/admin/object-cache.md`
  - **Client opt-in**: add the three `MICROMEGAS_OBJECT_CACHE_CLIENT_*` variables as a table under
    the existing two, noting they are set in the *client's* environment, and that the connect budget
    spans DNS resolution as well as the TCP/TLS handshake — clustered DNS (search-path expansion,
    resolver contention) is a reason to raise `..._CONNECT_TIMEOUT_MS`, not just TLS/cross-zone.
  - New subsection after **What gets cached** — "Failing fast when the cache is unresponsive": the
    abandon-at-direct-cost rule and the one exception, the trip condition, the backoff/probe schedule,
    and how to read the new metrics during an incident (`abandoned` vs `unresponsive` to tell slow from
    dead, `circuit_opened` once, `bypassed` climbing, `fallback` climbing with it, `circuit_closed` on
    recovery), plus the `..._BREAKER_THRESHOLD=0` escape hatch.
  - **Monitoring** table: the five new client metrics; amend the `range_cache_client_fallback` row to
    say it includes circuit-bypassed reads.
  - **Health and readiness**: the existing sentence about a cache outage surfacing as elevated
    client-side fallback traffic still holds; add that it now also surfaces as `circuit_opened`.
- `mkdocs/docs/architecture/caching.md`: the "any error (fallback)" edge (line 31) and the
  transparent-fallback paragraph get a sentence that the L2 hop is additionally gated by a
  fast-fail breaker.
- `CHANGELOG.md`: one bullet under Unreleased → **Caching:**, referencing #1360.

## Testing Strategy

### Unit — `rust/object-cache/tests/circuit_breaker_tests.rs`

Synthetic clock (`let base = Instant::now()` then `base + Duration::from_millis(n)`) through the
`_at` API — fully deterministic, zero sleeps:

- Below threshold stays `Allow`; a `record_responsive` resets the consecutive counter (4 failures,
  one success, 4 more failures → still closed).
- Trips at exactly `failure_threshold` consecutive failures; `Transition::Opened` reported once.
- `Bypass` for the whole cooldown; at `open_until` exactly one `Probe`, and an immediate second
  `admit_at` at the same instant returns `Bypass`.
- Failed probe → `Backoff` with doubled cooldown; repeated `admit_at`/fail cycles saturate at
  `max_cooldown` and never exceed it.
- A second `record_unresponsive_at` in the *same* open window (no intervening `admit_at`) is a no-op —
  `Transition::None`, cooldown unchanged — covering the stale in-flight-failures case.
- Successful probe → `Closed`, `Allow` afterwards, and the cooldown resets to `initial_cooldown`
  (verified by re-tripping and observing the initial cooldown again, not 30s).
- **The `max(cooldown, probe_budget)` floor**: with `initial_cooldown` deliberately set *below*
  `probe_budget` (e.g. 100ms vs 3s), `admit_at` at `open_until` returns one `Probe` and every
  `admit_at` for the next `probe_budget` returns `Bypass` — not one probe per 100ms. Directly covers
  the overlapping-probe escalation the floor exists to prevent.
- `failure_threshold: 0` → always `Allow`, never opens.
- Cancelled probe (admit a `Probe`, never report) → next `admit_at` after the extended window returns
  `Probe` again, i.e. no permanently-stuck circuit.

### Integration — `rust/object-cache-srv/tests/client_circuit_breaker_tests.rs`

An axum server whose handler increments an `AtomicUsize` and then parks on a
`tokio::sync::watch::Receiver::changed()` — a controllable hang, released by the test rather than by
elapsed time. Client built with `with_config` (`abandon_timeout`/`stall_timeout` both overridden to
short values, e.g. ~50ms and ~200ms, so the test is fast and doesn't wait out the production defaults);
`breaker.probe_budget` is overridden to match `stall_timeout` so the open-window floor stays consistent
with the shortened budgets. Following the precedent in
`rust/object-cache-srv/tests/memory_budget_tests.rs:589-594`, the `direct` store holds *different* bytes
from the cache path, so cache-vs-direct service is observable in the returned data.

- **Trip and bypass**: with a 60s cooldown, issue `threshold` sequential reads against the hung
  server — each returns the direct bytes (never an error). Snapshot the server's request counter,
  issue three more reads, assert the counter is unchanged: the cache was skipped without a
  connection. No sleeps.
- **Probe and recovery**: with a zero cooldown *and* a zero `probe_budget` (so the floor doesn't
  suppress the immediate probe), trip the breaker, release the hang, then read again — the next read is
  admitted as a probe, returns the *cache's* bytes, and subsequent reads keep using the cache path
  (counter climbing again).
- **`get_ranges`** is gated too: same trip, then a `get_ranges` call returns correct direct data with
  no new server request.
- **`get_opts` slow header abandons but stays recoverable**: an `/obj` handler held past the short
  `abandon_timeout` override before writing headers. Asserts every such read still returns correct
  data (from `direct`, never an error) — the abandon lands in the existing fallback — and that
  repeating it `failure_threshold` times trips the breaker, since a cache that keeps losing to the
  direct path is exactly what the gate exists to skip.
- **`get_opts` mid-body stall**: an `/obj` handler that writes headers and one body chunk immediately,
  then hangs past `stall_timeout` (short override) before writing the rest. A single call surfaces as a
  stream error to the caller (unrecoverable once bytes have been delivered — see "Why the `/obj` body's
  tail position is different") but still reports `record_unresponsive`; repeating it
  `failure_threshold` times trips the breaker (a subsequent `/obj` read is served from `direct` with no
  new server request), confirming `read_timeout` bounds the body phase and that it feeds the breaker
  instead of parking silently for up to 15s.
- **`get_opts` slow-but-progressing body stays closed**: the same handler, but emitting a chunk every
  interval shorter than `stall_timeout` for longer than `stall_timeout` in total. Asserts the read
  completes with the cache's bytes and the breaker never opens — confirming `read_timeout` resets per
  frame and never bounds cumulative size.
- **`get_ranges` read-stall**: a `/ranges` handler that writes the framed length-prefix header, then
  hangs on the same `watch::Receiver::changed()` mechanism past `abandon_timeout` before the test
  releases it. Asserts a single such call still returns correct direct-store bytes (the stall is
  recoverable — `read_framed_ranges` exposes nothing until it fully resolves), and that repeating it
  `failure_threshold` times trips the breaker. This is the test for constraint (a): `/ranges` commits
  its headers before any origin work, so only the body-side bound can catch this.
- **`get_ranges` slow-but-progressing body stays closed**: the same handler emitting each framed range
  at an interval shorter than `abandon_timeout`, for a total well past it. Asserts the call returns the
  cache's bytes and the breaker never opens — confirming the `pull_exact` wrap is per chunk, not
  cumulative (constraint (b)).
- **`get_ranges` non-2xx / truncated stays closed**: a `/ranges` handler returning a 500, and
  separately one that truncates the framed body mid-range (mirroring `FailAtOffsetStore` / the
  mid-stream truncation case in `rust/object-cache-srv/tests/memory_budget_tests.rs:505-552`), each
  called past `failure_threshold`. `get_ranges` falls back to direct bytes every time, but the server's
  request counter keeps climbing on every subsequent call (no `Bypass`, breaker never opens) —
  confirming non-2xx and `Truncated` are classified as responsive per the "any HTTP response counts as
  responsive" rule, not folded in with the read-stall/transport-error path above.
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
production caller is `DataLakeConnection::warm_object`
(`rust/ingestion/src/data_lake_connection.rs:57-80`), which posts a batch of **one** item from a
detached `spawn_with_context` task and logs failures at `debug`; at that size the header phase really is
a cache-health signal, so `prefetch` keeps the tight default rather than being exempted.

Fix it in the test, not the design: have that one test build its client via `with_config` with a relaxed
`abandon_timeout` (and `stall_timeout`, since `read_timeout`'s header bound also spans the upload), so
it exercises the oversized-body behavior it was written for without being gated on the new default.
`python3 build/rust_ci.py` for the workspace.

## Open Questions

1. **Cold-cache tripping.** After a cache restart the whole working set is cold, so many *consecutive*
   requests can exceed `abandon_timeout` and open the circuit. Demand traffic then bypasses with one
   probe per cooldown, up to the 30s cap — the cache can only rewarm from prefetch/write-time warming,
   and demand-driven warming stalls. Note this is a direct consequence of folding "abandoned" into
   "unresponsive" (see "Abandon vs. unresponsive"): the reads themselves are correctly served direct
   either way, but the bypass is what suppresses rewarming. Acceptable, or should the max cooldown be
   lower (5s), or should prefetch requests be exempt from the gate so a bypassed cache still warms?
