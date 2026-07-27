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
object-cache-srv's second, independent drain (fetch tasks) running
concurrently with the axum drain.

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

Wire it into both spawn sites, held for the task's full body (declared before
`FulfillGuard` so it's the last thing dropped, keeping the "outstanding"
window a superset of the "not yet fulfilled" window):

```rust
// fetch.rs: spawn_run_fetch
tokio::spawn(async move {
    let _task_guard = FetchScheduler::track_task(&cache.scheduler);
    let guard = FulfillGuard::new(...);
    ...
});
```
```rust
// mod.rs: RangeCache::size, owner branch
tokio::spawn(async move {
    let _task_guard = FetchScheduler::track_task(&scheduler);
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

### 3. Wait for drain in `main()`, bounded by the grace period
`object_cache_srv.rs`'s `main()` currently calls
`serve_axum_with_graceful_shutdown` directly with `wait_for_sigterm()`.
Following the `flight_sql_server.rs` precedent, construct `ShutdownFanout`
itself and run the HTTP drain and the fetch-task drain **concurrently**,
each independently bounded by `grace` starting from the same shutdown signal
(not additive — the fetch drain doesn't wait for the HTTP drain to finish
first, since HTTP requests and in-flight origin fetches shut down
independently):

```rust
use micromegas::servers::shutdown::{ShutdownFanout, serve_axum_with_graceful_shutdown, wait_for_sigterm};
...
let grace = args.common.grace();
let fanout = ShutdownFanout::new(wait_for_sigterm());

let axum_fut = serve_axum_with_graceful_shutdown(
    listener,
    app.into_make_service_with_connect_info::<SocketAddr>(),
    fanout.subscribe(),
    grace,
);

