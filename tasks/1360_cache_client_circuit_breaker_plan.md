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

1. **A much shorter detection budget** — 50ms connect, 500ms *time-to-response-headers* where nothing
   before headers touches the origin, 3s wherever it can (see "Two timeout phases, not one" for why
   `GET /obj` needs the larger of the two) — so an unresponsive cache is noticed in a few seconds
   instead of fifteen.
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

### Why the body phase can't share the tight budget

The fallback inside `full_stream_with_fallback` is only sound **before the first chunk** has been
yielded: once bytes have reached the consumer, a retry would re-emit an already-delivered prefix, so
the helper deliberately just ends the stream (`client.rs:302-308`) rather than retrying. A mid-stream
abort is therefore a **hard query failure** for the caller, not a degradation — but it is still a
meaningful liveness signal, so both positions report it to the breaker (see "Two timeout phases, not
one" below).

That matters because the cache server streams block-by-block, and each block may need its own origin
fetch: `range_cache_origin_get_ms` maxed at ~575ms in the sampled window, so inter-chunk gaps on a
cold multi-block range can legitimately exceed 500ms. A flat 500ms budget on the body phase would
convert cold-cache slowness into user-visible errors, which is why the body phase — on **both** `/obj`
and `/ranges` — gets the separate, larger `body_stall_timeout` (3s) instead: large enough to absorb that
observed tail, but still tight enough (versus the unbounded 15s total) to feed the breaker a real
signal when a stall is genuine.

### Where the header phase spends its time

The server commits before streaming, but only on the `GET /obj` path: `get_range_handler_inner`
resolves `size()`, waits for a memory-budget permit, then awaits the first chunk — which *is* the
per-block origin fetch, the same one whose tail reaches ~575ms — before building the response
(`rust/object-cache-srv/src/handlers.rs:282-395`, "Commit-before-stream"). So on that path
time-to-headers is exactly the quantity that includes the origin fetch and permit wait, and it is what
the client already measures as `range_cache_client_roundtrip_ms`. Bounding *that* is both the right
signal and the safe place to abort, since aborting there lands in the existing fallback — but a 500ms
budget on it would abort the same cold-but-healthy reads (p99–max: 311–575ms of origin time alone,
before permit wait and the network hop) that `body_stall_timeout` exists to tolerate elsewhere, so this
phase is bounded with `body_stall_timeout` (3s), not `header_timeout` (500ms): despite living at
"time to headers," `get_range_stream`/`get_full_stream`'s `send()` does the same class of work — a
phase that may include one origin fetch — that `body_stall_timeout` is sized for. `head_size` and
`prefetch` do no origin work before responding (`state.cache.size(&key)` is the same fast metadata
lookup `/ranges` also does within its own `header_timeout`, and prefetch only enqueues), so they keep
the tight `header_timeout`.

`POST /ranges` does **not** share this property. `post_ranges_handler_inner` awaits `framed.next()`
(`handlers.rs:~598`), but `frame_ranges_stream` (`handlers.rs:147-169`) yields the first range's 8-byte
little-endian length prefix before it ever polls the underlying block stream, and `stream_ranges_inner`
(`rust/object-cache/src/range_cache/mod.rs:381-414`) is a lazy `try_stream!` that fetches nothing until
first polled. So `/ranges` time-to-headers is key validation + `size()` + memory-permit wait only — the
block/origin fetch happens entirely after the length prefix has already been sent, i.e. in the body
phase, where it needs its own stall coverage rather than an end-to-end deadline (see below).

### Single instance per process

`make_cache` (`rust/ingestion/src/data_lake_connection.rs:86-108`) is the only construction site: one
`Arc<CacheClientStore>` per process, shared as both `Arc<dyn ObjectStore>` and
`Arc<dyn ObjectPrefetch>`. Breaker state therefore lives on the struct — no statics, no per-endpoint
sharding.

## Design

### Two timeout phases, not one

| Phase | Budget | On expiry |
|---|---|---|
| Connect | 50ms (`connect_timeout`) | reqwest error → unresponsive → fallback |
| `head_size`/`prefetch`: request → response headers (no origin work) | 500ms (`header_timeout`, `tokio::time::timeout` around `send()`) | future dropped → unresponsive → fallback |
| `get_ranges`: request → response headers (no origin work — see above) | 500ms (`header_timeout`, `tokio::time::timeout` around `send()`) | future dropped → unresponsive → fallback |
| `get_opts` (`GET /obj`, full or ranged): request → response headers (**contains the first block's origin fetch** — commit-before-stream) | 3s (`body_stall_timeout`, `tokio::time::timeout` around `send()` — deliberately not `header_timeout`; see "Where the header phase spends its time") | future dropped → unresponsive → fallback |
| `get_opts` response body (chunks after the first) and `get_ranges` body reassembly (`read_framed_ranges`/`pull_exact`) | 3s *inter-chunk stall* (`body_stall_timeout`, `tokio::time::timeout` around each `.next()` read on the one shared client — resets every chunk, never fires on cumulative size); total still bounded only by the unchanged 15s deadline | stalled read → unresponsive; pre-first-chunk (`get_opts`) / pre-completion (`get_ranges`) falls back, post-first-chunk (`get_opts`) ends the stream as a hard query failure but still reports the stall |

This deviates from the issue's wording ("total request timeout: 500ms") for the reason above: a 500ms
`ClientBuilder::timeout` would kill every legitimate multi-megabyte read. For `head_size`/`prefetch` and
`get_ranges`'s header wait, wrapping `send()` in `tokio::time::timeout(header_timeout, ...)` bounds
precisely the phase where no origin work happens yet and abandoning is safe. `get_opts`'s data-fetching
paths (`get_full_stream`/`get_range_stream`) wrap the same `send()` call in `body_stall_timeout` instead,
because commit-before-stream puts the origin fetch inside that same await (see "Where the header phase
spends its time"). Dropping either future cancels the request and releases the connection.

The body phase needs its own stall bound too, on **both** `get_opts` and `get_ranges` — not just
`get_ranges` — because a cache that answers promptly and then stalls mid-body is exactly the failure
mode this plan exists to catch, and today only the unchanged 15s `ClientBuilder::timeout` bounds it on
either path. Neither body can take a flat end-to-end deadline, though: `get_opts`'s body is a full
object/range with no size cap, and `get_ranges`'s reassembled total is unbounded too
(`client.rs:613-624`; the server caps only range *count*, not bytes) — a multi-megabyte read from a
healthy, fully-warm cache would routinely exceed any deadline short enough to be useful, the same
failure mode this plan already rejects for a flat `get_opts` deadline. The fix bounds the *stall*, not
the total, at the one place each path already awaits its next chunk:

- **`get_opts`**: `full_stream_with_fallback` (`client.rs:309-350`) already distinguishes "before the
  first chunk" (safe to retry against `direct`) from "after" (must end the stream — see "Why the body
  phase can't share the tight budget" above). Wrapping each of its `first.next()` awaits in
  `tokio::time::timeout(body_stall_timeout, ...)` catches a stall in either position: a pre-first-chunk
  stall reports `record_unresponsive` and falls back exactly like a stream error does today; a
  post-first-chunk stall reports `record_unresponsive` too (the breaker cares about liveness regardless
  of whether the query can recover) but still just ends the stream, since retrying would re-emit
  already-delivered bytes. Both synthesize an `object_store::Error::Generic` the same way the existing
  transport-error mapping does (`client.rs:112-116`), so the rest of the match arms are unchanged.
- **`get_ranges`**: `pull_exact` (`client.rs:419-443`) already does `stream.next().await` in a loop to
  reassemble length-prefixed frames — the one place in the whole path that awaits a chunk. Wrapping that
  single await in `tokio::time::timeout(body_stall_timeout, ...)` gives exactly "resets on every chunk,
  never fires on cumulative size," via a new `RangesReadError::Stalled` variant that `read_framed_ranges`
  propagates the same way it already propagates `Transport` — directly unit-testable, no server needed.
  `send_ranges` still wraps just `req.send()` in `header_timeout` (catching connect-level and
  memory-permit-wait stalls before headers) before driving `read_framed_ranges` to completion, and still
  reports `record_responsive` only once the whole framed body has been read successfully — reporting it
  at `send()` would call the cache healthy before the part that actually fails has even started. A
  `send()` timeout, `RangesReadError::Transport`, or the new `RangesReadError::Stalled` all report
  `unresponsive` and fall back through the same existing `get_ranges` fallback arms — no healthy cache
  stalls for 3s with zero bytes, so this stays a meaningful liveness signal, not a routine one.

Both wrap reads on the **one** `reqwest::Client` the store already builds — there is no second client.
`ClientBuilder::read_timeout` would give the same "resets on every read" semantics, but only by building
a dedicated `/ranges` client, which would split `/obj` and `/ranges` onto separate hyper connection pools
(more cold connects against a `connect_timeout` this plan is simultaneously tightening to 50ms) purely to
reach a mechanism (`tokio::time::timeout` around a single `.next()`) that already lives in-crate at
`pull_exact`. Reusing that mechanism directly also lets it cover `get_opts`'s body too, which a
`/ranges`-only client could never have done — so `read_timeout` is dropped entirely, and
`body_stall_timeout` is consumed only as a plain `Duration` passed into these `tokio::time::timeout`
calls.

**Why `header_timeout` can't double as `body_stall_timeout`, stated once so it isn't re-litigated:** a
phase with no origin work (the pre-header wait on `/ranges`/`head_size`/`prefetch`) must be bounded
tightly (500ms) to give the breaker a meaningful fast-fail signal at all; a phase that may include an
origin fetch — `get_opts`'s `send()`, and every body chunk on both `get_opts` and `get_ranges` — has a
legitimate tail (~575ms observed) that a 500ms budget would routinely misclassify as a failure. A single
shared constant can't satisfy both: tight enough to fail fast where nothing can stall on the origin, and
loose enough to tolerate a legitimate cold origin fetch wherever one can land. Hence two named constants,
`header_timeout` (500ms, no-origin-work phases only) and `body_stall_timeout` (3s, every phase that may
touch the origin), each validated against the phase it actually bounds — not against which request path
it happens to sit on.

### The breaker

New module `rust/object-cache/src/circuit_breaker.rs`, deliberately free of any cache-specific naming
so it stays reusable (and so `imetric!`'s literal-name requirement doesn't leak into it): state
transitions are *returned* to the caller, which owns the metrics and logs.

```rust
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Consecutive unresponsive requests that trip the breaker. `0` disables it.
    pub failure_threshold: u32,     // 5
    pub initial_cooldown: Duration, // 100ms
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
    /// `admit_at` opens a fresh window (a genuine trip or a newly-admitted
    /// probe) — so it bounds each *window* to at most one unearned doubling,
    /// not the whole trip: stale reports still trickling in after a probe
    /// has been admitted can add one more per window they land in. See the
    /// note below the pseudocode.
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
    /// No response: header-phase timeout, connect failure, or transport error.
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
            open_until = Some(now + cooldown);
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
            open_until = Some(now + cooldown);
            backoff_applied = true;
            Backoff { cooldown }
        }
    } else {
        consecutive += 1;
        if failure_threshold > 0 && consecutive >= failure_threshold {
            cooldown = initial_cooldown;
            open_until = Some(now + cooldown);
            backoff_applied = false;                     // opening trip itself doesn't count as the probe's backoff
            Opened { cooldown }
        } else { None }
    }
```

The flag makes the cooldown doubling idempotent *within* a single open window: the *first* unresponsive
report that arrives while `open_until.is_some()` doubles the cooldown (whether it's a genuine probe
failure or a stale pre-trip request still draining), but every other report arriving before the next
probe is admitted is a no-op. That caps each window at one unearned doubling — but it is not a
whole-trip guarantee: `admit_at` resets `backoff_applied` to `false` every time it admits a probe, so if
stale pre-trip requests are still draining after a probe has already been admitted (they were in flight
for up to the full ~500ms header-timeout, spread across the trip and the first couple of cooldowns),
each newly-opened window can absorb one more unearned doubling. Under continuous traffic the realistic
bound is roughly one doubling per cooldown window that falls inside the pre-trip drain window (a
handful — e.g. 100 → 200 → 400ms — not exactly one), not "exactly one doubling regardless of how many
requests were in flight." It is still far short of the up-to-nine stale reports compounding straight to
`max_cooldown` that no idempotence check would allow at all.

Two properties worth calling out:

- **Probe admission extends `open_until` immediately**, instead of tracking a "probe in flight" flag.
  That makes the breaker robust to a dropped/cancelled probe future (a cancelled query) — the worst
  case is one extra probe per cooldown, never a circuit stuck open forever waiting for a result that
  will never be reported.
- **A plain `std::sync::Mutex`**, never held across an `await`. Contention is a few nanoseconds
  against a network round trip; a lock-free atomics encoding would need a CAS loop for the same
  semantics and no measurable gain.

Cooldown sequence after a trip: 100ms → 200 → 400 → … → 30s (capped), reset to 100ms on recovery.

### Wiring into `CacheClientStore`

**One send helper** — every cache request already goes through `.send()` at five sites
(`get_range_stream`, `get_full_stream`, `head_size`, `get_ranges`, `prefetch`). Route them all through
one method so the timeout-wrap and the breaker bookkeeping exist exactly once, with each call site
choosing which budget applies:

```rust
/// Send a request to the cache, bounding *time-to-headers* with `budget` (not
/// body streaming — see below) and reporting the outcome to the circuit
/// breaker. Any HTTP response counts as responsive, whatever its status: a
/// 404 or 500 means the server is alive and answering cheaply, which is not
/// the failure mode this gate exists for. Callers pick `budget`:
/// `header_timeout` where nothing before headers touches the origin
/// (`head_size`, `prefetch`, `get_ranges`), `body_stall_timeout` where it does
/// (`get_full_stream`/`get_range_stream` — see "Where the header phase spends
/// its time").
async fn send(&self, req: reqwest::RequestBuilder, what: &str, budget: Duration) -> Result<reqwest::Response> {
    match tokio::time::timeout(budget, req.send()).await {
        Ok(Ok(resp)) => { self.report(self.breaker.record_responsive()); Ok(resp) }
        Ok(Err(e)) => { self.report_unresponsive(what); Err(e).with_context(|| format!("sending {what} to cache")) }
        Err(_) => { self.report_unresponsive(what); Err(anyhow!("cache {what} did not respond within {budget:?}")) }
    }
}
```

`get_opts`'s data-fetching paths report a second, later signal too: `full_stream_with_fallback` takes an
`Arc<CircuitBreaker>` (cloned from `self.breaker`, since the `'static` stream can't borrow `&self`) and
`body_stall_timeout`, and wraps each `first.next()` await in `tokio::time::timeout`. A stall reports
`record_unresponsive` the same way `send`'s timeout arm does, whether it lands before the first chunk
(falls back, exactly like today's stream-error arm) or after (still ends the stream, but the breaker
still hears about it — see "Two timeout phases, not one"). This is the only body-phase breaker feedback
`get_opts` needs; the `send()` timeout above already covers its header-equivalent phase.

`get_ranges` is the one caller that does **not** use `send` as-is: as established above, its response
headers arrive before the origin fetch, so reporting `record_responsive` the moment `send()` resolves
would tell the breaker the cache is healthy before the part that actually fails has even started. It
instead calls a `send_ranges` variant that wraps `req.send()` in `tokio::time::timeout(header_timeout,
...)` (catching connect-level/permit-wait stalls before headers — the same client, no dedicated
`/ranges` pool), then drives `read_framed_ranges` to completion, with `pull_exact`'s `stream.next()`
wrapped in `tokio::time::timeout(body_stall_timeout, ...)` (3s vs. 500ms; see "Two timeout phases, not
one" for why the two can't be the same budget) so a stall is caught without ever bounding cumulative body
size. `send_ranges` returns a typed `Result<Vec<Bytes>, RangesSendError>` that keeps the existing failure
arms distinct (non-2xx status, `RangesReadError::Transport`, `RangesReadError::Truncated`) plus the new
`RangesReadError::Stalled` (the `body_stall_timeout` elapsing) instead of folding them into one; only a
`send()` timeout/connect error or `RangesReadError::{Transport,Stalled}` reports `record_unresponsive` —
non-2xx status and `Truncated` still received a full HTTP response, so per the "any HTTP response counts
as responsive" rule they report `record_responsive` instead, and `Truncated` keeps its `warn!` (a
protocol violation from our own cache, not a health signal). All three failure kinds are exactly as safe
to abort as dropping `send()`'s future, since `read_framed_ranges` exposes nothing to the caller until it
fully resolves, and every arm still falls back through the same existing `get_ranges` fallback path. The
whole call is otherwise bounded only by the unchanged 15s total deadline, so a healthy multi-megabyte read
is never aborted on size alone — the `body_stall_timeout`-wrapped `pull_exact` read is what actually
feeds the breaker, sized so it only trips on a stall well past any observed cold-cache origin-fetch tail.

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
    pub connect_timeout: Duration,    // 50ms
    pub header_timeout: Duration,     // 500ms — phases with no origin work (`/ranges`/`head_size`/
                                       // `prefetch` headers)
    pub body_stall_timeout: Duration, // 3s — every phase that may touch the origin: `get_opts`'s
                                       // `send()` (commit-before-stream) plus the inter-chunk stall on
                                       // both `get_opts` and `get_ranges` bodies; see "Two timeout
                                       // phases, not one" for why this can't share header_timeout
    pub total_timeout: Duration,      // 15s (unchanged)
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

`initial_cooldown` (100ms) and `max_cooldown` (30s) stay named constants backing
`CircuitBreakerConfig::default()`, not env vars, mirroring `L1_TOTAL_FETCH_PERMITS` /
`L1_DEMAND_RESERVED_FETCH_PERMITS` in `l1_store.rs:36-40`: that tier exposes only its one
operator-meaningful knob (`MICROMEGAS_OBJECT_CACHE_L1_MB`) and keeps its secondary tuning private. Here
too, one knob is operator-meaningful (the threshold — `..._BREAKER_THRESHOLD=0` is the escape hatch if
the breaker misbehaves in production) while the cooldown schedule is a tuning-not-a-decision surface;
tests construct `CircuitBreakerConfig` directly (zero/huge cooldowns) rather than needing the env path.

`body_stall_timeout` (3s) is likewise a named constant backing `CacheClientConfig::default()`, not an
env var: it's validated directly against the sampled `range_cache_origin_get_ms` tail (~575ms) and the
unchanged 15s total deadline, not against a deployment property an operator would need to retune the
way `connect_timeout` might for TLS/cross-zone. It bounds every phase that may touch the origin —
`get_opts`'s `send()` and both bodies' inter-chunk stalls, not just `/ranges` — so one calibration
covers all of them. Tests construct `CacheClientConfig` directly when they need a different value.

Three operator overrides, parsed with the `warn`-and-default pattern from
`l1_store.rs:49-57` (factored into a small private `env_millis`/`env_u32` helper rather than repeated
three times):

| Variable | Default | Effect |
|---|---|---|
| `MICROMEGAS_OBJECT_CACHE_CLIENT_TIMEOUT_MS` | `500` | `header_timeout` — the no-origin-work header budget (`/ranges`/`head_size`/`prefetch`) |
| `MICROMEGAS_OBJECT_CACHE_CLIENT_CONNECT_TIMEOUT_MS` | `50` | Connect budget (raise for TLS / cross-zone) |
| `MICROMEGAS_OBJECT_CACHE_CLIENT_BREAKER_THRESHOLD` | `5` | Consecutive failures to trip; `0` disables the breaker |

### Metrics and logging

State transitions log once (not once per request), so an outage doesn't flood:

| Metric | Emitted on | Log |
|---|---|---|
| `range_cache_client_unresponsive` | Header-phase timeout, body-stall timeout, connect, or transport error | `debug!` |
| `range_cache_client_circuit_opened` | `Transition::Opened` | `warn!` with the cooldown |
| `range_cache_client_circuit_closed` | `Transition::Closed` | `info!` |
| `range_cache_client_circuit_bypassed` | Each read/prefetch that skipped the cache | `debug!` |

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
   `connect_timeout(config.connect_timeout)` and `timeout(config.total_timeout)` — no second client.
6. Add the `send(&self, req, what, budget: Duration)` helper and route `head_size`/`prefetch` through it
   with `header_timeout`, and `get_range_stream`/`get_full_stream` through it with `body_stall_timeout`
   (see "Where the header phase spends its time" for why the latter two use the larger budget). Wrap
   `full_stream_with_fallback`'s `first.next()` awaits in `tokio::time::timeout(body_stall_timeout, ...)`
   too, reporting `record_unresponsive` on a stall in either the pre- or post-first-chunk position (only
   the pre-first-chunk one also falls back). Add the `send_ranges` variant for `get_ranges`: wrap
   `req.send()` in `header_timeout` (catching connect/permit-wait stalls before headers, on the same
   client), then drive `read_framed_ranges` to completion, with `pull_exact`'s `stream.next()` wrapped in
   `tokio::time::timeout(body_stall_timeout, ...)` and a new `RangesReadError::Stalled` variant for the
   elapsed case. Report `record_responsive` only once the framed body fully resolves. `send_ranges`
   returns a typed `Result<Vec<Bytes>, RangesSendError>` that keeps the four failure arms distinct —
   non-2xx status, `RangesReadError::Transport`, `RangesReadError::Stalled`, `RangesReadError::Truncated`
   — rather than folding them into one `Result<Vec<Bytes>>`; `get_ranges` keeps matching all four (plus
   success) with their current `debug!`/`warn!` logs, and only the timeout/connect/`Transport`/`Stalled`
   arms report `record_unresponsive`.

### Phase 3 — the gate

7. Add `direct_get_opts` / `direct_get_ranges` (+ the free `direct_get_opts_with_metrics`) and
   collapse the six existing fallback blocks onto them.
8. Add the `Admission::Bypass` gate to `get_opts` (after the precondition short-circuit),
   `get_ranges`, and `prefetch`.
9. Add the transition reporting helper (`fn report(&self, t: Transition)`) with the metrics/logs
   above.

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
| `mkdocs/docs/admin/object-cache.md` | Client env vars, fast-fail section, 4 new metrics, amended `fallback` row |
| `mkdocs/docs/architecture/caching.md` | Note the fast-fail gate on the fallback edge |
| `CHANGELOG.md` | Entry under Unreleased → **Caching:** |

No changes needed in `rust/ingestion/src/data_lake_connection.rs`: `new` keeps its signature and
`from_env` reads the overrides.

## Trade-offs

- **500ms on phases with no origin work, 3s wherever the origin fetch can land.** As specified
  literally, a flat 500ms `ClientBuilder::timeout` would abort every read whose body takes longer than
  half a second — most real reads — and mid-stream aborts aren't recoverable by the existing fallback.
  Bounding time-to-headers targets the same latency the issue measured, at the only point where
  abandoning is safe — except on `get_opts`'s data path, where commit-before-stream puts the origin
  fetch *inside* that same wait, so it uses `body_stall_timeout` (3s) instead of `header_timeout`
  (500ms) there, same as the inter-chunk stall on both bodies (see "Where the header phase spends its
  time"). The cost is a slower fast-fail specifically on `/obj` (3s, not 500ms) for a hard-down cache;
  the alternative — 500ms there too — would misclassify the ~575ms cold-cache tail as failures instead.
- **One `reqwest::Client`, not a second one per endpoint.** `tokio::time::timeout` around each
  `.next()`/`send()` gives the same "resets on every read" semantics as
  `ClientBuilder::read_timeout` without a second hyper connection pool, so `/obj` and `/ranges` keep
  sharing connections even as `connect_timeout` gets tighter, and the same mechanism covers `get_opts`'s
  body too instead of being `/ranges`-only.
- **Any HTTP response counts as "responsive".** A 5xx-ing cache stays in circuit, because it answers
  cheaply and doesn't cause the resource exhaustion this gate exists to prevent; reads still fall back
  per-request as they do today. The gate is about *responsiveness*, not correctness.
- **A stale success can close the circuit early.** A request admitted before the trip, completing
  successfully after it, reports `Closed`. Tracking a probe epoch would prevent that, but a fresh
  response is genuine liveness evidence, and the cost of acting on it is one more 500ms detection
  cycle. Symmetrically, a stale failure while open can trigger a `Backoff` it didn't strictly earn —
  `backoff_applied` makes the doubling idempotent *within* an open window, capping it at one unearned
  doubling per window, but since `admit_at` re-arms the flag on every probe admission, staggered stale
  reports still trickling in across the first few cooldown windows of the drain period can each add one
  more unearned doubling — realistically a handful of doublings over the drain window, not exactly one,
  though still far short of every stale report compounding in turn. A probe-epoch/generation counter
  (tag each open window, ignore reports tagged with an older one) would close this gap outright at the
  cost of one more field; deferred here as a refinement, not required for correctness.
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
  - New subsection after **What gets cached** — "Failing fast when the cache is unresponsive":
    the two-phase budget, the trip condition, the backoff/probe schedule, and how to read the new
    metrics during an incident (`circuit_opened` once, `bypassed` climbing, `fallback` climbing with
    it, `circuit_closed` on recovery), plus the `..._BREAKER_THRESHOLD=0` escape hatch.
  - **Monitoring** table: the four new client metrics; amend the `range_cache_client_fallback` row to
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
  (verified by re-tripping and observing 100ms again, not 30s).
- `failure_threshold: 0` → always `Allow`, never opens.
- Cancelled probe (admit a `Probe`, never report) → next `admit_at` after the extended cooldown
  returns `Probe` again, i.e. no permanently-stuck circuit.

### Integration — `rust/object-cache-srv/tests/client_circuit_breaker_tests.rs`

An axum server whose handler increments an `AtomicUsize` and then parks on a
`tokio::sync::watch::Receiver::changed()` — a controllable hang, released by the test rather than by
elapsed time. Client built with `with_config` (`header_timeout`/`body_stall_timeout` both overridden to
short values, e.g. ~50-100ms, so the test is fast and doesn't wait out the 3s production default).
Following the precedent in `rust/object-cache-srv/tests/memory_budget_tests.rs:589-594`, the `direct`
store holds *different* bytes from the cache path, so cache-vs-direct service is observable in the
returned data.

- **Trip and bypass**: with a 60s cooldown, issue `threshold` sequential reads against the hung
  server — each returns the direct bytes (never an error). Snapshot the server's request counter,
  issue three more reads, assert the counter is unchanged: the cache was skipped without a
  connection. No sleeps.
- **Probe and recovery**: with a zero cooldown, trip the breaker, release the hang, then read again —
  the next read is admitted as a probe, returns the *cache's* bytes, and subsequent reads keep using
  the cache path (counter climbing again).
- **`get_ranges`** is gated too: same trip, then a `get_ranges` call returns correct direct data with
  no new server request.
- **`get_opts` slow-but-healthy cold read stays closed**: an `/obj` handler that sleeps (via the same
  `watch`-release mechanism, held past the test's short `header_timeout` override but well inside its
  `body_stall_timeout` override) before writing headers and the full body, called `failure_threshold`
  times. Asserts every read still returns the cache's bytes and the breaker never opens — confirming
  `get_range_stream`/`get_full_stream`'s `send()` is bounded by `body_stall_timeout`, not
  `header_timeout`, so a cold-but-healthy origin fetch (the ~575ms tail from "Where the header phase
  spends its time") isn't misreported as a failure. This is the regression test for issue: a cache that
  is merely slow on `/obj`, not down, must not trip the breaker.
- **`get_opts` mid-body stall**: an `/obj` handler that writes headers and one body chunk immediately,
  then hangs past `body_stall_timeout` (short override) before writing the rest. A single call surfaces
  as a stream error to the caller (unrecoverable once bytes have been delivered — see "Why the body
  phase can't share the tight budget") but still reports `record_unresponsive`; repeating it
  `failure_threshold` times trips the breaker (a subsequent `/obj` read is served from `direct` with no
  new server request), confirming `get_opts`'s body phase now feeds the breaker instead of parking
  silently for up to 15s.
- **`get_ranges` read-stall (`body_stall_timeout`)**: a `/ranges` handler that writes the framed
  length-prefix header, then hangs on the same `watch::Receiver::changed()` mechanism for longer than
  `body_stall_timeout` (built with a short override, e.g. ~50ms, so the test doesn't wait 3s real time)
  before the test releases it. Asserts a single such call still returns correct direct-store bytes (the
  stall is recoverable, not a hard failure), and that repeating it `failure_threshold` times trips the
  breaker (a subsequent read is served from `direct` with no new server request), confirming a
  `body_stall_timeout` stall reports `record_unresponsive` as the design intends.
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
cache path through `CacheClientStore::new` and must keep passing unchanged with the tighter default
budgets — a local loopback server responds well inside both (500ms `header_timeout`; 3s
`body_stall_timeout`). `python3 build/rust_ci.py` for the workspace.

## Open Questions

1. **Are 500ms/3s the right budgets?** The issue grounds 500ms in server-side
   `range_cache_origin_get_ms` (p50 36ms / p90 152ms / p99 311ms / max 575ms), which excludes the
   memory-permit wait and the network hop. That budget now only governs phases with no origin work
   (`/ranges`/`head_size`/`prefetch` headers); `get_opts`'s `send()` and the inter-chunk stall on both
   bodies instead use `body_stall_timeout` (3s), sized off the same distribution's tail plus margin —
   see "Where the header phase spends its time" and "Two timeout phases, not one" for why the split
   fell there rather than on request-path lines. The directly comparable signals are
   `object_cache_ttfb_ms` (server) and `range_cache_client_roundtrip_ms` (client) for the
   header-equivalent phases. Worth checking those percentiles — and the still-unmeasured inter-chunk
   gap distribution for multi-block reads — in production before merge; the env override on
   `header_timeout` means a wrong guess there is tunable rather than a redeploy, but `body_stall_timeout`
   is a compiled-in constant (see "Configuration"), so a wrong default for it needs a code change.
2. **Cold-cache tripping.** After a cache restart the whole working set is cold, so many *consecutive*
   header times can exceed 500ms and open the circuit. Demand traffic then bypasses with one probe
   per cooldown, up to the 30s cap — the cache can only rewarm from prefetch/write-time warming, and
   demand-driven warming stalls. Acceptable, or should the max cooldown be lower (5s), or should
   prefetch requests be exempt from the gate so a bypassed cache still warms?
