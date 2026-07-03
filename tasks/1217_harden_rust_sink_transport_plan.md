# Harden Rust Telemetry Sink Transport Plan

## Overview
The Rust telemetry sink (`rust/telemetry-sink/src/http_event_sink.rs`,
`HttpEventSink`) is deliberately simple: a single FIFO `mpsc` channel, a
coarse "drop the whole block when the queue holds ≥16 items" policy, and eager
stream-metadata sends. The Unreal sink (`unreal/MicromegasTelemetrySink`), which
shares the same wire protocol, has grown a much more resilient transport:
priority queues, byte budgets with graded drop, in-flight request gating, lazy
stream metadata, and per-priority retry tuning. This plan ports those
transport-hardening ideas to the Rust side so it behaves well under
backpressure, network flakiness, and ingestion outages — **without any wire /
server changes**, and with defaults chosen so the low-overhead co-located case
stays essentially unchanged.

Scope is strictly *how closed blocks leave the process*. The recording core
(buffer sizes, block cutting, sampling) is out of scope.

## Current State

### Transport (`rust/telemetry-sink/src/http_event_sink.rs`)
- **Single FIFO queue.** `HttpEventSink` holds a `std::sync::mpsc::Sender<SinkEvent>`
  (line 76). A dedicated OS thread (`thread_proc`, line 551) hosts a tokio
  runtime and drains the receiver with `recv_timeout` (line 479). All events —
  process, stream, and every block type — share one channel and are handled
  strictly in arrival order (`handle_sink_event`, line 335).
- **Coarse count-based drop.** `queue_size: Arc<AtomicIsize>` (line 77) is
  incremented on `send` (line 149) and decremented on `recv`. `push_block`
  (line 264) drops a whole block when `queue_size >= max_queue_size` (line 275,
  default **16** from `lib.rs:115`). The in-code comment at line 276–277 already
  notes the starvation problem: a burst of thread blocks can crowd out logs and
  metrics because there is no per-type budget.
- **Eager stream metadata.** `on_init_*_stream` (lines 613, 623, 633, 643)
  immediately enqueue `SinkEvent::InitStream`, sending `insert_stream` even for
  streams that never emit a block.
- **Serial sends.** The worker `await`s each `push_block` / `push_stream` /
  `push_process` before handling the next event — at most one request in flight.
- **Retry.** `tokio_retry2` exponential backoff, two fixed profiles
  (`metadata_retry` ~10 attempts, `blocks_retry` ~3), with
  `Transient`/`Permanent` classification (lines 34–60, 288). This part is sound
  and is reused as-is.
- **Four near-identical `push_block` call sites** (lines 365–444), one per block
  type, differing only in the log message.

### Construction / config (`rust/telemetry-sink/src/lib.rs`)
- `HttpEventSink::new(url, max_queue_size, metadata_retry, blocks_retry, make_decorator)`
  (http_event_sink.rs:114). `max_queue_size` is a `TelemetryGuardBuilder` field
  (lib.rs:80) defaulting to `16` (lib.rs:115); it is **not** currently read from
  any env var. The only transport-related env var today is
  `MICROMEGAS_TELEMETRY_URL` (lib.rs:413).

### Reusable building blocks
- `TracingBlock::len_bytes()` (`tracing/src/event/block.rs:39`) gives the raw
  uncompressed queued size of a block — the natural byte-budget proxy (Unreal
  uses the analogous `GetEvents().GetSizeBytes()`).
- `EventBlock::stream_id` (`tracing/src/event/block.rs:7`) and
  `StreamInfo::stream_id` — the key for lazy stream-meta lookup.
- `StreamBlock` trait (`stream_block.rs:13`) is object-safe (`encode_bin(&self, &ProcessInfo)`),
  so all four block types can be stored uniformly as `Arc<dyn StreamBlock + Send + Sync>`.
- `imetric!` / `fmetric!` (`tracing/src/macros.rs:163,203`) for self-instrumented
  counters, already used from within this crate (`system_monitor.rs`).

### Reference: Unreal sink (`unreal/MicromegasTelemetrySink/Private/HttpEventSink.cpp`)
- `EUploadPriority { Metadata=0, Logs=1, Metrics=2, Traces=3 }` (HttpEventSink.h:28).
- Four MPSC queues drained strictly in priority order (`DrainOneItem`, cpp:249).
- Soft cap 128 MiB (drops Traces), hard cap 256 MiB (drops Logs/Metrics too),
  Metadata never dropped (`EnqueueWithPriority`, cpp:329). `DroppedUploads` counter.
