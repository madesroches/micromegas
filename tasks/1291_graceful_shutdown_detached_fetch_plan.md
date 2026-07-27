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
joiner gets a synthesized error and must refetch after restart *unless the
fetched bytes are also recoverable from the disk-tier cache across the
restart*, which today they are not: `FoyerBackend::new_with_shards` never
sets a write policy, so foyer defaults to `WriteOnEviction` and a demand
fill's `put()` only lands in the RAM tier, and `main()` never calls
`FoyerBackend::close()`, so even a prefetch-hinted fill that *did* reach
foyer's write pipeline depends on foyer's own drop-time close attempt
(`impl Drop for Inner` spawns `close_inner` onto the `Spawner` captured at
store-build time — the runtime being torn down, since that happens to be
the same runtime here — `foyer-0.22.3/src/hybrid/cache.rs:347-366`) racing —
and typically losing to — that teardown before it can reach disk. This plan (1) makes the
log message accurate by
distinguishing a real panic from a runtime-shutdown drop, (2) has
`FetchScheduler` track outstanding fetch tasks so `main()` can wait for them
to drain (bounded by the existing grace period) before returning, and (3)
retains and closes the `FoyerBackend` after that drain so a successfully
drained fetch's bytes actually survive the restart instead of only avoiding
the synthesized-error path.

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
`rust/public/src/servers/flight_sql_server.rs:248-317` already constructs `ShutdownFanout` itself
(rather than going through `serve_axum_with_graceful_shutdown`) and
subscribes to it three times — once for the gRPC serve future and once for
its own grace-period deadline, both racing the same signal via
`tokio::select!` (`:294-315`), and once for an optional health-check sidecar
(`:271`), which is consumed by a separately spawned axum server outside that
`select!`, not by it. This is the pattern to reuse for
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
        let panicking = std::thread::panicking();
        if panicking {
            warn!("fetch task panicked; fulfilling {n} in-flight entries with an error");
        } else {
            warn!(
                "cache service shutting down; abandoning {n} in-flight fetch entries \
                 (joiners see a synthesized error and must refetch after restart)"
            );
        }
        let msg = if panicking {
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

    /// Resolves once no detached fetch task is outstanding. Race-free by
    /// construction: `Notify::notified()`'s docs guarantee that a
    /// `notify_waiters()` call is observed by a `Notified` future as long as
    /// that call happens after the `Notified` was created, regardless of
    /// whether `enable`/`poll` has run yet — so creating the `Notified`
    /// *before* re-checking the count (rather than after) means a
    /// `notify_waiters()` racing the check in between is never missed. This
    /// is a different guarantee than `any_entry_promoted`'s below, which
    /// relies on `notify_one`'s stored-permit semantics; `notify_waiters`
    /// stores no permit, so the ordering above — not a permit — is what
    /// makes this race-free.
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

**Stop new prefetch intake, in both the channel and each in-flight item's
window loop.** Thread the shutdown signal into `spawn_prefetch_worker`, and
also into `warm_item` itself. The worker wraps its receiver in
`.take_until(shutdown)` so it stops pulling *new* items from the channel the
moment the signal fires. Correctness of the fetch-task drain doesn't depend
on `warm_item` itself noticing shutdown: as described below, `main()` awaits
the prefetch worker's `JoinHandle` — which only resolves once every
in-flight `warm_item` call has fully finished — *before* trusting
`wait_for_fetch_tasks_drain()`, which is what actually closes the window
where an already-started `warm_item` call spawns one more `spawn_run_fetch`
task between windows. The `shutting_down: Arc<AtomicBool>` flag's purpose is
therefore latency, not correctness: `warm_item` pulls its `BlockWindows`
through `.buffered(WINDOW_CONCURRENCY)` with `WINDOW_CONCURRENCY = 1`
(`prefetch_queue.rs:21`), so without the flag an already-started `warm_item`
call would keep pulling one window at a time, uninterrupted, until its
entire window stream is exhausted — and the awaited `JoinHandle` (bounded by
the shared grace budget, see the "Sequence the drains" trade-off below)
would have to wait that long before the fetch-task drain and `foyer.close()`
ever run. `warm_item` therefore checks the flag before pulling its next
window so it stops between windows instead of running to completion (any
window already dispatched still runs to completion and is tracked via
`FetchTaskGuard` like demand fetches). Since the flag must be checked
synchronously from every concurrent `warm_item` call rather than awaited
once, `spawn_prefetch_worker` takes a *second* shutdown future (a second
`fanout.subscribe()` from the call site) driving a tiny detached task that
flips the flag — cheaper to clone into every `warm_item` call than the
shutdown future itself:

```rust
async fn warm_item(
    cache: &RangeCache,
    item: PrefetchItem,
    block_size: u64,
    shutting_down: &Arc<AtomicBool>,
) {
    let windows = lazy_windows(&item, block_size);
    let mut stream = stream::iter(windows)
        .map(|w| {
            let cache = cache.clone();
            let key = item.key.clone();
            let size = item.size;
            async move { cache.prefetch_blocks(&key, size, &w).await }
        })
        .buffered(WINDOW_CONCURRENCY);

    let mut warmed_any = false;
    loop {
        // Stop pulling the next window once shutdown has fired; a window
        // already dispatched into `stream`'s internal buffer still runs to
        // completion (it's already tracked by its own `FetchTaskGuard`).
        if shutting_down.load(Ordering::Acquire) {
            // Emit the metric on this path too -- today it always fires
            // once the loop exits, and a shutdown-return is exactly like
            // any other early exit for items whose windows already
            // succeeded.
            if warmed_any {
                imetric!("object_cache_prefetch_keys_warmed", "count", 1_u64);
            }
            return;
        }
        match stream.next().await {
            Some(Ok(())) => warmed_any = true,
            Some(Err(e)) => {
                imetric!("object_cache_prefetch_fill_error", "count", 1_u64);
                debug!("prefetch fill failed key={}: {e:?}", item.key);
                return;
            }
            None => break,
        }
    }
    if warmed_any {
        imetric!("object_cache_prefetch_keys_warmed", "count", 1_u64);
    }
}

pub fn spawn_prefetch_worker(
    cache: RangeCache,
    queue_capacity: usize,
    worker_concurrency: usize,
    shutdown: impl Future<Output = ()> + Send + 'static,
    window_shutdown: impl Future<Output = ()> + Send + 'static,
) -> (mpsc::Sender<PrefetchItem>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<PrefetchItem>(queue_capacity);
    let shutting_down = Arc::new(AtomicBool::new(false));
    let flag = shutting_down.clone();
    tokio::spawn(async move {
        window_shutdown.await;
        flag.store(true, Ordering::Release);
    });
    let handle = tokio::spawn(async move {
        let block_size = cache.block_size();
        ReceiverStream::new(rx)
            .take_until(shutdown)
            .for_each_concurrent(worker_concurrency, |item| {
                let cache = cache.clone();
                let shutting_down = shutting_down.clone();
                async move { warm_item(&cache, item, block_size, &shutting_down).await }
            })
            .await;
    });
    (tx, handle)
}
```

`window_shutdown` is a second, independent `fanout.subscribe()` from the
call site (see Design §3's `main()` snippet below) — `ShutdownFanout` exists
precisely to hand out cheap independent subscriptions, so this is one more
of the same call already made for the axum drain and the channel gate, not
a new primitive.

Note: `for_each_concurrent` drops its stream (sets it to `None`) the instant
`take_until(shutdown)` fires, which drops `rx` and therefore `prefetch_tx`'s
receiving end. Any prefetch POST that arrives during axum's own remaining
drain window (axum's drain hasn't necessarily finished yet — see the
"Sequence the drains" discussion below) will find the channel gone and hit
`handlers.rs:703`'s `error!("prefetch queue worker is gone")` plus a 503 —
expected on a graceful shutdown, not a bug, but worth downgrading that log
line for the shutdown case in a follow-up. This plan does not change
`handlers.rs`.

**The worker's `JoinHandle` must be awaited before the fetch-task drain
starts**, not discarded. `main()` currently binds it to `_prefetch_worker`
and never looks at it again; that leaves a window where `warm_item`'s
in-flight window fetch has been dispatched (tracked by `FetchTaskGuard`) but
`join_prefetch` hasn't yet reached the `remove_entry`/`guard.disarm()` tail
of the spawned fetch task, and the *next* window's future isn't even
constructed yet — so `outstanding_tasks` can legitimately read 0 between two
windows of the same still-running `warm_item` call. Awaiting
`_prefetch_worker` first (bounded by a sub-budget of what's left of `grace`,
not the whole remainder — see "Sequence the drains against one shared
deadline" below) establishes that the last prefetch producer has actually
exited before `wait_for_fetch_tasks_drain()` is trusted to mean "no more
origin GETs will be spawned."

`spawn_saturation_monitor` itself does **not** take a shutdown parameter.
The sample loop's only stated reason to take one would be to stop sampling
and drop its `prefetch_tx` clone early, but neither holds up: the drain
budget is short (default 25s grace) and `SAMPLE_INTERVAL` is 5s, so a
maxed-out drain is exactly the case where telemetry from the last few
samples is most useful, not something to cut off at the first sign of
shutdown; and once the worker exits via `take_until(shutdown)` (and the
window-loop flag above), nothing depends on this clone's closure anymore —
the channel only has to close for `for_each_concurrent` to stop pulling,
which `take_until` already guarantees independent of any other clone. The
sampler is therefore left unchanged and simply dies with the runtime, like
today:

```rust
pub fn spawn_saturation_monitor(
    cache: RangeCache,
    mem_permits: Arc<Semaphore>,
    memory_budget_mb: u32,
    prefetch_tx: mpsc::Sender<PrefetchItem>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut networks = Networks::new_with_refreshed_list();
        let mut prev_disk_stats: Option<BackendDiskStats> = None;
        loop {
            tokio::time::sleep(SAMPLE_INTERVAL).await;
            sample_once(&cache, &mem_permits, memory_budget_mb, &prefetch_tx,
                        &mut networks, &mut prev_disk_stats, SAMPLE_INTERVAL.as_secs_f64());
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

**Sequence the drains against one shared budget, split up front into four
fixed proportional slices — one per stage.** A concurrent `tokio::join!` of
the axum drain and the fetch-task drain does not deliver the intended
guarantee: axum handlers are the *producers* of demand fetch tasks
(`RangeCache::size`'s HEAD, and `stream_demand_windows`'s lazy, `buffered(2)`
per-window `spawn_run_fetch` calls made from inside the streamed response
body), so a slow client mid-stream can have zero outstanding fetch tasks at
the instant shutdown begins — `wait_drained()` would resolve immediately —
and then spawn more after that, which the runtime would still drop at
teardown. The fetch drain must therefore start only *after* axum's own drain
has actually finished, sharing the same `grace` budget rather than getting a
second full `grace` window bolted on afterward.

Four stages now share one `grace` budget: axum drain, prefetch-worker await,
fetch-task drain, `foyer.close()`. A naive way to share it is to have each
stage consume "whatever is left until a single recorded deadline" — but that
breaks down exactly on the busy-shutdown path these reservations exist for:
axum's own deadline arm always sleeps for the *full* duration it's given
before returning (`rust/public/src/servers/shutdown.rs:98-115`), so on that
branch a `remaining = grace - elapsed` computed right after
`serve_axum_with_graceful_shutdown` returns is already ~= 0 — starving every
stage after it, most importantly `foyer.close()` (Design §4's entire
purpose), last in line.

The fix is to stop deriving each stage's budget from "what's left of a
shared deadline" and instead give each of the four stages its own fixed,
proportional slice of `grace`, computed once up front and used directly to
bound that stage — not recomputed from elapsed time after earlier stages
run:

```rust
// A small absolute floor under each slice, so no stage's budget is ever
// `Duration::ZERO` regardless of the configured `grace` --
// `--shutdown-grace-period-seconds` (`rust/public/src/config.rs`) has no
// minimum-value validation, so a small or even zero `grace` is reachable in
// practice, and `tokio::time::timeout(Duration::ZERO, fut)` polls its
// future exactly once and fails immediately if it isn't already ready
// (`tokio-1.52.3/src/time/timeout.rs:211-222`) -- the "budget already
// exhausted before the stage got a fair chance" failure this design must
// avoid. The floor's cost is bounded and explicit: in the worst case
// (`grace` far below one second) total wall-clock time can exceed the
// nominal `grace` by at most `4 * MIN_STAGE_BUDGET` -- a few hundred
// milliseconds -- which is preferable to a shutdown sequence guaranteed to
// fail every stage on its first poll.
const MIN_STAGE_BUDGET: Duration = Duration::from_millis(250);

// An even, proportional split: each of the four sequential stages gets the
// same fixed quarter of `grace`. Because each slice is independent (not
// drawn from a pool the others can exhaust), one stage using its whole
// slice can no longer starve another, and the four slices still sum to
// `grace` (plus, at most, the floor's small overshoot for a pathologically
// small `grace`) -- the same total-wall-clock-bounded-by-`grace` property
// the shared-deadline design was reaching for, without its failure mode.
let stage_budget = (grace / 4).max(MIN_STAGE_BUDGET);
let axum_budget = stage_budget;
let prefetch_budget = stage_budget;
let fetch_budget = stage_budget;
let close_budget = stage_budget;
```

Because every stage now has its own always-nonzero budget, the per-stage
timeout warnings below stop being spurious: previously the "prefetch worker
did not exit" / "fetch tasks still in flight" warnings could fire on *every*
busy shutdown purely because `remaining` was already 0 before the stage got
a turn, regardless of how close the stage actually was to finishing. With a
real, fair `stage_budget`, a warning firing means the stage genuinely didn't
finish in its share of the time.

```rust
use micromegas::servers::shutdown::{ShutdownFanout, serve_axum_with_graceful_shutdown, wait_for_sigterm};
use std::time::Duration;
...
let grace = args.common.grace();
const MIN_STAGE_BUDGET: Duration = Duration::from_millis(250);
let stage_budget = (grace / 4).max(MIN_STAGE_BUDGET);
let axum_budget = stage_budget;
let prefetch_budget = stage_budget;
let fetch_budget = stage_budget;
let close_budget = stage_budget;

let fanout = ShutdownFanout::new(wait_for_sigterm());

let (prefetch_tx, mut prefetch_worker) = spawn_prefetch_worker(
    cache.clone(),
    args.prefetch_queue_capacity,
    args.prefetch_worker_concurrency,
    fanout.subscribe(),
    fanout.subscribe(), // window_shutdown: gates warm_item's per-window loop
);

let state = AppState::new(cache, allowed_prefixes, args.memory_budget_mb, prefetch_tx);

// No shutdown parameter: the sampler simply dies with the runtime (see
// Design §3 above for why taking one would buy nothing).
let _saturation_monitor = saturation_monitor::spawn_saturation_monitor(
    state.cache.clone(),
    state.mem_permits.clone(),
    state.memory_budget_mb,
    state.prefetch_tx.clone(),
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
    axum_budget, // bounded by its own slice, not the whole `grace`
)
.await;

// axum's own drain can spawn fetch tasks up until the moment it returns, so
// only now is it safe to wait for the fetch-task count to reach zero.

// The prefetch worker is the *other* fetch-task producer, and its
// `JoinHandle` must actually be awaited (not discarded, as `main()` does
// today) before `wait_for_fetch_tasks_drain()` is trusted: `warm_item`'s
// window loop can still be between iterations -- guard dropped, next
// window's future not yet spawned -- when `outstanding_tasks` transiently
// reads 0, so only the worker's own exit proves no further window will be
// dispatched. On timeout the handle is `abort()`-ed rather than merely
// left to drop: dropping a `JoinHandle` *detaches* the task instead of
// stopping it (`tokio-1.52.3/src/runtime/task/join.rs:18,35`), so a
// dropped-not-aborted handle would leave the worker free to dispatch
// further `spawn_run_fetch` windows during and after the fetch-task drain
// below -- exactly the precondition that drain depends on. `abort()` makes
// the precondition best-effort rather than exact on this branch: a window
// already dispatched before the abort takes effect keeps running, but it's
// tracked by its own `FetchTaskGuard` and is still caught by the
// fetch-task drain that follows.
match tokio::time::timeout(prefetch_budget, &mut prefetch_worker).await {
    Ok(Ok(())) => {}
    Ok(Err(e)) => warn!("prefetch worker task failed: {e:#}"),
    Err(_) => {
        warn!(
            "prefetch worker did not exit within its {:.1}s budget; aborting",
            prefetch_budget.as_secs_f64()
        );
        prefetch_worker.abort();
    }
}

tokio::select! {
    () = cache_for_drain.wait_for_fetch_tasks_drain() => {
        info!("origin fetch tasks drained");
    }
    () = tokio::time::sleep(fetch_budget) => {
        let n = cache_for_drain.outstanding_fetch_tasks();
        if n > 0 {
            warn!(
                "fetch-task drain's {:.1}s budget elapsed with {n} origin \
                 fetch task(s) still in flight; abandoning",
                fetch_budget.as_secs_f64()
            );
        }
    }
}
```

(The close call itself — bounded by its own `close_budget` slice — and the
final `axum_res?;` follow in Design §4 below.)

`AppState::cache` (`app_state.rs`) is already `Clone` and cheap — every
field is an `Arc` clone except `ns: String`, one small heap allocation, not
a concern on a shutdown path — so cloning it via `state.cache.clone()`
between `AppState::new`
(`object_cache_srv.rs:144`) and `.with_state(state)` (`:180`) — the same gap
the existing saturation-monitor clone at `:153-154` already uses — is a
one-line change.

### 4. Close the `FoyerBackend` after the fetch-task drain
The fetch-task drain above only prevents the *symptom* (a synthesized error
instead of a clean result) — it does not, by itself, make a drained fetch's
bytes recoverable across a restart, which is the benefit the Overview
claims. Two gaps must be closed for that to actually be true:

- **A handle to close.** `object_cache_srv.rs` currently does
  `RangeCache::new(origin_store, Arc::new(foyer), ...)`, so no reference to
  the `FoyerBackend` survives outside the `RangeCache`. Bind it to a local
  first — `let foyer = Arc::new(FoyerBackend::new_with_shards(...).await?);`
  — and pass that same `Arc` into `RangeCache::new`, so `main()` retains
  `foyer` alongside `cache` for use after the drain.
- **A close call, bounded by its own reserved slice.** After the fetch-task
  drain (Design §3) completes or times out, call `foyer.close()`, racing it
  against its own fixed `close_budget` slice from Design §3 — not against
  whatever happens to be left of `grace` at that point. This plan keeps
  foyer's defaults as-is: no `.with_flush_on_close(false)` and no
  `.with_policy(WriteOnInsertion)` change to `FoyerBackend::new_with_shards`.
  foyer 0.22.3 defaults `flush_on_close = true`
  (`foyer-0.22.3/src/hybrid/builder.rs:96-104`, `cache.rs:278-286`), and
  `close()` flushes the whole RAM tier by evicting every entry to zero and
  piping each one to storage (`cache.rs:320-328` ->
  `foyer-memory-0.22.3/src/raw.rs:657-675`) through the existing
  `WriteOnEviction` policy — so this close call, on its own, is already
  sufficient to make a demand fill's bytes reach disk; see below. That write
  still goes through the disk engine's normal (non-forced) admission path —
  `HybridCachePipe::flush` calls `store.enqueue(piece, false)`, unlike the
  prefetch path's `.force()` — so an entry can be silently not admitted if a
  device write throttle is active (`foyer-storage-0.22.3/src/engine/block/engine.rs:430`,
  `filter.rs:113-125`); this repo configures no throttle, so every entry is
  admitted today, but the dependency is worth noting.

```rust
// Set immediately before `close()`'s eviction-to-zero sweep so
// `RamEvictionListener::on_leave` (see below) can tell this flush apart
// from capacity-driven thrashing.
foyer.mark_shutting_down();
match tokio::time::timeout(close_budget, foyer.close()).await {
    Ok(Ok(())) => info!("foyer cache closed"),
    Ok(Err(e)) => warn!("foyer cache close failed: {e:#}"),
    Err(_) => warn!(
        "foyer cache close did not finish within its {:.1}s budget",
        close_budget.as_secs_f64()
    ),
}

axum_res?;
```

**Close-time RAM flush would otherwise poison the #1281 eviction gauges.**
`close()`'s eviction-to-zero sweep calls `listener.on_leave(Event::Evict,
...)` for every flushed entry (`foyer-memory-0.22.3/src/raw.rs:657-675`).
`RamEvictionListener::on_leave` (`foyer_backend.rs:203-230`) emits
`object_cache_ram_tier_eviction_count` for all four `Event` reasons, but
gates `object_cache_ram_tier_eviction_age_ms` on `Event::Evict` specifically
— that's the one it treats as the capacity-driven thrashing signal (#1281).
Left unguarded, every clean shutdown would emit both gauges for the *entire*
RAM tier (default `--ram-mb` 512, `cli.rs:24`) — indistinguishable from real
capacity thrashing. Making that check possible from `on_leave` requires more
than a field on `FoyerBackend`: `RamEvictionListener` is a separate struct
built and moved into the foyer builder (as an `Arc<dyn EventListener>`,
`foyer_backend.rs:314,317`) *before* `FoyerBackend` itself is constructed
(`:335-339`), and neither holds a reference to the other, so a plain
`FoyerBackend` field would be unreachable from `on_leave`. Instead, create a
`shutting_down: Arc<AtomicBool>` before the listener is built, clone it into
both `RamEvictionListener` and `FoyerBackend` — mirroring the existing
`tags: Arc<EvictionTagTable>` sharing between the same two constructs
(`:313-314`/`:336`) — and have `on_leave` check it and skip emission when
set, the same short-circuit shape already used for the `is_prefetch`
phantom-record case just above it. Because `main()` lives in a different
crate, the flag also needs a public setter — `FoyerBackend::mark_shutting_down()`
— called immediately before `foyer.close()` (see the snippet above).
`mkdocs/docs/admin/object-cache.md` notes that these two gauges go quiet
during the final close step rather than spiking.

This is what actually delivers the Overview's claim: a demand fetch's bytes
are only recoverable after restart once this close step exists. A demand
fill's `put()` only lands in the RAM tier during normal operation (the
`WriteOnEviction` policy is unchanged by this plan), but that's irrelevant
to `close()`'s own flush: `close()`'s default eviction-to-zero sweep pipes
*every* RAM entry — demand fills included — through the write pipeline
regardless of how (or whether) it got written during normal operation. No
change to the write policy is needed, and none is made.

**Post-close cache access by orphaned callers is unaddressed.** After
`close()`, `get` still serves from RAM and disk, but a `put` lands in RAM
and is then silently dropped when the pipe tries to enqueue it
(`foyer-storage-0.22.3/src/engine/block/engine.rs:570-574` logs
`warn!("cannot enqueue new entry after closed")`); no error reaches the
caller. This can only happen from a connection axum's own drain didn't
manage to close in time; no code change is made for it here (see the
matching Trade-offs note).

## Implementation Steps
1. **Accurate log** — `rust/object-cache/src/range_cache/scheduler.rs`:
   rewrite `FulfillGuard::drop` to branch on `std::thread::panicking()`.
2. **Task tracking** — `rust/object-cache/src/range_cache/scheduler.rs`: add
   `outstanding_tasks: AtomicUsize` + `drained: Notify` to `FetchScheduler`,
   `FetchTaskGuard`, `FetchScheduler::track_task`/`outstanding_tasks`/`wait_drained`
   (add the `std::sync::atomic::AtomicUsize` import, currently absent,
   `scheduler.rs:2`).
3. **Wire the guard into both spawn sites** — `rust/object-cache/src/range_cache/fetch.rs`
   (`spawn_run_fetch`) and `rust/object-cache/src/range_cache/mod.rs`
   (`RangeCache::size`'s owner branch): construct `FetchTaskGuard` *before*
   `tokio::spawn`, then move it into the async block.
4. **Expose on `RangeCache`** — `rust/object-cache/src/range_cache/mod.rs`:
   `outstanding_fetch_tasks()`, `wait_for_fetch_tasks_drain()`.
5. **Stop new prefetch intake on shutdown** — `rust/object-cache-srv/src/prefetch_queue.rs`:
   add `shutdown` and `window_shutdown` future parameters to
   `spawn_prefetch_worker`; wrap the receiver in `.take_until(shutdown)` and
   have `window_shutdown` drive a small detached task that flips a shared
   `shutting_down: Arc<AtomicBool>`, which `warm_item` (now taking that flag)
   checks before pulling each window's next item from its `buffered` stream.
   Add the new imports this needs: `std::future::Future`, `std::sync::Arc`,
   and `std::sync::atomic::{AtomicBool, Ordering}` (none currently present,
   `prefetch_queue.rs:1-10`). Update the three now-inaccurate doc comments
   this touches: `prefetch_queue.rs:107-112`'s `spawn_prefetch_worker` doc
   (the handle no longer resolves only on channel closure) and
   `saturation_monitor.rs:160-163` (only the clause claiming the sampler's
   handle parallels the prefetch worker's is now stale — the sampler's own
   handle still needn't be awaited, per Design §3). `app_state.rs:28-31`'s
   `prefetch_tx` doc comment needs no change: it says the channel "closes
   only once the server shuts down," never that closure is what *stops* the
   worker, so it stays true verbatim. Also update the six existing call sites in
   `rust/object-cache-srv/tests/prefetch_tests.rs` (`:180`, `:247`, `:294`,
   `:447`, `:534`, `:597`), threading a never-firing
   `std::future::pending::<()>()` for both new parameters so the tests keep
   compiling and behaving as before.
6. **Wire shutdown in `main()`** — split into a new lib module
   `rust/object-cache-srv/src/shutdown_sequence.rs` (exported from
   `lib.rs`) plus its call site in
   `rust/object-cache-srv/src/object_cache_srv.rs`, so the sequence is
   callable from the integration-test crate (see Testing Strategy's
   "Prefetch worker actually stops on shutdown" and B5 in design review —
   `main()` itself isn't unit-testable, mirroring why `cli::validate_write_tuning`
   was split out, `cli.rs:155-172`). The new module computes the four
   `stage_budget` slices from `grace` (Design §3) and exposes the sequenced
   drain (prefetch-worker await with `abort()` on timeout, fetch-task drain,
   `foyer.close()`) as one async function taking the pieces `main()` already
   has (`prefetch_worker`, `cache_for_drain`, `foyer`, `stage_budget`).
   `main()` builds `ShutdownFanout` directly, passes two independent
   `subscribe()`s into the prefetch worker (`shutdown` and `window_shutdown`)
   and one into axum's drain (bounded by `axum_budget`, not the whole
   `grace`); retains the `Arc<FoyerBackend>` bound before it's passed into
   `RangeCache::new`; and after axum's drain completes, calls the new
   module's sequenced-drain function, which calls
   `FoyerBackend::mark_shutting_down()` immediately before `foyer.close()`
   and bounds that close call by its own `close_budget` slice, not by
   `grace` minus elapsed time (Design §3/§4).
7. **Saturation gauge** — `rust/object-cache-srv/src/saturation_monitor.rs`:
   emit `outstanding_fetch_tasks()` as a new `object_cache_outstanding_fetch_tasks`
   gauge alongside `inflight_len()` (already sampled), so a stuck drain is
   visible in telemetry, not just at shutdown; add the corresponding row to
   the Saturation table in `mkdocs/docs/admin/object-cache.md` and a
   `rust/object-cache-srv/tests/saturation_tests.rs` test driving `sample_once`
   directly, mirroring the existing per-gauge tests there (e.g. the
   `object_cache_ram_tier_entries` precedent, #1322).
8. **Tests** — see Testing Strategy below, including a new panic-on-`get_opts`
   `ObjectStore` test double (panicking on the ranged-GET branch), a new
   `log_blocks`/`LogStringEvent` text-extraction helper for the
   shutdown-drop test — no in-repo precedent exists (unlike the
   metrics-block helpers already in `saturation_tests.rs`), so it must be
   written from scratch — a `Notify`-backed shutdown case in
   `prefetch_tests.rs` (see Testing Strategy) exercising `take_until`, the
   window flag, and the deadline arithmetic end to end, mirroring `#1037`'s
   `graceful_shutdown_tests.rs` pattern rather than only threading
   `std::future::pending::<()>()` into the other five call sites — and a new
   `foyer_backend_tests.rs` case proving Design §4's load-bearing claim: a
   single `FillHint::Demand` `put()` with no RAM eviction pressure, then
   `close()`, then a fresh `FoyerBackend` reopened over the same directory,
   asserting `get()` still hits. Every existing put->close->get case in that
   file forces eviction first (a tiny `ram_bytes`) or uses
   `FillHint::Prefetch`'s `.force()` writer, so none of them today exercises
   a RAM-resident demand entry surviving *only* because `close()` flushed it.
9. **Docs** — update `mkdocs/docs/admin/object-cache.md`,
   `mkdocs/docs/admin/service-lifecycle.md`,
   `mkdocs/docs/architecture/caching.md`, and
   `rust/object-cache-srv/README.md` to describe the second drain; see
   Documentation below.
10. **Changelog** — append a bullet to the existing `## Unreleased` →
    `**Caching:**` subsection (`CHANGELOG.md:32`) noting the grace period
    now also drains in-flight origin fetches, not just HTTP connections,
    ending the bullet with `(#1291)` per this file's existing convention
    (every `**Caching:**` bullet ends with an issue reference, e.g. `:33`,
    `:37`).
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
- `rust/object-cache-srv/src/prefetch_queue.rs` — add `shutdown` and
  `window_shutdown` future parameters to `spawn_prefetch_worker`;
  `.take_until(shutdown)` on the receiver stream; a `shutting_down:
  Arc<AtomicBool>` flag flipped by `window_shutdown` and checked by
  `warm_item` before each window; update the `spawn_prefetch_worker` doc
  comment (`:107-112`) to stop claiming the handle resolves only on channel
  closure.
- `rust/object-cache-srv/src/saturation_monitor.rs` — new
  `object_cache_outstanding_fetch_tasks` gauge in `sample_once` (no
  `shutdown` parameter — see Design §3/Step 5 for why); narrow the
  `spawn_saturation_monitor` doc comment update (`:160-163`) to just the
  clause claiming the sampler's handle parallels the prefetch worker's —
  the sampler's own handle still needn't be awaited, so that half of the
  comment stays true.
- `rust/object-cache-srv/src/lib.rs` — export the new `shutdown_sequence`
  module so the integration-test crate can call it directly (Design
  §3/Step 6).
- `rust/object-cache-srv/src/shutdown_sequence.rs` (new) — computes the four
  `stage_budget` slices from `grace` and the sequenced drain (prefetch-worker
  await + `abort()` on timeout, fetch-task drain, `foyer.close()`), as a
  testable async function (Design §3/§4, Step 6).
- `rust/object-cache-srv/src/object_cache_srv.rs` — build `ShutdownFanout`
  directly; wire two independent subscriptions into the prefetch worker
  (`shutdown`, `window_shutdown`) and one into axum's drain (bounded by its
  own `axum_budget` slice, not the whole `grace`); retain the
  `Arc<FoyerBackend>` bound before it's passed into `RangeCache::new` so
  `close()` has something to call it on (Design §4); call into
  `shutdown_sequence` after axum's drain completes.
- `rust/object-cache/src/foyer_backend.rs` — add a `shutting_down:
  Arc<AtomicBool>`, created before `RamEvictionListener` is built and cloned
  into both it and `FoyerBackend` (mirroring the existing `tags:
  Arc<EvictionTagTable>` sharing), plus a public `FoyerBackend::mark_shutting_down()`
  setter called from `shutdown_sequence` immediately before `close()`;
  `RamEvictionListener::on_leave` checks the flag and skips emission, so
  `close()`'s full-tier flush doesn't poison the #1281 eviction gauges
  (Design §4).
- `rust/object-cache/tests/range_cache_tests.rs` — drain/panic-distinction
  regression tests; new panic-on-`get_opts` `ObjectStore` double (panicking
  on the ranged-GET branch).
- `rust/object-cache/tests/foyer_backend_tests.rs` — new case: a
  `FillHint::Demand` `put()` with no RAM eviction pressure, `close()`, reopen
  a `FoyerBackend` over the same directory, assert `get()` hits (Design §4).
- `rust/object-cache-srv/tests/saturation_tests.rs` — new
  `object_cache_outstanding_fetch_tasks` gauge test.
- `rust/object-cache-srv/tests/prefetch_tests.rs` — update the six existing
  `spawn_prefetch_worker` call sites (`:180`, `:247`, `:294`, `:447`, `:534`,
  `:597`) to pass the two new parameters (a never-firing
  `std::future::pending::<()>()` for each); add a new `Notify`-backed
  shutdown case (see Testing Strategy).
- `mkdocs/docs/admin/object-cache.md` — shutdown-behavior note on the grace
  period; new Saturation-table row; note that the eviction gauges go quiet
  during the close-time flush.
- `mkdocs/docs/admin/service-lifecycle.md` — update the object cache's "What
  it drains on `SIGTERM`" row and the drain-algorithm description.
- `mkdocs/docs/architecture/caching.md` — L1-vs-L2 shutdown/drain
  clarification (see Documentation).
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
  that window on the branch where axum's drain actually finishes; it costs
  some of the shared grace budget in the worst case (slow HTTP drain *and*
  slow fetches), which the fixed-slice design below already accounts for.
  On the *deadline* branch the window narrows but does not fully close:
  axum 0.8.9 `tokio::spawn`s a task per accepted connection
  (`serve/mod.rs:389`), so dropping `serve_future` at the deadline kills only
  the accept loop — already-accepted connections survive as orphaned tasks
  and can still call `RangeCache::size`/`spawn_run_fetch` after the fetch
  drain has already started. This residual window is bounded, not
  vanishing: with fixed per-stage slices (Design §3), `prefetch_budget` and
  `fetch_budget` are each a real, non-negligible quarter of `grace`
  (unlike the earlier elapsed-time-based design, where the analogous window
  happened to shrink to `~= 0` on exactly this branch), so an orphaned
  connection has up to `prefetch_budget + fetch_budget` to dispatch a task
  that the fetch-task drain is still around to track. A task spawned *after*
  that combined window (once the drain has already timed out and `main()`
  has moved on to `foyer.close()`) is untracked and still lost at teardown —
  an accepted risk, since tracking orphaned axum connections themselves is
  out of scope for this plan.
- **25s default grace period, unchanged.** `rust/public/src/config.rs:9-21`
  is the single source of the default (`default_value = "25"`), already set
  deliberately against a 30s orchestrator termination window — Kubernetes
  `terminationGracePeriodSeconds` (`mkdocs/docs/admin/service-lifecycle.md:72`)
  and ECS `stopTimeout` (`:85`). `:92-95` prescribes an escape hatch (raise
  **both** the service grace period and the orchestrator window together)
  framed around high write latency (large blocks, slow object store). Two
  facts specific to this feature confirm 25s doesn't need revisiting for it:
  the cache client's own `CACHE_REQUEST_TIMEOUT` is 15s with
  fallback-to-direct-store (`object-cache/src/client.rs:19-25`), so no
  caller is listening past 15s regardless of how long the server-side drain
  runs; and there is no per-fetch timeout on the origin GET at all, so no
  finite grace could *guarantee* draining an 8 MiB coalesced run — which is
  exactly why "abandon and warn at the deadline" (rather than lengthening
  the default) is the right shape for this drain. Operators with unusually
  slow origins should use the existing escape hatch above; the same escape
  hatch generalizes to slow origin reads even though `:92-95` is framed
  around high write latency.
- **Fixed proportional per-stage slices vs. a single elapsed-time deadline
  vs. independent full-`grace` timers per stage.** Four timers each given
  the full `grace` would effectively quadruple the usable budget now that
  the stages are sequential, the opposite of the intended bound. A single
  shared deadline (one recorded signal `Instant`, each stage bounded by
  "whatever's left of `grace`") keeps the total at `grace`, but breaks down
  operationally: a stage's own bounded wait can itself consume the *entire*
  remaining budget merely by timing out (axum's deadline arm, in
  particular, always sleeps for the full duration it's given), leaving
  exactly 0 for every later stage on precisely the busy-shutdown path this
  plan targets — see Design §3. Fixed, independent slices computed once
  from `grace` (`stage_budget = (grace / 4).max(MIN_STAGE_BUDGET)`) avoid
  that failure mode: no stage can borrow against, or be starved by, another
  stage's budget, while the four slices still sum to `grace` (plus, at
  most, the floor's small overshoot for a pathologically small `grace`),
  preserving the same total-wall-clock-bounded-by-`grace` property the
  shared-deadline design was reaching for. The cost is that a stage which
  finishes early does not hand its unused time to a later stage — simpler,
  and the right trade given the alternative's correctness bug.
- **Post-close access from orphaned connections, left unhandled.** After
  `close()`, a `put` from a connection axum's own drain didn't finish
  closing in time lands in RAM and is then silently dropped when foyer
  tries to enqueue it to disk (`foyer-storage-0.22.3/src/engine/block/engine.rs:570-574`);
  no error reaches that caller. `get` is unaffected — it still serves from
  RAM and disk after `close()`. Handling this would mean either tracking
  and draining orphaned axum connections themselves (out of scope, see the
  "Concurrent drain" trade-off above) or having `FoyerBackend` reject
  `put`s once `shutting_down` is set, which is unnecessary extra plumbing
  for a case that just means the state ends up how it would have if the
  request had lost its race with shutdown a moment earlier. No code change
  is made for this.
- **Not modifying `serve_axum_with_graceful_shutdown` itself.** It's shared
  by `ingestion.rs`, `analytics-web-srv/web_server.rs`, and
  `object_cache_srv.rs` itself (three production callers); the other two have no
  detached-task concept, so adding one there would be dead complexity for
  both of them. `rust/public/src/servers/flight_sql_server.rs` does not call this helper at all — it
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
  `object_cache_outstanding_fetch_tasks` row in the two-column Saturation
  table, `mkdocs/docs/admin/object-cache.md:251-264`, next to the
  `object_cache_inflight_entries` row at `:255`) alongside the existing
  `inflight_len()`-derived gauge (`saturation_monitor.rs:75`).
- Note near `object-cache.md:209` (the `object_cache_promotion_count` row,
  which already cross-references `object_cache_ram_tier_eviction_*` in
  prose — the eviction gauges themselves have no row in either the counters
  or the Saturation table) that the close-time full-tier flush on a clean
  shutdown (Design §4) is intentionally excluded from these gauges (via the
  new `shutting_down` flag), so it never appears as a spike there — without
  this note, an operator could otherwise expect (and be confused not to see)
  a burst at every shutdown.
- `mkdocs/docs/admin/object-cache.md:104-123` (Prefetch section) documents
  the bounded queue and its load-shedding counters but says nothing about
  shutdown; add a note that on `SIGTERM` the worker stops admitting new
  items once the drain begins and any queued backlog is abandoned (Step 5).
- `mkdocs/docs/admin/object-cache.md:156-170` (In-process L1 cache) and
  `mkdocs/docs/architecture/caching.md` (after `### What is intentionally not
  cached in L1`, `:107`, or after the read-path-mechanics prose at `:65` —
  `:54` is mid-sentence, running into the eviction-policy table at `:57-60`,
  so there's no sentence boundary to attach to there): add a sentence
  clarifying that the L1 `RangeCache` used by FlightSQL/the monolith
  (`rust/object-cache/src/l1_store.rs:101`, wired via
  `rust/analytics/src/lakehouse/lakehouse_context.rs` and
  `rust/analytics/src/lakehouse/static_tables_configurator.rs`) gets the
  same accurate panic/shutdown log from Step 1, but **not** this plan's
  fetch-task drain — that drain is object-cache-srv-specific.
  `caching.md` is the right anchor for this, not
  `service-lifecycle.md:14-17`: those lines are table *body rows* for
  ingestion/FlightSQL/analytics-web/maintenance (nowhere to add a sentence),
  and the monolith has no row in that table at all (it appears only in the
  readiness-probe table at `:128`); if a service-lifecycle-side note is
  still wanted, target prose after `:54` or the object-cache row at `:18`
  instead. Without a note somewhere, a reader could infer from `caching.md`'s
  L1/L2-sameness framing that L1 drains too.
- `mkdocs/docs/admin/monolith.md`, `ingestion.md`, and `maintenance.md` need
  no change — they document no object-cache shutdown behaviour.
- While touching this area, also fix `rust/monolith/src/main.rs:13`'s stale
  doc comment claiming "the monolith runs no in-process cache" — it does run
  the in-process L1 `RangeCache` (`main.rs:183` ->
  `LakehouseContext::from_connection` -> `LakehouseContext::new` ->
  `l1_wrap`, `lakehouse_context.rs:75`); the comment predates that wiring and
  contradicts this plan's own Testing-Strategy note on the same point
  (`object_cache_srv.rs` integration test, below).

## Testing Strategy
- **Panic vs. shutdown log distinction** (`range_cache_tests.rs`), split into
  two halves since nothing public survives the shutdown-drop path (`InFlight`,
  `FulfillGuard`, and `FetchScheduler` are all `pub(super)`, and the joiner
  future dies with the runtime):
  - *Panic half*: drive a fetch that panics (the new panic-on-`get_opts`
    `ObjectStore` test double from step 8, panicking on the ranged-GET
    branch — `get_range` is not an overridable trait method in object_store
    0.13.2; ranged GETs go through `ObjectStoreExt::get_opts`'s blanket impl,
    which the existing `CountingStore` already discriminates on via
    `options.range`/`options.head`, `range_cache_tests.rs:156-190`) through a
    public entry point
    (e.g. `get_range`/`prefetch_blocks`) and assert the returned `Err`'s
    formatted text **contains** the panic-branch message ("fetch task
    panicked before producing a result"), confirming `FulfillGuard::drop`
    took the panic branch. The test cannot call `reconstruct_shared_error`
    directly — it's `pub(super)`, not visible from the external test crate —
    so the assertion goes through whatever public call path already invokes
    it internally (`fetch.rs:445`/`:467`); and it must be a substring check
    (`.contains(...)`), never equality, since `reconstruct_shared_error`
    formats with `{shared:?}`, which includes the full anyhow context chain
    around the message, not just the message itself.
  - *Shutdown-drop half*: this needs `micromegas_tracing::test_utils::init_in_memory_tracing()`
    (already used in `object-cache/tests/telemetry_tests.rs:11`) plus
    `micromegas_tracing::dispatch::flush_log_buffer()` and `#[serial]`
    (global sink; precedent `telemetry_tests.rs:71`), and a plain `#[test]`
    (not `#[tokio::test]`) that builds its own `tokio::runtime::Runtime` via
    `Builder::new_multi_thread()`, spawns the fetch on it, then
    `drop(runtime)` before it completes to force a
    task-drop-without-poll-to-completion — dropping a `Runtime` from inside
    an async context panics. The real precedent for this pattern is
    `rust/analytics/tests/async_span_tests.rs:38-60` (explicit
    `drop(runtime)` at `:60`); `telemetry_tests.rs:70-74` does not drop its
    runtime early and is not an example of this. Reading the captured log
    requires a new helper with no precedent in the repo (see Step 8);
    existing in-memory-sink assertions in this codebase all read
    `metrics_blocks` (`saturation_tests.rs:30,47`), not `log_blocks` — the
    new helper walks `MemSinkState::log_blocks`
    (`tracing/src/event/in_memory_sink.rs:23`, `pub`) and matches
    `LogMsgQueueAny::LogStringEvent` (`tracing/src/logs/block.rs:96`) to
    extract log text. Assert the captured log contains the shutdown-branch
    message, since that log line is the only thing observable on this path.
    This relies on a coupling worth stating explicitly: `dispatch.rs:725-737`
    routes non-interpolating log messages to `LogStaticStrEvent` instead, not
    `LogStringEvent`. Both new shutdown-log messages interpolate `{n}`, so
    `LogStringEvent` is the correct match today, but changing either message
    to a bare literal during implementation would silently make this helper
    find nothing.
- **Drain waits for outstanding tasks**: drive `RangeCache::prefetch_blocks`
  (public, and — unlike `get_range`, which resolves `size()` via a separate
  HEAD before calling `fetch_blocks` — takes `file_size` directly, so no
  HEAD call happens at all; that HEAD, when it does happen, is itself a
  tracked task whose guard can transiently overlap the block fetch's guard,
  making an `== 1` assertion racy) against a `CountingStore::with_gate` origin double
  (existing infrastructure, `range_cache_tests.rs:65`/`:91`, same pattern as
  the gated fetches at `:613`/`:684` — not `:751`/`:785`/`:836`, which use
  the ungated `CountingStore::new`) with an explicit `file_size` so no
  `size()` call happens at all. Poll with a `tokio::task::yield_now` loop
  (precedent `range_cache_tests.rs:620`) until `counting.get_range_count() >= 1`
  (the origin GET has actually started, so its `spawn_run_fetch` task guard
  is registered), then assert `cache.outstanding_fetch_tasks() >= 1`. Spawn
  `wait_for_fetch_tasks_drain()` via `tokio::spawn` into a `JoinHandle`, run
  a bounded `for _ in 0..20 { tokio::task::yield_now().await }` loop
  (precedent for the "must not progress" idiom applied to a `JoinHandle`:
  `rust/object-cache-srv/tests/memory_budget_tests.rs:359-365`; the
  superficially similar `range_cache_tests.rs:640-648`/`:692-695` assert on
  `counting.get_range_count()`, not `is_finished`, and don't apply here;
  avoids the wall-clock-guess pattern
  `tasks/completed/1252_test_quality_timing_tests_plan.md:7-14` steers away
  from), and assert `!handle.is_finished()` to confirm the drain has not
  resolved yet; release the gate and assert the handle finishes and the
  count returns to 0.
- **Drain resolves immediately with nothing in flight**: `wait_for_fetch_tasks_drain()`
  on a fresh cache returns without blocking (bound with a short timeout to
  catch a regression that hangs forever).
- **Demand fill survives close-without-eviction** (`foyer_backend_tests.rs`,
  Step 8): a single `FillHint::Demand` `put()` with no RAM eviction pressure
  (a `ram_bytes` generous enough that nothing evicts), `close()`, then a
  fresh `FoyerBackend::new_with_shards` reopened over the same directory,
  asserting `get()` still hits. This is the one case load-bearing for Design
  §4's persistence claim that no existing test covers: every current
  `Demand` put->`close()`->`get` case forces RAM eviction first with a tiny
  `ram_bytes` (its own comment notes "the disk write is triggered by memory
  eviction, not by insert itself"), and every close-without-eviction case
  uses `FillHint::Prefetch`'s `.force()` storage-only writer instead.
- **Prefetch worker actually stops on shutdown** (`prefetch_tests.rs`): the
  six existing `spawn_prefetch_worker` call sites thread a never-firing
  `std::future::pending::<()>()` into both new parameters (Step 5), so none
  of them exercises `take_until`, the window flag, or the deadline
  arithmetic — only an optional manual SIGTERM check does. Following #1037's
  `graceful_shutdown_tests.rs` precedent (driving drain from a `Notify`
  rather than real signals, for both the axum wrapper and a service-level
  drain function), add a case that: spawns the worker with real
  `Notify`-backed `shutdown`/`window_shutdown` futures, sends an item,
  triggers both notifies mid-flight, and asserts (a) the worker's
  `JoinHandle` completes, and (b) an item sent after the notify is not
  admitted (e.g. `tx.send(...)` returns an error, or the item is observably
  never warmed). This also requires extracting `object_cache_srv.rs`'s
  `main()` drain sequence (prefetch-worker await, fetch-task drain,
  `foyer.close()`) into a testable helper mirroring #1037's
  `run_tasks_forever`, since `main()` itself isn't unit-testable.
- **`object_cache_srv.rs` integration** (manual/optional, no new test file):
  since `main()` isn't easily unit-testable, verify via
  `local_test_env/ai_scripts/start_services.py` in its default **split**
  mode (with MinIO up and `MICROMEGAS_OBJECT_STORE_URI` set, so the object
  cache actually starts — `start_object_cache` is only wired into
  `start_split_mode`; `--monolith` starts the monolith *instead*, and the
  monolith runs no object-cache *server* — it does run the in-process L1
  `RangeCache`, which is unaffected by this plan), issuing a slow/streamed request
  against the cache, then sending SIGTERM to the `micromegas-object-cache-srv`
  PID and confirming via `/tmp/object_cache.log` that fetch tasks drain (or
  are abandoned with the new accurate wording) before the process exits, with
  no more spurious "(likely a panic)" during a clean shutdown.
- Full gate: `cargo fmt`, `cargo clippy --workspace -- -D warnings`,
  `cargo test -p micromegas-object-cache -p micromegas-object-cache-srv -p micromegas`.

## Open Questions
None — all prior questions resolved; see Design §4. (The foyer
flush-semantics question previously here — whether the close step should
keep foyer's default `flush_on_close = true` or opt into
`with_flush_on_close(false)` — is settled: foyer 0.22.3 itself defaults
`flush_on_close = true` (`foyer-0.22.3/src/hybrid/builder.rs:96-104`,
`cache.rs:278-286`, cited in Design §4 above), and a related but distinct
question — keeping the RAM-tier
`WriteOnEviction` policy rather than switching to `WriteOnInsertion` — was
already decided in-repo:
`tasks/completed/1281_ram_tier_eviction_instrumentation_plan.md:372-377`.
The code that depends on the `WriteOnEviction` default lives at
`rust/object-cache/src/foyer_backend.rs:36-41,92-95`, and existing tests
(`rust/object-cache-srv/tests/saturation_tests.rs:135`,
`rust/object-cache/tests/foyer_backend_tests.rs`'s `put`->`close`->`get`
idiom) already codify flush-on-close as the contract. Design §4 keeps
foyer's defaults and relies on `close()`'s default RAM-tier flush.)
