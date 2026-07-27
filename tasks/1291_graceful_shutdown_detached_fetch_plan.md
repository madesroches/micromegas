# object-cache Graceful Shutdown for Detached Fetch Tasks Plan

## Overview
Origin fetches in the object-cache service run as detached `tokio::spawn` tasks
(`spawn_run_fetch` in `fetch.rs`, and the HEAD fetch inside `RangeCache::size`
in `mod.rs`) so a disconnected client can never strand other joiners waiting on
the same coalesced run. At shutdown, `serve_axum_with_graceful_shutdown` only
drains axum's HTTP connections; it has no visibility into these detached
tasks, so when the grace period elapses and `main()` returns, the tokio
runtime drops any still-in-flight fetch task. That trips `FulfillGuard`'s
panic-path fallback and logs a misleading "(likely a panic)" warning even
though nothing panicked, and it wastes the in-flight origin GET — every
joiner gets a synthesized error and must refetch after restart. This plan (1)
makes the log message accurate by distinguishing a real panic from a
runtime-shutdown drop, and (2) has `FetchScheduler` track outstanding fetch
tasks so `main()` can wait for them to drain (bounded by the existing grace
period) before returning.

## Current State

### The misleading log
`FulfillGuard::drop` (`rust/object-cache/src/range_cache/scheduler.rs:320-337`)
fires whenever the task owning one or more in-flight entries exits without
calling `disarm()` — both on a genuine panic and when the runtime drops the
task future outright (shutdown). It can't currently tell the two apart:

```rust
impl Drop for FulfillGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        warn!(
            "fetch task exited without completing normally (likely a panic); \
             fulfilling {} in-flight entries with an error",
            self.entries.len()
        );
        ...
    }
}
```

`std::thread::panicking()` is `true` only while the current thread is
unwinding from a real panic; it is `false` when a task future is simply
dropped (e.g. the tokio runtime shutting down without polling it to
completion), so it's exactly the signal needed to pick the right message.

### The real shutdown gap
Two call sites spawn detached fetch tasks, each wrapped in a `FulfillGuard`
so a panic still unblocks joiners:
- `spawn_run_fetch` (`rust/object-cache/src/range_cache/fetch.rs:261-351`) —
  one coalesced block-run GET.
- `RangeCache::size`'s owner branch (`rust/object-cache/src/range_cache/mod.rs`,
  around line 240) — one origin HEAD.

Neither task's lifetime is tracked anywhere. `serve_axum_with_graceful_shutdown`
(`rust/public/src/servers/shutdown.rs:62-116`) only knows about axum's HTTP
connections:

```rust
tokio::select! {
    res = serve_future => { ... }
    _ = deadline => { warn!("grace period ... elapsed with work still in flight"); Ok(()) }
}
```

Once this returns and `object_cache_srv.rs`'s `main()` (the function body,
`rust/object-cache-srv/src/object_cache_srv.rs:33-210`) returns, the
`#[micromegas_main]`-provided tokio runtime is torn down and every detached
fetch task in flight (permit acquired, origin GET underway) is dropped —
discarding real origin bandwidth/latency and forcing every joiner (including
ones from a fresh connection after restart) to refetch.

### Precedent: `ShutdownFanout` used directly by a binary
`flight_sql_server.rs:248-317` already constructs `ShutdownFanout` itself
(rather than going through `serve_axum_with_graceful_shutdown`) and
subscribes to it three times — once for the gRPC serve future, once for an
optional health-check sidecar, once for its own grace-period deadline — all
racing the same signal via `tokio::select!`. This is the pattern to reuse for
object-cache-srv's second drain (fetch tasks), run after the axum drain
completes and bounded by what's left of the same grace deadline (see Design
§3 for why this must be sequential, not concurrent).

## Design

### 1. Accurate shutdown-vs-panic log
Change `FulfillGuard::drop` to branch on `std::thread::panicking()`:

```rust
impl Drop for FulfillGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let n = self.entries.len();
        if std::thread::panicking() {
            warn!("fetch task panicked; fulfilling {n} in-flight entries with an error");
        } else {
            warn!(
                "cache service shutting down; abandoning {n} in-flight fetch entries \
                 (joiners see a synthesized error and must refetch after restart)"
            );
        }
        let msg = if std::thread::panicking() {
            "fetch task panicked before producing a result"
        } else {
            "fetch task was abandoned (cache service shutting down)"
        };
        for (key, entry) in &self.entries {
            entry.fulfill(Err(Arc::new(anyhow!(msg))));
            self.scheduler.remove_entry(key);
        }
    }
}
```

This is a pure diagnostics change — behavior for joiners (a synthesized
error, `remove_entry`) is unchanged either way.

### 2. Track outstanding fetch tasks
Add a task counter + `Notify` to `FetchScheduler`, mirroring the existing
non-lost-wakeup pattern already used in this file for `any_entry_promoted`
(construct the `Notified` future, re-check state, *then* await it):

```rust
pub(super) struct FetchScheduler {
    ...
    outstanding_tasks: AtomicUsize,
    drained: Notify,
}
```

A small RAII guard, held by each detached task for its whole body, tracks
task lifetime independent of `FulfillGuard` (which tracks *entry
fulfillment*, not task lifetime — the two guards serve different purposes and
both must survive a panicking OR shutdown-dropped task):

```rust
pub(super) struct FetchTaskGuard(Arc<FetchScheduler>);

impl FetchScheduler {
    pub(super) fn track_task(scheduler: &Arc<FetchScheduler>) -> FetchTaskGuard {
        scheduler.outstanding_tasks.fetch_add(1, Ordering::AcqRel);
        FetchTaskGuard(scheduler.clone())
    }

    pub(super) fn outstanding_tasks(&self) -> usize {
        self.outstanding_tasks.load(Ordering::Acquire)
    }

    /// Resolves once no detached fetch task is outstanding. Race-free the
    /// same way `any_entry_promoted` is: the `Notified` future is created
    /// and the count re-checked before awaiting it, so a `notify_waiters()`
    /// between the first check and the await is never missed.
    pub(super) async fn wait_drained(&self) {
        loop {
            if self.outstanding_tasks() == 0 {
                return;
            }
            let notified = self.drained.notified();
            if self.outstanding_tasks() == 0 {
                return;
            }
            notified.await;
        }
    }
}

impl Drop for FetchTaskGuard {
    fn drop(&mut self) {
        if self.0.outstanding_tasks.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.drained.notify_waiters();
        }
    }
}
```

Because `FetchTaskGuard` is a plain local variable inside the spawned async
block, its `Drop` runs whether the task finishes normally, panics, *or* is
dropped without being polled to completion (the shutdown case this issue is
about) — so the outstanding count is accurate in all three cases without any
special-casing.

Wire it into both spawn sites. `tokio::spawn` returns before its future is
ever polled, and the counter must not have a window where a queued-but-not-
yet-polled task is invisible to `wait_drained()` — so `track_task` is called
*before* `tokio::spawn`, synchronously incrementing the counter, and the
resulting guard is then moved into the async block (declared before
`FulfillGuard` so it's the last thing dropped, keeping the "outstanding"
window a superset of the "not yet fulfilled" window):

```rust
// fetch.rs: spawn_run_fetch
let task_guard = FetchScheduler::track_task(&cache.scheduler);
tokio::spawn(async move {
    let _task_guard = task_guard;
    let guard = FulfillGuard::new(...);
    ...
});
```
```rust
// mod.rs: RangeCache::size, owner branch
let task_guard = FetchScheduler::track_task(&scheduler);
tokio::spawn(async move {
    let _task_guard = task_guard;
    let guard = FulfillGuard::new(...);
    ...
});
```

Expose on `RangeCache` (`mod.rs`), alongside the other saturation-sampler
accessors:

```rust
/// Number of detached fetch tasks (coalesced block runs + `size()` HEADs)
/// currently in flight to origin, for the saturation sampler and shutdown
/// logging.
pub fn outstanding_fetch_tasks(&self) -> usize {
    self.scheduler.outstanding_tasks()
}

/// Resolves once every detached fetch task has finished. Used by graceful
/// shutdown to wait for in-flight origin GETs instead of letting the tokio
/// runtime drop them.
pub async fn wait_for_fetch_tasks_drain(&self) {
    self.scheduler.wait_drained().await;
}
```

### 3. Stop new prefetch intake, then wait for drain in `main()`, bounded by the grace period
Axum handlers are not the only source of detached fetch tasks: the prefetch
queue worker (`prefetch_queue.rs`) is a second one, and it is never stopped
today. `spawn_prefetch_worker`'s consumer only exits once its channel
closes, but nothing ever closes it — the saturation sampler
(`saturation_monitor.rs`) holds a `prefetch_tx` clone for the entire process
lifetime, and every `AppState` clone (one per in-flight request) holds
another. At the default queue depth (4096) and worker concurrency (8), a
shutdown that only drains axum and the fetch-task counter could otherwise
spend the whole grace period on prefetch warming nobody is waiting on.
Shutdown must therefore stop *both* producers — axum handlers and the
prefetch worker — from admitting new work before it waits for
`outstanding_fetch_tasks()` to reach zero.