- In-flight gate default 3 (`CVarMaxInFlightRequests`, cpp:53; `DrainOneItem`, cpp:251).
- Lazy stream meta via `PendingStreamMeta` map, flushed on the stream's first
  block (`StorePendingStreamMeta`/`FlushPendingStreamMeta`, cpp:366–383, 393).
- Per-priority retry: counts `{10,5,2,1}`, windows `{300,120,30,6}`s
  (cpp:36–37, `SendBinaryRequest`, cpp:411–413).

## Design

### Priority classes
Introduce a `UploadPriority` enum mirroring Unreal, used for queueing, drop
budgeting, and (Phase 4) retry selection:

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum UploadPriority {
    Metadata = 0, // insert_process, insert_stream — never dropped
    Logs = 1,
    Metrics = 2,
    Traces = 3,   // thread + image blocks
}
```

Mapping: process/stream → `Metadata`; log block → `Logs`; metrics block →
`Metrics`; thread and image blocks → `Traces` (matches Unreal, which routes
thread/net/image to `Traces`).

### Shared transport state
Replace the single `mpsc` channel with a shared, priority-aware queue. The
enqueue side (EventSink callbacks on arbitrary app threads) does classification,
byte accounting, and drop decisions; the worker drains strictly by priority.

```rust
struct QueuedItem {
    priority: UploadPriority,
    bytes: usize,          // len_bytes() for blocks; 0 for metadata
    payload: Payload,
}

enum Payload {
    Process(Arc<ProcessInfo>),
    Stream(Arc<StreamInfo>),
    Block(Arc<dyn StreamBlock + Send + Sync>),
}

struct SharedQueue {
    // one deque per priority, index by priority as usize
    queues: Mutex<[VecDeque<QueuedItem>; 4]>,
    queue_bytes: AtomicIsize,     // sum of QueuedItem.bytes currently queued
    queue_count: AtomicIsize,     // for is_busy()
    dropped: [AtomicU64; 4],      // dropped-block counter per priority
    // wakeup: worker waits on this; enqueue + request-completion signal it
    notify: (Mutex<()>, Condvar),
    shutdown: AtomicBool,
}
```

Rationale for a `Condvar` (vs. keeping `mpsc`): the worker must wake on *two*
independent events — a new enqueue and a freed in-flight slot — and must also
wake on a flush-monitor timeout. A `Mutex`+`Condvar` with `wait_timeout` covers
all three cleanly and preserves the existing `FlushMonitor` cadence
(`recv_timeout` → `wait_timeout`). Critical sections hold only the queue lock,
never across an `.await`.

`StreamBlock` gains `Send + Sync` supertraits (all impls are on plain data
blocks, so this is free) so `Arc<dyn StreamBlock + Send + Sync>` is storable.

### Drop policy (byte budgets, graded)
On enqueue, before pushing:

```
soft = max_queue_bytes         (default 128 MiB)
hard = max(hard_queue_bytes, soft)  (default 256 MiB)
b = queue_bytes.load()

if priority == Traces  && b >= soft { drop() }        // shed traces first
if priority != Metadata && b >= hard { drop() }        // shed logs/metrics at the ceiling
// Metadata is never dropped
```

`drop()` increments `dropped[priority]`, emits an `imetric!` (see below), and
returns. Otherwise charge `queue_bytes += item.bytes` and `queue_count += 1` *before*
pushing (matching the Unreal ordering note: charge before enqueue so a fast
worker dequeue can't drive the counters negative), push to
`queues[priority as usize]`, and signal the condvar. The worker de-charges on
dequeue.

The old count-based `max_queue_size` drop is removed; `queue_count` survives
only to back `is_busy()`.

### Worker drain loop (priority order + in-flight gate)
The worker keeps its dedicated thread + tokio runtime. The loop becomes:

```
loop {
    if shutdown { final_drain(); break }
    while in_flight < max_in_flight {
        match pop_highest_priority() {   // Metadata → Logs → Metrics → Traces
            Some(item) => {
                queue_bytes -= item.bytes; queue_count -= 1;
                in_flight += 1;
                tokio::spawn(send(item, permit)); // permit drops -> signal on completion
            }
            None => break,
        }
    }
    // wait for: new work, a freed slot, shutdown, or flush interval
    let timeout = flusher.time_to_flush_seconds();
    notify.wait_timeout(timeout);
    flusher.tick();
}
```

- **In-flight gating** uses a `tokio::sync::Semaphore` of `max_in_flight`
  permits; each spawned send holds a permit and signals the condvar on
  completion so the drainer re-runs. Default `max_in_flight = 3` (Unreal
  parity, `CVarMaxInFlightRequests` cpp:53), so a slow request no longer stalls
  the backlog. This is a behavior change from today's strictly-serial sends,
  chosen deliberately to match the Unreal transport. Because each send now runs
  in a `tokio::spawn`'d `Send + 'static` future that moves an
  `Arc<dyn RequestDecorator>`, the `RequestDecorator` trait must gain a `Sync`
  supertrait (`pub trait RequestDecorator: Send + Sync`, request_decorator.rs:53)
  so `Arc<dyn RequestDecorator>: Send`; both concrete impls
  (`ApiKeyRequestDecorator`, `OidcClientCredentialsDecorator`) are already
  `Sync`-compatible, so only the bound changes. Store/clone the decorator as
  `Arc<dyn RequestDecorator>` per spawned send.