let cache_for_drain = state.cache.clone();
let drain_fut = async move {
    fanout.subscribe().await;
    tokio::select! {
        () = cache_for_drain.wait_for_fetch_tasks_drain() => {
            info!("origin fetch tasks drained");
        }
        () = tokio::time::sleep(grace) => {
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
};

let (axum_res, ()) = tokio::join!(axum_fut, drain_fut);
axum_res?;
```

`AppState::cache` (`app_state.rs`) is already an `Arc`/`Clone`-cheap
`RangeCache`, so `state.cache.clone()` before it moves into `AppState::new`
is a one-line change.

## Implementation Steps
1. **Accurate log** — `rust/object-cache/src/range_cache/scheduler.rs`:
   rewrite `FulfillGuard::drop` to branch on `std::thread::panicking()`.
2. **Task tracking** — `rust/object-cache/src/range_cache/scheduler.rs`: add
   `outstanding_tasks: AtomicUsize` + `drained: Notify` to `FetchScheduler`,
   `FetchTaskGuard`, `FetchScheduler::track_task`/`outstanding_tasks`/`wait_drained`.
3. **Wire the guard into both spawn sites** —
   `rust/object-cache/src/range_cache/fetch.rs` (`spawn_run_fetch`) and
   `rust/object-cache/src/range_cache/mod.rs` (`RangeCache::size`'s owner
   branch).
4. **Expose on `RangeCache`** — `rust/object-cache/src/range_cache/mod.rs`:
   `outstanding_fetch_tasks()`, `wait_for_fetch_tasks_drain()`.
5. **Wire shutdown in `main()`** —
   `rust/object-cache-srv/src/object_cache_srv.rs`: build `ShutdownFanout`
   directly, run the axum drain and the fetch-drain concurrently via
   `tokio::join!`, both bounded by `grace`.
6. **Optional: saturation gauge** — `rust/object-cache-srv/src/saturation_monitor.rs`:
   emit `outstanding_fetch_tasks()` as a gauge alongside `inflight_len()` (already
   sampled), so a stuck drain is visible in telemetry, not just at shutdown.
7. **Tests** — see Testing Strategy below.
8. **Docs** — update `mkdocs/docs/admin/object-cache.md` if it documents
   shutdown behavior (check first; add a short note if so).
9. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and the
   object-cache / object-cache-srv test suites.

## Files to Modify
- `rust/object-cache/src/range_cache/scheduler.rs` — accurate panic/shutdown
  log; `FetchTaskGuard` + counter/`Notify` on `FetchScheduler`.
- `rust/object-cache/src/range_cache/fetch.rs` — hold `FetchTaskGuard` in
  `spawn_run_fetch`.
- `rust/object-cache/src/range_cache/mod.rs` — hold `FetchTaskGuard` in
  `size()`'s owner branch; expose `outstanding_fetch_tasks()` /
  `wait_for_fetch_tasks_drain()`.
- `rust/object-cache-srv/src/object_cache_srv.rs` — build `ShutdownFanout`
  directly; run axum drain + fetch drain concurrently.
- `rust/object-cache-srv/src/saturation_monitor.rs` — optional new gauge.
- `rust/object-cache/tests/range_cache_tests.rs` — drain/panic-distinction
  regression tests.
- `mkdocs/docs/admin/object-cache.md` — shutdown behavior note, if applicable.

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
  fetches).** Concurrent is correct here: an in-flight origin fetch has no
  relationship to axum connection draining, so making it wait its turn behind
  HTTP drain would waste part of the shared grace budget for no reason.
  Sequential would also be simpler but effectively halves the usable grace
  budget in the worst case (slow HTTP drain *and* slow fetches).
- **Independent grace deadlines vs. a single shared deadline threaded through
  both.** Two `tokio::time::sleep(grace)` timers started at the same signal
  produce practically the same deadline as one shared `Instant`, without
  changing `serve_axum_with_graceful_shutdown`'s existing signature (used by
  three other binaries) or `ShutdownFanout`'s API. Simpler to review, no
  churn to shared shutdown code.
- **Not modifying `serve_axum_with_graceful_shutdown` itself.** It's shared
  by `ingestion.rs`, `flight_sql_server.rs`, `maintenance.rs`, and
  `object_cache_srv.rs`; none of the others have a detached-task concept, so
  adding one there would be dead complexity for three of four callers.
  `flight_sql_server.rs` already demonstrates the alternative — construct
  `ShutdownFanout` in the binary and compose extra drain concerns around it —
  so object-cache-srv follows that precedent instead.

## Documentation
- `mkdocs/docs/admin/object-cache.md:77` documents `--shutdown-grace-period-seconds`
  as "Seconds to drain before hard exit on `SIGTERM`" — update this row (or an
  adjacent note) to say the grace period now also covers draining in-flight
  origin fetches, not just HTTP connections.
- If Implementation Step 6 (saturation gauge) is done, add
  `outstanding_fetch_tasks` to the saturation gauge list alongside the
  existing `inflight_len()`-derived gauge (`saturation_monitor.rs:75`).

## Testing Strategy
- **Panic vs. shutdown log distinction** (`range_cache_tests.rs`): drive a
  fetch that panics (e.g. an origin store test double that panics inside
  `get_range`) and assert the resulting joiner error message/log path differs
  from a task that's simply dropped (e.g. `tokio::runtime::Runtime` built
  locally, `spawn` the fetch, then `drop(runtime)` before it completes — the
  standard way to force a task-drop-without-poll-to-completion in a test).
- **Drain waits for outstanding tasks**: spawn a `RangeCache::fetch_blocks`
  demand call against an origin double that delays via a `Notify` the test
  controls; assert `cache.outstanding_fetch_tasks() == 1` while the delay is
  held, that `wait_for_fetch_tasks_drain()` does not resolve yet (race against
  a short `tokio::time::timeout`), then release the delay and assert the wait
  resolves and the count returns to 0.
- **Drain resolves immediately with nothing in flight**: `wait_for_fetch_tasks_drain()`
  on a fresh cache returns without blocking (bound with a short timeout to
  catch a regression that hangs forever).
- **`object_cache_srv.rs` integration** (manual/optional, no new test file):
  since `main()` isn't easily unit-testable, verify via
  `local_test_env/ai_scripts/start_services.py --monolith`, issuing a slow
  request, then sending SIGTERM and confirming via logs that fetch tasks
  drain before the process exits and that the shutdown log message matches
  the new, accurate wording (no more spurious "(likely a panic)" during a
  clean shutdown).
- Full gate: `cargo fmt`, `cargo clippy --workspace -- -D warnings`,
  `cargo test -p micromegas-object-cache -p micromegas-object-cache-srv -p micromegas`.

## Open Questions
- Should `outstanding_fetch_tasks()` also be sampled into the saturation
  monitor as a standing gauge (Implementation Step 6), or is shutdown-time
  logging sufficient? Leaning toward adding it since it's a one-line addition
  alongside the already-sampled `inflight_len()` and gives visibility into a
  stuck drain, not just its outcome at shutdown.
- Is 25s (the current default `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS`)
  still enough once fetch-task draining is added, or should the default be
  revisited given large coalesced runs (`max_coalesced_get_bytes`, default
  8 MiB) can take longer than typical HTTP handler drains? No evidence either
  way; flagging for the reviewer rather than guessing.