**Stop new prefetch intake.** Thread the shutdown signal into both
`spawn_prefetch_worker` and `spawn_saturation_monitor`. The worker wraps its
receiver in `.take_until(shutdown)`, so it stops pulling *new* items the
moment the signal fires but still lets any `warm_item` call it already
started run to completion (its underlying fetch tasks are tracked the same
as demand's via `FetchTaskGuard`); the sampler exits its loop on the same
signal, which drops its own `prefetch_tx` clone instead of holding it
forever:

```rust
pub fn spawn_prefetch_worker(
    cache: RangeCache,
    queue_capacity: usize,
    worker_concurrency: usize,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> (mpsc::Sender<PrefetchItem>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<PrefetchItem>(queue_capacity);
    let handle = tokio::spawn(async move {
        let block_size = cache.block_size();
        ReceiverStream::new(rx)
            .take_until(shutdown)
            .for_each_concurrent(worker_concurrency, |item| {
                let cache = cache.clone();
                async move { warm_item(&cache, item, block_size).await }
            })
            .await;
    });
    (tx, handle)
}
```

```rust
pub fn spawn_saturation_monitor(
    cache: RangeCache,
    mem_permits: Arc<Semaphore>,
    memory_budget_mb: u32,
    prefetch_tx: mpsc::Sender<PrefetchItem>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tokio::pin!(shutdown);
        let mut networks = Networks::new_with_refreshed_list();
        let mut prev_disk_stats: Option<BackendDiskStats> = None;
        loop {
            tokio::select! {
                () = tokio::time::sleep(SAMPLE_INTERVAL) => {
                    sample_once(&cache, &mem_permits, memory_budget_mb, &prefetch_tx,
                                &mut networks, &mut prev_disk_stats, SAMPLE_INTERVAL.as_secs_f64());
                }
                () = &mut shutdown => {
                    return; // drops this task's `prefetch_tx` clone
                }
            }
        }
    })
}
```

This does not wait for the prefetch queue's backlog to empty — a full
queue at shutdown is simply abandoned, exactly as it would be today; it only
stops *new* items from being admitted, so the fetch-task drain below isn't
chasing an ever-refilling queue. `Priority::Prefetch` and `Priority::Demand`
fetch tasks that are already running are **not** distinguished by the
drain: both are tracked by the same `outstanding_tasks` counter and are
equally drained if they finish before the deadline, or equally abandoned
(and logged) if they don't.

**Sequence the drains against one shared deadline.** A concurrent
`tokio::join!` of the axum drain and the fetch-task drain does not deliver
the intended guarantee: axum handlers are the *producers* of demand fetch
tasks (`RangeCache::size`'s HEAD, and `stream_demand_windows`'s lazy,
`buffered(2)` per-window `spawn_run_fetch` calls made from inside the
streamed response body), so a slow client mid-stream can have zero
outstanding fetch tasks at the instant shutdown begins — `wait_drained()`
would resolve immediately — and then spawn more after that, which the
runtime would still drop at teardown. The fetch drain must therefore start
only *after* axum's own drain has actually finished, sharing what's left of
the same grace budget rather than getting a second full `grace` window
bolted on afterward:

```rust
use micromegas::servers::shutdown::{ShutdownFanout, serve_axum_with_graceful_shutdown, wait_for_sigterm};
use std::sync::OnceLock;
use std::time::Instant;
...
let grace = args.common.grace();

// Recorded by the shutdown future itself so both drains below can measure
// the same deadline (`signal_at + grace`) without a second subscriber race.
let signal_at: Arc<OnceLock<Instant>> = Arc::new(OnceLock::new());
let signal_at2 = signal_at.clone();
let fanout = ShutdownFanout::new(async move {
    wait_for_sigterm().await;
    let _ = signal_at2.set(Instant::now());
});

let (prefetch_tx, _prefetch_worker) = spawn_prefetch_worker(
    cache.clone(),
    args.prefetch_queue_capacity,
    args.prefetch_worker_concurrency,
    fanout.subscribe(),
);

let state = AppState::new(cache, allowed_prefixes, args.memory_budget_mb, prefetch_tx);

let _saturation_monitor = saturation_monitor::spawn_saturation_monitor(
    state.cache.clone(),
    state.mem_permits.clone(),
    state.memory_budget_mb,
    state.prefetch_tx.clone(),
    fanout.subscribe(),
);

// Taken here -- between `AppState::new` and `.with_state(state)`, the same
// gap the saturation-monitor clone above already uses -- since `state` is
// moved into the router by `.with_state(state)` further down.
let cache_for_drain = state.cache.clone();

... // router construction, listener bind (unchanged)

let axum_res = serve_axum_with_graceful_shutdown(
    listener,
    app.into_make_service_with_connect_info::<SocketAddr>(),
    fanout.subscribe(),
    grace,
)
.await;

// axum's own drain can spawn fetch tasks up until the moment it returns, so
// only now is it safe to wait for the fetch-task count to reach zero.
let signal_instant = signal_at.get().copied().unwrap_or_else(Instant::now);
let remaining = grace.saturating_sub(signal_instant.elapsed());
tokio::select! {
    () = cache_for_drain.wait_for_fetch_tasks_drain() => {
        info!("origin fetch tasks drained");
    }
    () = tokio::time::sleep(remaining) => {
        let n = cache_for_drain.outstanding_fetch_tasks();
        if n > 0 {
            warn!(
                "grace period of {}s elapsed with {n} origin fetch task(s) \
                 still in flight; abandoning",
                grace.as_secs()
            );
        }
    }
}

axum_res?;
```

`AppState::cache` (`app_state.rs`) is already an `Arc`/`Clone`-cheap
`RangeCache`, so cloning it via `state.cache.clone()` between `AppState::new`
(`object_cache_srv.rs:144`) and `.with_state(state)` (`:180`) — the same gap
the existing saturation-monitor clone at `:153-154` already uses — is a
one-line change.

## Implementation Steps
1. **Accurate log** — `rust/object-cache/src/range_cache/scheduler.rs`:
   rewrite `FulfillGuard::drop` to branch on `std::thread::panicking()`.
2. **Task tracking** — `rust/object-cache/src/range_cache/scheduler.rs`: add
   `outstanding_tasks: AtomicUsize` + `drained: Notify` to `FetchScheduler`,
   `FetchTaskGuard`, `FetchScheduler::track_task`/`outstanding_tasks`/`wait_drained`.
3. **Wire the guard into both spawn sites** — `rust/object-cache/src/range_cache/fetch.rs`
   (`spawn_run_fetch`) and `rust/object-cache/src/range_cache/mod.rs`
   (`RangeCache::size`'s owner branch): construct `FetchTaskGuard` *before*
   `tokio::spawn`, then move it into the async block.
4. **Expose on `RangeCache`** — `rust/object-cache/src/range_cache/mod.rs`:
   `outstanding_fetch_tasks()`, `wait_for_fetch_tasks_drain()`.
5. **Stop new prefetch intake on shutdown** — `rust/object-cache-srv/src/prefetch_queue.rs`:
   add a `shutdown` future parameter to `spawn_prefetch_worker`, wrap the
   receiver in `.take_until(shutdown)`; `rust/object-cache-srv/src/saturation_monitor.rs`:
   add the same `shutdown` parameter to `spawn_saturation_monitor` and exit
   its sample loop on it (dropping its `prefetch_tx` clone).
6. **Wire shutdown in `main()`** — `rust/object-cache-srv/src/object_cache_srv.rs`:
   build `ShutdownFanout` directly (recording the signal instant), pass its
   `subscribe()` into the prefetch worker and saturation monitor, run the
   axum drain to completion, then run the fetch-task drain bounded by
   whatever remains of `grace` measured from the recorded signal instant.
7. **Saturation gauge** — `rust/object-cache-srv/src/saturation_monitor.rs`:
   emit `outstanding_fetch_tasks()` as a new `object_cache_outstanding_fetch_tasks`
   gauge alongside `inflight_len()` (already sampled), so a stuck drain is
   visible in telemetry, not just at shutdown; add the corresponding row to
   the Saturation table in `mkdocs/docs/admin/object-cache.md` and a
   `rust/object-cache-srv/tests/saturation_tests.rs` test driving `sample_once`
   directly, mirroring the existing per-gauge tests there (e.g. the
   `object_cache_ram_tier_entries` precedent, #1322).
8. **Tests** — see Testing Strategy below, including a new panic-on-`get_range`
   `ObjectStore` test double.
9. **Docs** — update `mkdocs/docs/admin/object-cache.md`,
   `mkdocs/docs/admin/service-lifecycle.md`, and
   `rust/object-cache-srv/README.md` to describe the second drain; see
   Documentation below.
10. **Changelog** — add a `## Unreleased` → `**Caching:**` bullet in
    `CHANGELOG.md` noting the grace period now also drains in-flight origin
    fetches, not just HTTP connections.
11. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and the
    object-cache / object-cache-srv test suites.

## Files to Modify
- `rust/object-cache/src/range_cache/scheduler.rs` — accurate panic/shutdown
  log; `FetchTaskGuard` + counter/`Notify` on `FetchScheduler`.
- `rust/object-cache/src/range_cache/fetch.rs` — construct + hold
  `FetchTaskGuard` around `spawn_run_fetch`.
- `rust/object-cache/src/range_cache/mod.rs` — construct + hold
  `FetchTaskGuard` around `size()`'s owner branch; expose
  `outstanding_fetch_tasks()` / `wait_for_fetch_tasks_drain()`.
- `rust/object-cache-srv/src/prefetch_queue.rs` — add a `shutdown` future
  parameter to `spawn_prefetch_worker`; `.take_until(shutdown)` on the
  receiver stream.
- `rust/object-cache-srv/src/saturation_monitor.rs` — add a `shutdown`
  parameter to `spawn_saturation_monitor`; new `object_cache_outstanding_fetch_tasks`
  gauge in `sample_once`.
- `rust/object-cache-srv/src/object_cache_srv.rs` — build `ShutdownFanout`
  directly with a recorded signal instant; wire it into the prefetch worker
  and saturation monitor; sequence the axum drain then the fetch drain
  against the shared deadline.
- `rust/object-cache/tests/range_cache_tests.rs` — drain/panic-distinction
  regression tests; new panic-on-`get_range` `ObjectStore` double.
- `rust/object-cache-srv/tests/saturation_tests.rs` — new
  `object_cache_outstanding_fetch_tasks` gauge test.
- `mkdocs/docs/admin/object-cache.md` — shutdown-behavior note on the grace
  period; new Saturation-table row.
- `mkdocs/docs/admin/service-lifecycle.md` — update the object cache's "What
  it drains on `SIGTERM`" row and the drain-algorithm description.
- `rust/object-cache-srv/README.md` — update the `--shutdown-grace-period-seconds`
  description.
- `CHANGELOG.md` — `**Caching:**` bullet under `## Unreleased`.

## Trade-offs
- **Counter + `Notify` vs. `tokio_util::task::TaskTracker`.** The issue
  suggests either. A raw counter + `Notify` needs no new dependency, mirrors
  a wakeup pattern already present in this file (`any_entry_promoted`), and
  is a closer fit: `TaskTracker` is built around wrapping the *spawned
  futures themselves* (`tracker.spawn(fut)`) and a `close()` step before
  `wait()`, which doesn't map cleanly onto per-entry `FulfillGuard`-style RAII
  already threaded through these tasks. Rejected in favor of the smaller,
  dependency-free primitive.
- **Concurrent drain (via `tokio::join!`) vs. sequential (drain HTTP, then
  fetches).** Sequential is required, not just simpler: axum handlers
  themselves spawn demand fetch tasks (`RangeCache::size`'s HEAD, and each
  streamed window's `spawn_run_fetch`), so a concurrent `tokio::join!` can
  observe `wait_for_fetch_tasks_drain()` resolve immediately — while a slow
  client is merely between windows — and then watch axum's drain spawn more
  fetch tasks afterward, which the runtime would still drop uncounted at
  teardown. Running the fetch drain only after axum's drain completes closes
  that window; it costs some of the shared grace budget in the worst case
  (slow HTTP drain *and* slow fetches), which the shared-deadline design
  below already accounts for.
- **Single shared deadline (via a recorded signal `Instant`) vs. two
  independent `tokio::time::sleep(grace)` timers.** Independent timers would
  effectively double the usable grace budget now that the drains are
  sequential (up to `grace` for axum, then a fresh `grace` for fetches), the
  opposite of the intended bound. A single `signal_at + grace` deadline,
  computed once from the shutdown future itself, keeps the total wall-clock
  budget at `grace` regardless of how it's split between the two drains, at
  the cost of one small `OnceLock` to pass the signal instant across the two
  awaits.
- **Not modifying `serve_axum_with_graceful_shutdown` itself.** It's shared
  by `ingestion.rs`, `analytics-web-srv/web_server.rs`, and
  `object_cache_srv.rs` itself (three callers total); the other two have no
  detached-task concept, so adding one there would be dead complexity for
  both of them. `flight_sql_server.rs` does not call this helper at all — it
  already constructs `ShutdownFanout` directly and composes its own extra
  drain concerns around it (see Precedent above), which is exactly the
  pattern object-cache-srv follows instead of modifying the shared helper.

## Documentation
- `mkdocs/docs/admin/object-cache.md:77` documents `--shutdown-grace-period-seconds`
  as "Seconds to drain before hard exit on `SIGTERM`" — update this row (or an
  adjacent note) to say the grace period now also covers draining in-flight
  origin fetches, not just HTTP connections. This doc already documents the
  same flag at `:48` (env var) and `:77` (flag), so both need the note.
- `mkdocs/docs/admin/service-lifecycle.md:18` lists what the object cache
  drains on `SIGTERM` as "In-flight range/object read requests" — update to
  also mention in-flight origin fetches, since that description becomes
  incomplete once the second drain exists. `:43-54` spells out the
  drain algorithm and the exact log strings (`drain completed`, `grace
  period of <N>s elapsed with work still in flight`); note there that the
  object cache's second drain logs its own distinct messages
  (`origin fetch tasks drained` / `grace period of <N>s elapsed with <n>
  origin fetch task(s) still in flight; abandoning`).
- `rust/object-cache-srv/README.md:78` documents the same
  `--shutdown-grace-period-seconds` flag/default — add the same note there.
- Add `outstanding_fetch_tasks` to the saturation gauge list (new
  `object_cache_outstanding_fetch_tasks` row in the Saturation table,
  `mkdocs/docs/admin/object-cache.md:247-266`) alongside the existing
  `inflight_len()`-derived gauge (`saturation_monitor.rs:75`).
- `mkdocs/docs/admin/monolith.md`, `ingestion.md`, and `maintenance.md` need
  no change — none of them run or front an object cache.

## Testing Strategy
- **Panic vs. shutdown log distinction** (`range_cache_tests.rs`), split into
  two halves since nothing public survives the shutdown-drop path (`InFlight`,
  `FulfillGuard`, and `FetchScheduler` are all `pub(super)`, and the joiner
  future dies with the runtime):
  - *Panic half*: drive a fetch that panics (the new panic-on-`get_range`
    `ObjectStore` test double from step 8) and assert on the returned `Err`'s
    text (via `reconstruct_shared_error`) — it must match the panic-branch
    message ("fetch task panicked before producing a result"), confirming
    `FulfillGuard::drop` took the panic branch.
  - *Shutdown-drop half*: this needs `micromegas_tracing::test_utils::init_in_memory_tracing()`
    (already used in `object-cache/tests/telemetry_tests.rs:11`) plus
    `micromegas_tracing::dispatch::flush_log_buffer()` and `#[serial]`
    (global sink; precedent `telemetry_tests.rs:71`), and a plain `#[test]`
    (not `#[tokio::test]`) that builds its own `tokio::runtime::Runtime`,
    spawns the fetch on it, then `drop(runtime)` before it completes to force
    a task-drop-without-poll-to-completion — dropping a `Runtime` from inside
    an async context panics (precedent `telemetry_tests.rs:70-74`,
    `rust/analytics/tests/async_span_tests.rs:38-60`). Assert the captured
    log contains the shutdown-branch message, since that log line is the
    only thing observable on this path.
- **Drain waits for outstanding tasks**: drive `RangeCache::prefetch_blocks`
  (public, and skips the `size()` HEAD call that `get_range`/`fetch_blocks`
  would otherwise trigger first — that HEAD is itself a tracked task whose
  guard can transiently overlap the block fetch's guard, making an `== 1`
  assertion racy) against a `CountingStore::with_gate` origin double
  (existing infrastructure, `range_cache_tests.rs:65`/`:91`, same pattern as
  `:613`/`:684`/`:751`/`:785`/`:836`) with an explicit `file_size` so no
  `size()` call happens at all. Poll with a `tokio::task::yield_now` loop
  (precedent `range_cache_tests.rs:620`) until `counting.get_range_count() >= 1`
  (the origin GET has actually started, so its `spawn_run_fetch` task guard
  is registered), then assert `cache.outstanding_fetch_tasks() >= 1` and that
  `wait_for_fetch_tasks_drain()` does not resolve yet (race against a short
  `tokio::time::timeout`); release the gate and assert the wait resolves and
  the count returns to 0.
- **Drain resolves immediately with nothing in flight**: `wait_for_fetch_tasks_drain()`
  on a fresh cache returns without blocking (bound with a short timeout to
  catch a regression that hangs forever).
- **`object_cache_srv.rs` integration** (manual/optional, no new test file):
  since `main()` isn't easily unit-testable, verify via
  `local_test_env/ai_scripts/start_services.py` in its default **split**
  mode (with MinIO up and `MICROMEGAS_OBJECT_STORE_URI` set, so the object
  cache actually starts — `start_object_cache` is only wired into
  `start_split_mode`; `--monolith` starts the monolith *instead*, and the
  monolith runs no in-process object cache), issuing a slow/streamed request
  against the cache, then sending SIGTERM to the `micromegas-object-cache-srv`
  PID and confirming via `/tmp/object_cache.log` that fetch tasks drain (or
  are abandoned with the new accurate wording) before the process exits, with
  no more spurious "(likely a panic)" during a clean shutdown.
- Full gate: `cargo fmt`, `cargo clippy --workspace -- -D warnings`,
  `cargo test -p micromegas-object-cache -p micromegas-object-cache-srv -p micromegas`.

## Open Questions
- Is 25s (the current default `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS`)
  still enough once fetch-task draining is added, or should the default be
  revisited given large coalesced runs (`max_coalesced_get_bytes`, default
  8 MiB) can take longer than typical HTTP handler drains? No evidence either
  way; flagging for the reviewer rather than guessing.