- `send()` wraps the existing `push_process` / `push_stream` / `push_block`
  retry logic. The four duplicated `push_block` match arms (lines 365–444)
  collapse into a single call taking the `Arc<dyn StreamBlock ...>`.
- **Shutdown** sets `shutdown`, signals the condvar; the worker does a final
  drain (submit all remaining queued items, bypassing the gate as Unreal does
  in `Run()` cpp:284) then waits for in-flight sends to finish before returning.
  The existing `shutdown_complete` condvar / `on_shutdown` 5s wait is kept.

### Lazy stream metadata
Add a pending map to `HttpEventSink`:

```rust
pending_stream_meta: Mutex<HashMap<uuid::Uuid, Arc<StreamInfo>>>,
```

- `on_init_*_stream` → `pending_stream_meta.insert(stream_id, Arc::new(make_stream_info(...)))`.
  No enqueue. Idle streams cost one map entry and nothing on the wire.
- `on_process_*_block` → before enqueuing the block, `remove(block.stream_id)`;
  if present, enqueue it as a `Metadata`-priority `Payload::Stream` first, then
  enqueue the block. This enqueues `insert_stream` ahead of the first
  `insert_block` for that stream (submission order only; the server tolerates
  out-of-order arrival, matching Unreal). Subsequent blocks find no pending entry
  and skip straight to enqueue.

The `on_startup` process send stays eager (it's the connection primer and must
precede everything).

### Per-priority retry
Included in the first version (Unreal parity). Replace the two fixed
`metadata_retry` / `blocks_retry` profiles with a per-priority table indexed by
`UploadPriority`, mirroring Unreal's `RetryCountByPriority` /
`RetryWindowSecondsByPriority` (cpp:36–37):

| Priority | Retry count | Window (s) |
|---|---|---|
| Metadata | 10 | 300 |
| Logs | 5 | 120 |
| Metrics | 2 | 30 |
| Traces | 1 | 6 |

Represented as `[Take<ExponentialBackoff>; 4]` in the config (each cloned per
attempt, as the current code already clones its retry strategy). `send()`
selects the strategy by the item's priority. Note that in Unreal the window is
applied only as the total retry budget
(`FRetryTimeoutRelativeSecondsSetting(RetryWindow)`, cpp:423), *not* as a
socket/request timeout — despite the cpp:35 comment, no socket or request
timeout is set. On the Rust side, decide independently whether to also map the
window onto reqwest's per-request `.timeout()`; if so, document it as a
deliberate Rust addition rather than Unreal parity.

### Configuration
Replace the growing positional arg list (and the existing
`#[expect(clippy::too_many_arguments)]`) with a config struct — extensible per
the open/closed principle:

```rust
pub struct HttpSinkConfig {
    pub max_queue_bytes: usize,        // soft cap, default 128 MiB (Traces shed above)
    pub hard_queue_bytes: usize,       // hard ceiling, default 256 MiB (Logs/Metrics shed above)
    pub max_in_flight_requests: usize, // default 3 (Unreal parity)
    pub retry_by_priority: [Take<ExponentialBackoff>; 4], // per-priority profiles (see table)
}
```

`HttpEventSink::new(url, config, make_decorator)` — signature change is
approved. `TelemetryGuardBuilder` gains matching `with_*` setters plus env
fallbacks in `build()`, following the `MICROMEGAS_*` convention already used for
the URL. Env var names mirror the Unreal CVar spellings
(`telemetry.max_queue_bytes` etc.) as closely as env-var syntax allows:

| Env var | Field | Default |
|---|---|---|
| `MICROMEGAS_TELEMETRY_MAX_QUEUE_BYTES` | `max_queue_bytes` | 128 MiB |
| `MICROMEGAS_TELEMETRY_HARD_QUEUE_BYTES` | `hard_queue_bytes` | 256 MiB |
| `MICROMEGAS_TELEMETRY_MAX_IN_FLIGHT_REQUESTS` | `max_in_flight_requests` | 3 |

The legacy `max_queue_size` builder field/setter is removed (interface change is
approved; the intended entry point is `TelemetryGuardBuilder`). The 128/256 MiB
byte caps sit far above the old 16-block ceiling, so a healthy co-located
monolith drops nothing; the caps only bite under a real outage.

### Dropped-block observability
On every drop, emit `imetric!("telemetry_dropped_blocks", "count", 1)` tagged by
priority name (or per-priority named metrics, e.g.
`telemetry_dropped_traces`), replacing the silent `debug!`. Emitted
unconditionally, matching Unreal's `DroppedUploads` imetric (no flag gating —
Unreal parity). Kept minimal (a counter, not per-drop logging) and only on the
drop path.

## Implementation Steps

### Phase 1 — Priority queues + byte-budget drop (highest value)
1. Add `UploadPriority` enum and `Send + Sync` to the `StreamBlock` trait
   (`stream_block.rs`).
2. Add `QueuedItem` / `Payload` / `SharedQueue` in `http_event_sink.rs`; replace
   the `mpsc` sender/receiver and `AtomicIsize` queue_size with `SharedQueue`.
3. Implement priority enqueue with the graded byte-budget drop; charge/de-charge
   `queue_bytes`. Wire `is_busy()` to `queue_count`.
4. Rewrite the worker loop to pop by priority and `wait_timeout` on the condvar;
   collapse the four `push_block` arms into one `Payload::Block` path. Sends stay
   serial in this phase (the concurrency gate lands in Phase 4).
5. Preserve shutdown semantics (final drain + `shutdown_complete`).

### Phase 2 — Lazy stream metadata
6. Add `pending_stream_meta` map; change `on_init_*_stream` to store instead of
   enqueue; flush on first block in `on_process_*_block`.

### Phase 3 — Configuration + per-priority retry
7. Introduce `HttpSinkConfig` (incl. `retry_by_priority` table); change
   `HttpEventSink::new` signature; select retry strategy by priority in `send()`.
8. Add `TelemetryGuardBuilder` setters + `MICROMEGAS_*` env fallbacks; remove the
   `max_queue_size` field. Update the construction site (`lib.rs:427`).
8a. Retire the two-profile retry API on `TelemetryGuardBuilder`: remove the public
   `with_telemetry_metadata_retry` / `with_telemetry_blocks_retry` setters
   (lib.rs:192–208) and their `telemetry_metadata_retry` /
   `telemetry_blocks_retry` fields (lib.rs:89–91). Their role is subsumed by the
   4-entry `retry_by_priority` table on `HttpSinkConfig`; expose a single
   `with_retry_by_priority` setter (defaulting to the per-priority table above)
   in their place. Rework `build()` (lib.rs:417–434) to populate the
   `retry_by_priority` table and pass `HttpSinkConfig` to `HttpEventSink::new`
   instead of the two positional retry strategies. (Removing the two setters is a
   breaking API change, accepted alongside the `HttpEventSink::new` signature
   change already approved.)

### Phase 4 — In-flight gating + dropped metric
9. Add the `tokio::sync::Semaphore` gate (default cap 3) and spawn sends
   concurrently; wake the drainer on completion. Change the `RequestDecorator`
   trait to `pub trait RequestDecorator: Send + Sync` (request_decorator.rs:53)
   and store/clone it as `Arc<dyn RequestDecorator>` per spawned send so the
   `Send + 'static` future can move it.
10. Emit `imetric!` dropped-block counters (by priority), unconditionally.

## Files to Modify
- `rust/telemetry-sink/src/http_event_sink.rs` — core rewrite (priority queue,
  worker loop, lazy-meta flush, config, gating, metric).
- `rust/telemetry-sink/src/stream_block.rs` — `Send + Sync` on `StreamBlock`.
- `rust/telemetry-sink/src/lib.rs` — `HttpSinkConfig`, builder setters, env
  fallbacks, construction site.
- `rust/telemetry-sink/tests/` — new transport unit tests (see Testing).
- `mkdocs/docs/getting-started.md` — document new env vars / behavior.
- `mkdocs/docs/admin/authentication.md` — update both `HttpEventSink::new`
  examples (lines ~339–345 and ~365–371) to the new `HttpSinkConfig`-based
  signature (they currently pass the removed positional `max_queue_size`,
  `metadata_retry`, `blocks_retry` args).

## Trade-offs
- **Condvar + shared queue vs. keeping `mpsc` as a wakeup with side queues.**
  A single `mpsc` can't naturally express priority ordering or wake on a freed
  in-flight slot. Reusing `mpsc` purely as a signal while holding real priority
  queues elsewhere is possible but duplicates the wakeup concern; a
  `Mutex`+`Condvar` around the queues is simpler and covers all three wake
  sources. Cost: manual charge/de-charge discipline (mitigated by charging
  before enqueue, matching the Unreal ordering rationale).
- **Byte proxy = `len_bytes()` (uncompressed) vs. encoded size.** Using the raw
  queued size avoids compressing at enqueue time (which would move CPU cost onto
  the app thread and defeat lazy encoding at send time). It over-counts vs. the
  compressed wire size, which is safe (drops slightly earlier). Matches Unreal.
- **`max_in_flight` default = 3 (Unreal parity).** A behavior change from
  today's strictly-serial sends, chosen to match the Unreal transport so a slow
  request can't stall the backlog. The trade-off is that strict cross-priority
  send ordering is relaxed once more than one request is in flight; set the env
  var to `1` to restore serial behavior.
- **Keeping the dedicated thread + `Runtime::new()`.** Minimal disruption;
  reuses the proven retry/decorator path. An alternative (spawn onto a caller's
  runtime) is out of scope and already flagged as a `TODO` in the code.
- **Drop count cap removed.** The old 16-block cap is meaningless when block
  sizes vary; byte budgets are the issue's explicit ask. `queue_count` is
  retained only for `is_busy()`.

## Documentation
- `mkdocs/docs/getting-started.md` — add the three `MICROMEGAS_TELEMETRY_*` env
  vars near the existing env-var setup block (`MICROMEGAS_TELEMETRY_URL`,
  line ~27) and a short note on the priority/byte-budget drop policy and lazy
  stream metadata. (The `native/index.md` page documents the C-ABI `MmConfig`
  struct fields, not the Rust sink env vars, so it is not the right target.)
- `mkdocs/docs/admin/authentication.md` — the two `HttpEventSink::new(...)`
  examples (lines ~339–345 and ~365–371) pass the old positional
  `max_queue_size`, `metadata_retry`, `blocks_retry` args; rewrite both to the
  new `HttpEventSink::new(url, config, make_decorator)` `HttpSinkConfig`
  signature so they stay accurate.
- Update the doc comment on `HttpEventSink::new` and `TelemetryGuardBuilder`
  setters (rustdoc) to describe the new knobs and defaults.

## Testing Strategy
- **Unit (crate `tests/` folder, per project convention):**
  - Priority ordering: enqueue a mix, assert a stub sink drains
    Metadata→Logs→Metrics→Traces.
  - Byte-budget drop: fill past soft cap → only Traces dropped; past hard cap →
    Logs/Metrics dropped, Metadata always survives; `dropped[]` counters
    increment correctly.
  - Lazy stream meta: init a stream with no blocks → no `insert_stream`; first
    block → exactly one `insert_stream` precedes the `insert_block`; second
    block → no extra `insert_stream`.
  - Shutdown final-drain: queued items are all submitted before the worker exits.
  - (These use a fake/local HTTP target or a mockable send path; consider
    extracting the send behind a small trait so the queue logic is testable
    without a real server — aligns with the existing `RequestDecorator` seam.)
- **Integration:** run `local_test_env/ai_scripts/start_services.py`, run an
  instrumented binary, verify `insert_process`/`insert_stream`/`insert_block`
  land and query via `micromegas-query`. Simulate an outage (stop ingestion) to
  confirm Traces shed first and Metadata/Logs survive to a bounded memory
  footprint, then recover.
- `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
  `python3 build/rust_ci.py`.

## Resolved Decisions
- **Interface change approved** — `HttpEventSink::new` takes `HttpSinkConfig`;
  the legacy `max_queue_size` field/setter is removed. `TelemetryGuardBuilder`
  is the intended entry point.
- **Follow Unreal when in doubt** — `max_in_flight` defaults to 3; dropped-block
  `imetric!` is emitted unconditionally (no feedback flag); env var names mirror
  the Unreal CVar spellings.
- **Per-priority retry is in the first version** (Phase 3), not deferred.
