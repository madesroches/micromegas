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
the synthesized-error path. Axum's own HTTP drain runs first, awaited
directly for up to the full grace period; the three post-axum stages then
run in sequence, bounded by whatever of that grace period axum didn't use,
in priority order (live client connections, then the prefetch worker, then
demand fetches, then cache persistence); on a saturated shutdown the later
stages can get little or no time, so cross-restart warmth from the close
step is best-effort, not guaranteed.

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
completes, within the same overall grace-period deadline (see Design
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
Add a task counter + `Notify` to `FetchScheduler`. Its race-free ordering
(construct the `Notified` future, re-check state, *then* await it) is not a
mirror of the existing `any_entry_promoted` pattern in this file: that
code's safety comes from `notify_one`'s stored permit
(`scheduler.rs:346-350`'s own comment describes exactly this), a different
guarantee from the `notify_waiters`-ordering argument `wait_drained`'s doc
comment below actually relies on — see that comment for the correct
rationale:

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
    /// construction: `Notified::enable`'s docs guarantee that a
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

### 3. Stop new prefetch intake, then run the shutdown sequence under one overall deadline
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
window loop.** Thread a single shutdown signal into `spawn_prefetch_worker`,
and also into `warm_item` itself, via a new `ShutdownFanout::receiver() ->
watch::Receiver<bool>` accessor (`rust/public/src/servers/shutdown.rs`,
alongside the existing `subscribe()`): a `watch::Receiver<bool>` is cheap to
clone into every concurrent `warm_item` call and supports exactly the
synchronous check `warm_item` needs (`*rx.borrow()`), and the worker's
`take_until` future can be derived from the same receiver
(`rx.wait_for(|v| *v)`) — so one subscription covers both needs and no
second future or extra detached task is required. The worker wraps a
`take_until` future derived from the receiver around its stream so it stops
pulling *new* items from the channel the moment the signal fires.
Correctness of the fetch-task drain doesn't depend on `warm_item` itself
noticing shutdown: as described below, `main()` awaits the prefetch
worker's `JoinHandle` — which only resolves once every in-flight
`warm_item` call has fully finished — *before* trusting
`wait_for_fetch_tasks_drain()`, which is what actually closes the window
where an already-started `warm_item` call spawns one more `spawn_run_fetch`
task between windows. Checking the receiver inside `warm_item`'s loop is
therefore about latency, not correctness: `warm_item` pulls its
`BlockWindows` through `.buffered(WINDOW_CONCURRENCY)` with
`WINDOW_CONCURRENCY = 1` (`prefetch_queue.rs:21`), so without that check an
already-started `warm_item` call would keep pulling one window at a time,
uninterrupted, until its entire window stream is exhausted — and the
awaited `JoinHandle` (bounded by the same overall grace deadline as
everything else, see the "Concurrent drain" trade-off below) would have to
wait that long before the fetch-task drain and `foyer.close()` ever run.
`warm_item` therefore checks
`*shutdown.borrow()` before pulling its next window so it stops between
windows instead of running to completion:

```rust
async fn warm_item(cache: &RangeCache, item: PrefetchItem, block_size: u64, shutdown: &watch::Receiver<bool>) {
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
        // Stop pulling the next window once shutdown has fired. This check
        // runs when `stream`'s internal buffer is empty -- right after the
        // previous `next()` returned, or before the first one -- so with
        // `WINDOW_CONCURRENCY = 1` there is no window in flight to abandon
        // here; returning just drops `stream`. What survives past this
        // return is any already-spawned `spawn_run_fetch` task dispatched
        // by a window that already completed, each tracked by its own
        // `FetchTaskGuard`.
        if *shutdown.borrow() {
            // Emit the metric on this path too: a shutdown return is a
            // clean partial warm (some windows already succeeded), not a
            // known failure like the error-return path just below, which
            // does not emit this metric today.
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
    shutdown: watch::Receiver<bool>,
) -> (mpsc::Sender<PrefetchItem>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<PrefetchItem>(queue_capacity);
    let take_until_shutdown = {
        let mut rx = shutdown.clone();
        async move {
            let _ = rx.wait_for(|v| *v).await;
        }
    };
    let handle = tokio::spawn(async move {
        let block_size = cache.block_size();
        ReceiverStream::new(rx)
            .take_until(take_until_shutdown)
            .for_each_concurrent(worker_concurrency, |item| {
                let cache = cache.clone();
                let shutdown = shutdown.clone();
                async move { warm_item(&cache, item, block_size, &shutdown).await }
            })
            .await;
    });
    (tx, handle)
}
```

`shutdown` is `fanout.receiver()` from the call site (see Design §3's
`main()` snippet below) — the new accessor `ShutdownFanout` gains alongside
`subscribe()`, since a plain `watch::Receiver<bool>` is what both the
worker's `take_until` and `warm_item`'s synchronous check need, and it's
cheap to clone into every concurrent `warm_item` call.

Note: `for_each_concurrent` only polls the derived `take_until` future (and can
therefore only drop `rx`, closing `prefetch_tx`'s receiving end) once a
worker slot frees up — it gates polling the underlying stream on
`futures.len() < limit` (`futures-util-0.3.32/src/stream/stream/for_each_concurrent.rs:79`).
With `worker_concurrency` (default 8) `warm_item` futures in flight and no
per-fetch origin timeout, the channel can therefore stay open well past the
moment shutdown fires, and prefetch POSTs that arrive in that window are
still admitted into the queue rather than immediately hitting
`handlers.rs:703`'s `error!("prefetch queue worker is gone")` plus a 503.
That 503 path is real but not guaranteed on every shutdown — it only fires
once a slot has actually freed and the channel has closed, but once it
does, it stays closed for the rest of the drain window, so every
`/prefetch` POST in that window hits it. Logging that at `error!` is at
odds with this repo's own policy that routine shutdown degradation logs at
`debug` and only genuinely unexpected conditions log at `warn`/`error`
(`mkdocs/docs/admin/object-cache.md:270`) — a routine SIGTERM is exactly
the case here, not an unexpected one. This plan does not change
`handlers.rs` to reconcile that (see the matching Trade-offs entry); the
`error!` + 503 on this path during a normal shutdown is accepted as-is.

**The worker's `JoinHandle` must be awaited before the fetch-task drain
starts**, not discarded. `main()` currently binds it to `_prefetch_worker`
and never looks at it again; that leaves a window where `warm_item`'s
in-flight window fetch has been dispatched (tracked by `FetchTaskGuard`) but
`join_prefetch` hasn't yet reached the `remove_entry`/`guard.disarm()` tail
of the spawned fetch task, and the *next* window's future isn't even
constructed yet — so `outstanding_tasks` can legitimately read 0 between two
windows of the same still-running `warm_item` call. Awaiting
`_prefetch_worker` first — within the single deadline that bounds the
three post-axum stages (see below) — establishes that the last prefetch
producer has actually exited before `wait_for_fetch_tasks_drain()` is
trusted to mean "no more origin GETs will be spawned."

`spawn_saturation_monitor` itself does **not** take a shutdown parameter.
The sample loop's only stated reason to take one would be to stop sampling
and drop its `prefetch_tx` clone early, but neither holds up: the drain
budget is short (default 25s grace) and `SAMPLE_INTERVAL` is 5s, so a
maxed-out drain is exactly the case where telemetry from the last few
samples is most useful, not something to cut off at the first sign of
shutdown; and once the worker exits via the derived `take_until` future
(and the window-loop check above), nothing depends on this clone's closure
anymore. The causality runs the other way from "the channel has to close
for `for_each_concurrent` to stop pulling": once `take_until` yields
`None`, `ForEachConcurrent` sets its own `this.stream` to `None`
(`futures-util-0.3.32/src/stream/stream/for_each_concurrent.rs:96-97`),
which is what drops the wrapped `ReceiverStream` and, with it, `rx` —
that's what closes the channel, not the reverse — so no other `prefetch_tx`
clone needs to go away for the worker to stop pulling. The sampler is
therefore left unchanged and simply dies with the runtime, like today:

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

**One overall deadline, not per-stage budgets.** A concurrent `tokio::join!`
of the axum drain and the fetch-task drain does not deliver the intended
guarantee: axum handlers are the *producers* of demand fetch tasks
(`RangeCache::size`'s HEAD, and `stream_demand_windows`'s lazy, `buffered(2)`
per-window `spawn_run_fetch` calls made from inside the streamed response
body), so a slow client mid-stream can have zero outstanding fetch tasks at
the instant shutdown begins — `wait_drained()` would resolve immediately —
and then spawn more after that, which the runtime would still drop at
teardown. The fetch drain must therefore start only *after* axum's own drain
has actually finished.

Four stages need to run in that order — axum drain, prefetch-worker await,
fetch-task drain, `foyer.close()` — and that order **is** the priority
order: live client connections come first, then the prefetch worker (a
background producer nobody is blocked on), then the demand-fetch drain
(bytes a caller is actually waiting on), then cache persistence (a
cross-restart nicety no caller observes directly). Four successive
revisions of this plan tried to give each stage its own slice of `grace` —
first an even split, then fixed percentages, then weighted percentages with
a cap on axum's share and a floor under every stage, then a version that let
`foyer.close()` absorb whatever the other three didn't use — and each
revision's design review found a new hole in the arithmetic (see
Trade-offs). This plan abandons per-stage budgeting entirely. Instead, axum's
drain keeps running exactly as it does today — awaited directly, not wrapped
in anything new — and one single deadline covers the three stages that come
after it:

```rust
// main() — unchanged shape, still awaited with `?` for consistency with
// this helper's other callers. `serve_axum_with_graceful_shutdown` always
// resolves `Ok(())` (see below), so this line can never itself produce a
// nonzero exit:
serve_axum_with_graceful_shutdown(listener, make_service, shutdown_signal, grace).await?;

// `shutdown_signal` (an `async move` wrapper around `wait_for_sigterm()`,
// built in the main() snippet below) stamps `signal_at` -- a shared
// `Arc<OnceLock<Instant>>` -- the instant the shutdown signal fires. It is
// always set by the time the line above returns, because
// `serve_axum_with_graceful_shutdown`'s `tokio::select!`
// (`shutdown.rs:106-115`) can only resolve once its own internal fanout has
// observed that same signal.
let remaining = grace.saturating_sub(signal_at.get().expect("signal_at set before serve returns").elapsed());

// stage order IS the priority order; `remaining`, not `grace`, is what
// bounds these three stages, since axum may have already spent some (or
// all) of `grace` on its own drain. `run` returns `()`, not a `Result`: an
// elapsed deadline is logged at `warn` and swallowed inside `run`, never
// propagated, so this call is `.await;`, not `.await?`.
shutdown_sequence::run(remaining, prefetch_worker, cache_for_drain, foyer).await;
```

```rust
// inside shutdown_sequence::run (`async fn run(...) -> ()`):
// Set first, before the early-return check below and outside the timeout
// entirely, so every branch -- including the early return and the
// timeout-abandoned branches, where `close()` never starts -- suppresses
// the #1281 eviction gauges on foyer's drop-time close fallback (Design
// §4). Calling this only right before `foyer.close()` would leave it unset
// on exactly those branches.
foyer.mark_shutting_down();

// Below this, and not exactly zero, is treated as "not enough of the grace
// period left to be worth attempting" -- see the `saturating_sub`
// discussion below for why a strictly-positive-but-tiny `remaining` is just
// as dangerous as exactly zero.
const MIN_POST_AXUM_BUDGET: Duration = Duration::from_millis(50);

if remaining < MIN_POST_AXUM_BUDGET {
    // A `tokio::time::timeout` below still polls the wrapped future once,
    // unconditionally, before ever consulting its deadline (see the
    // `saturating_sub` discussion below) -- enough to latch foyer's
    // `closed` flag and disable the drop-time close fallback even when
    // `remaining` is a few milliseconds, not exactly zero. This early
    // return is what actually skips the three stages outright.
    warn!("axum drain left less than {MIN_POST_AXUM_BUDGET:?} of the grace \
           period; skipping prefetch-worker await, fetch-task drain, and \
           foyer close");
    return;
}
if tokio::time::timeout(remaining, async {
    // `JoinHandle::Output` is `Result<(), JoinError>`, `#[must_use]`, so
    // this must be handled, not a bare `.await;` -- and the plan's own
    // `-D warnings` gate would fail `unused_must_use` otherwise.
    if let Err(e) = prefetch_worker.await {
        warn!("prefetch worker panicked: {e:#}");
    }
    cache.wait_for_fetch_tasks_drain().await;
    foyer.close().await;
}).await.is_err() {
    // Elapsed and swallowed here, not propagated -- see Design §3's
    // warning set below for the elapsed-deadline and
    // abandoned-fetch-count `warn!`s this branch actually logs.
}
```

This is the direct fix for a bug in an earlier draft of this plan: wrapping
the axum serve call itself inside `tokio::time::timeout(grace, ..)` does not
work, because `tokio::time::timeout` arms its `Sleep` at creation time — at
process boot, for a serve future that runs for the process's entire
lifetime — not when the shutdown signal fires. That timeout would therefore
elapse `grace` seconds after *startup*, at which point the whole wrapped
block (including the still-running `serve_future`) gets dropped and the
error is discarded, silently killing the server long before any real
shutdown signal arrives. Keeping axum's own `await` outside any timeout
(exactly as it works today) and measuring the *second* deadline from
`signal_at` rather than from process start fixes the problem: the post-axum
deadline actually reflects time remaining in the grace period, not time
since boot. (Unlike the rejected draft, this fix has nothing to do with an
error path surviving: `serve_axum_with_graceful_shutdown` always resolves
`Ok(())` — see below — so the real fix is entirely about when the timeout's
`Sleep` starts.)

`serve_axum_with_graceful_shutdown` keeps the **full** `grace` it gets today
(`object_cache_srv.rs:200-207`) — nothing changes about axum's own drain or
the meaning of its existing log lines (`shutdown.rs:85,91,112`,
`mkdocs/docs/admin/service-lifecycle.md:49-54`). (This helper's `.await?` can
never itself produce a nonzero exit: `WithGracefulShutdown::into_future`
always resolves `Ok(())` — axum 0.8.9's accept loop only exits via `break` on
the shutdown signal, `serve/mod.rs:289,345-350` — so the `?` is inert, kept
only for shape-consistency with this helper's other callers.)
Because axum's own deadline arm always sleeps for the entire duration it's
given before returning (`shutdown.rs:98-115`), a busy axum drain can by
itself consume the whole outer `grace`, leaving little or nothing — or,
once scheduling overhead is accounted for, nothing at all — for the three
stages that follow it. `remaining` is computed with `saturating_sub`
specifically for that case: if axum's drain took `grace` or longer,
`remaining` is `Duration::ZERO`. Skipping the three stages on exactly that
value would not be enough: `Timeout::poll` polls the wrapped future
unconditionally *before* checking its deadline
(`tokio-1.52.3/src/time/timeout.rs:211-222`), so `tokio::time::timeout(d,
..)` still polls the wrapped future once *regardless of how small `d` is* —
enough for `prefetch_worker.await` or `wait_for_fetch_tasks_drain()` to
resolve immediately if there's nothing to wait for, and, worse, for
`foyer.close()`'s first poll to reach `closed.fetch_or(true,
Ordering::Relaxed)` (`foyer-0.22.3/src/hybrid/cache.rs:319-321`) *before*
its first await point — latching `closed` and then abandoning the future,
which permanently disables the drop-time `impl Drop for Inner` close
fallback the Overview describes as today's baseline. That risk is exactly
as real when `remaining` is a few milliseconds — axum's drain finishing
just under `grace` on a busy shutdown — as when it's precisely zero: either
way the timeout gets its one poll, and that's enough to do the damage. So
the guard can't be `remaining.is_zero()`; it has to be a minimum viable
budget below which attempting the three stages is strictly worse than
skipping them. `shutdown_sequence::run` therefore checks for this case
explicitly, before ever constructing the timeout — the `if remaining <
MIN_POST_AXUM_BUDGET { ... return; }` guard shown at the top of the `run`
snippet above. With that early return in place, a saturated (or
near-saturated) axum drain genuinely means the prefetch-worker await, the
fetch-task drain, and `foyer.close()` are skipped outright, not attempted
and cut short. That is accepted, not engineered around: this design does
not try to guarantee any minimum time for those three stages on a saturated
shutdown.
Cross-restart cache warmth — the benefit Design §4 adds — is therefore
explicitly **best-effort**, never a guarantee, and the Overview and this
section should be read that way.

If the post-axum timeout fires, the `async` block wrapping the three stages
is dropped mid-stage, and whatever it was doing is simply abandoned — the
same fate every in-flight future has today at process-exit runtime teardown.
(The `remaining < MIN_POST_AXUM_BUDGET` case is handled separately, by the
early return shown above, before this timeout is ever constructed — see the
`saturating_sub` discussion above for why a too-small-duration timeout can't
be trusted to skip the block on its own.) Concretely:
- If it fires while awaiting `prefetch_worker`, the `JoinHandle` is dropped,
  which *detaches* the task rather than stopping it
  (`tokio-1.52.3/src/runtime/task/join.rs:18,35`); the worker keeps running,
  unjoined, until the process exits a moment later anyway. There is no
  `abort()` call here (an earlier draft had one): the process is about to
  exit either way, so detaching costs nothing an explicit abort would have
  saved.
- If it fires while awaiting `wait_for_fetch_tasks_drain()`, any
  still-outstanding fetch tasks are abandoned the same way `FulfillGuard::drop`'s
  shutdown branch (Design §1) already describes.
- If it fires during `foyer.close()`, the flush is abandoned permanently,
  not merely delayed: `close_inner` latches `closed.fetch_or(true, ...)`
  *before* flushing (`foyer-0.22.3/src/hybrid/cache.rs:319-321`), so the
  runtime's own drop-time close attempt (`impl Drop for Inner`, the same
  fallback the Overview describes) sees `closed` already set and returns
  `Ok(())` immediately without retrying — a `foyer.close()` interrupted by
  the outer deadline gets no second chance.

With only one deadline around the three post-axum stages, there is no
per-stage timeout branch left to instrument individually, so the warning set
collapses to what's actually observable from outside the dropped block
(kept minimal and consistent with `mkdocs/docs/admin/object-cache.md:270`'s
policy — routine conditions log at `debug`, genuinely unexpected ones at
`warn`/`error`):
- If the post-axum timeout elapses (only reachable when `remaining` was at
  least `MIN_POST_AXUM_BUDGET` — the `remaining < MIN_POST_AXUM_BUDGET` case
  returns early before this timeout is ever constructed, see above) — log
  once at `warn` (an elapsed shutdown deadline is not routine — the same
  level `serve_axum_with_graceful_shutdown`'s own deadline arm already uses)
  that the grace period elapsed before the sequence finished, so whichever
  stage was still running was abandoned.
- On the `remaining < MIN_POST_AXUM_BUDGET` early-return branch, log once at
  `warn` (same reasoning) that axum's drain left too little of the grace
  period for the three stages to be worth attempting, and all three were
  skipped outright before ever starting — the `warn!` shown in the
  early-return snippet above.
- On either of the two branches above, if `cache.outstanding_fetch_tasks()`
  — read from a clone held outside the `async` block, since the block
  itself was dropped (or, on the early-return branch, never constructed) —
  is still nonzero, log a second `warn` reporting that count. This is the
  one per-stage-shaped warning worth keeping: "how many origin fetches were
  abandoned" is the one number an operator can actually act on.
- A `prefetch_worker` `JoinError` (task panicked) and an `Err` from
  `foyer.close()` both stay `warn`, exactly as before, on the branch where
  the sequence finishes within `remaining` (so the block is still alive to
  log them).
- Successful completion of each stage stays an `info!`, as today
  (`origin fetch tasks drained`, `foyer cache closed`) — this isn't the
  routine/unexpected distinction the `debug`/`warn` policy is about.

```rust
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use micromegas::servers::shutdown::{ShutdownFanout, wait_for_sigterm};
use micromegas_object_cache_srv::shutdown_sequence;
...
let grace = args.common.grace();

// Stamped the instant the shutdown signal fires, so `remaining` below can
// measure from when shutdown actually started rather than from process
// boot. Wrapping `wait_for_sigterm()` this way means the stamp happens
// exactly once, inside the same future `ShutdownFanout` polls to fan the
// signal out to every subscriber.
let signal_at: Arc<OnceLock<Instant>> = Arc::new(OnceLock::new());
let shutdown_signal = {
    let signal_at = signal_at.clone();
    async move {
        wait_for_sigterm().await;
        let _ = signal_at.set(Instant::now());
    }
};
let fanout = ShutdownFanout::new(shutdown_signal);

let (prefetch_tx, prefetch_worker) = spawn_prefetch_worker(
    cache.clone(),
    args.prefetch_queue_capacity,
    args.prefetch_worker_concurrency,
    fanout.receiver(), // gates both the channel's take_until and warm_item's per-window check
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
// moved into the router by `.with_state(state)` further down. Kept alive
// past the shutdown sequence below so the outstanding-task count is still
// readable for the elapsed-deadline warning even if the sequence itself
// was dropped.
let cache_for_drain = state.cache.clone();

... // router construction, listener bind (unchanged)

// Awaited directly, outside any timeout, exactly as today: axum keeps the
// full `grace` for its own drain. This call always resolves `Ok(())`
// (Design §3), so the `?` here is inert and can't itself produce a
// nonzero exit.
serve_axum_with_graceful_shutdown(
    listener,
    app.into_make_service_with_connect_info::<SocketAddr>(),
    fanout.subscribe(),
    grace,
)
.await?;

// `signal_at` is always set by the time the call above returns -- see
// Design §3's explanation above. `remaining`, not `grace`, is what bounds
// the three stages below, since axum may have already spent some (or all)
// of `grace` on its own drain.
let remaining = grace.saturating_sub(
    signal_at.get().expect("signal_at set before serve returns").elapsed(),
);

shutdown_sequence::run(remaining, prefetch_worker, cache_for_drain, foyer).await;
```

`shutdown_sequence::run` owns the `remaining < MIN_POST_AXUM_BUDGET` early return, the
single `tokio::time::timeout(remaining,
...)` wrapping these three post-axum stages, plus the
elapsed-deadline/abandoned-fetch-count warnings; it takes only `remaining`
and the three handles it needs (`prefetch_worker`, `cache`, `foyer`) — no
budget struct, and no axum inputs, since axum is awaited directly in
`main()` above, outside `run` entirely. `run` returns `()`, not a
`Result`: an elapsed deadline is logged at `warn` and swallowed inside
`run` (Design §3's warning set), so it can never turn into a nonzero exit —
consistent with the axum stage above it, whose `.await?` also can't produce
one, since `serve_axum_with_graceful_shutdown` always resolves `Ok(())`.

`AppState::cache` (`app_state.rs`) is already `Clone` and cheap — every
field is an `Arc` clone or a `Copy` scalar except `ns: String`, one small
heap allocation, not a concern on a shutdown path — so cloning it via
`state.cache.clone()`
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
- **A close call, the last stage under the one overall deadline.** After the
  fetch-task drain (Design §3) completes, call `foyer.close()` as the last
  stage inside the same single `tokio::time::timeout(remaining, ...)` that
  bounds the three post-axum stages — there is no separate per-stage budget
  for this call. If the overall deadline elapses while it's running, the close is
  simply abandoned (Design §3's timeout-branch discussion, and the
  `closed.fetch_or` note below). This plan keeps
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
  `HybridCachePipe::flush` calls `store.enqueue(piece, false)` — so an entry
  can be silently not admitted if a device write throttle is active. A
  throttle would decline the prefetch path at the same place too: `.force()`
  only bypasses the *writer-level* pre-check
  (`foyer-0.22.3/src/hybrid/writer.rs:126-140`), not the disk engine's own
  admission check below it, and no `enqueue(.., true)` exists anywhere in
  the foyer facade for either path to bypass that check
  (`foyer-storage-0.22.3/src/engine/block/engine.rs:430`,
  `foyer-storage-0.22.3/src/filter.rs:113-125`); this repo configures no
  throttle, so every entry is admitted today, but the dependency is worth
  noting. Three further caveats on how completely this close-time flush
  covers the RAM tier: (1) the disk engine also silently drops an enqueue
  independent of any throttle when its submit queue is already over
  threshold — `storage_queue_channel_overflow++; return;` when
  `submit_queue_size > submit_queue_size_threshold`
  (`foyer-storage-0.22.3/src/engine/block/engine.rs:591-594`) — and this
  repo pins that threshold to `2 * write_buffer_mb` = 256 MiB by default
  (`object_cache_srv.rs:110-114`, `cli.rs:150` default 128), while the
  default RAM tier is 512 MiB (`cli.rs:24`), so a full-tier close flush
  pushed through one `HybridCachePipe::flush` loop can plausibly exceed the
  submit-queue threshold and silently lose bytes this close step exists to
  persist, independent of any write throttle; (2) even under that
  threshold, a flusher's own write buffer can independently be full: `push`
  returning `false` bumps `storage_queue_buffer_overflow` and drops the
  entry (`foyer-storage-0.22.3/src/engine/block/flusher.rs:420-442`), and
  each of the default 2 flushers gets only `buffer_pool_size / flushers` =
  128 MiB / 2 = 64 MiB (`engine.rs:412`) — `HybridCachePipe::flush` enqueues
  the whole tier in one loop with no await point once no device throttle is
  configured (`foyer-0.22.3/src/hybrid/cache.rs:244-262`), so the producer
  never yields to let a flusher drain its buffer, and this path can drop
  entries independent of, and in addition to, caveat (1); and (3) `close()`'s
  eviction-to-zero sweep (`foyer-memory-0.22.3/src/raw.rs:119-137`'s
  `evict(0, ...)`) pops from the eviction container and stops on the first
  `None`, so entries still pinned by an outstanding `get`-derived handle are not
  swept — the sweep is therefore not *every* RAM entry, only every
  evictable one; and (4) most, but not all, entries already on disk are
  intentionally not rewritten: the disk->RAM promotion path
  (`promote_if_valid`, `foyer_backend.rs:368-419`, reached via
  `load_and_promote` -> `load_from_disk` -> `RangeCacheBackend::get`)
  forwards the loaded block's `age` into
  `HybridCacheProperties::default().with_age(age)` (`foyer_backend.rs:412-417`)
  rather than hardcoding it, and foyer's disk-engine load path stamps
  `age = Age::Old` only for blocks marked "probation"
  (`foyer-storage-0.22.3/src/engine/block/engine.rs:704-706`) — a ~10% share
  under the default `FifoPicker`'s `probation_ratio = 0.1`
  (`foyer-storage-0.22.3/src/engine/block/eviction.rs:66-69`), which
  `FoyerBackend` never overrides. So roughly 90% of disk->RAM promotions are
  stamped `Age::Young`, and `BlockEngine::enqueue` skips (does not write)
  any entry with that age (`engine/block/engine.rs:582-589`, bumping
  `storage_block_engine_enqueue_skip`, no write) — correctly, since those
  bytes are already durable — but the remaining ~10%, promoted from a
  probation block and stamped `Age::Old`, *are* re-enqueued and rewritten by
  the close-time flush. A promoted entry's RAM residency therefore usually,
  but not always, means the close-time flush skips writing it again. A
  demand `put` is unrelated to this promotion path: `FoyerBackend::put`
  (`foyer_backend.rs:518-556`) sets no properties at all for its demand arm
  (a plain `self.cache.insert(...)`, `:553`), so it lands with
  `HybridCacheProperties::default()` — `Age::Fresh` — and `enqueue` writes
  `Age::Fresh` entries just like `Age::Old` ones, skipping only `Age::Young`.
  That `Age::Fresh` stamp is precisely why the close-time flush persists a
  RAM-resident demand fill, which is this section's load-bearing claim.
  Neither overflow
  counter in (1)/(2) is wired into this repo's telemetry — `FoyerBackend`
  never calls foyer's `with_metrics_registry`, and neither counter is
  reflected in `BackendDiskStats`/`backend_disk_stats` — so today this loss
  is invisible, and this plan does not attempt to measure it (see the
  matching Trade-offs entry, "Close-time flush loss is left unmeasured, not
  approximated" — an earlier draft logged a coarse RAM-usage-before vs.
  disk-write-bytes-after delta as a proxy, and that heuristic is removed:
  the two quantities are incommensurable, and caveat (4) above means the
  comparison reads as a shortfall on nearly every clean shutdown of a warm
  cache even when nothing was actually lost). The new `foyer_backend_tests.rs`
  case below (Step 8) proves the close flush works on the unpinned,
  unsaturated path; it does not exercise any of these four caveats.

```rust
// `mark_shutting_down()` already ran at the very top of
// `shutdown_sequence::run`, before the `MIN_POST_AXUM_BUDGET` early return
// (Design §3) -- not here -- so `RamEvictionListener::on_leave` (see below)
// can tell this flush apart from capacity-driven thrashing even on a
// branch where `close()` itself never gets called.
match foyer.close().await {
    Ok(()) => info!("foyer cache closed"),
    Err(e) => warn!("foyer cache close failed: {e:#}"),
}
```

There is no per-call timeout here and no byte-count claim on success — this
is the tail of `shutdown_sequence::run`'s single `async` block (Design §3);
if the overall `grace` deadline elapses while this call is running, the
block (including this `match`) is dropped before either log line runs, and
Design §3's elapsed-deadline warning fires instead.

**Close-time RAM flush would otherwise poison the #1281 eviction gauges.**
`close()`'s eviction-to-zero sweep calls `listener.on_leave(Event::Evict,
...)` for every flushed entry (`foyer-memory-0.22.3/src/raw.rs:657-675`).
`RamEvictionListener::on_leave` (`foyer_backend.rs:203-230`) emits
`object_cache_ram_tier_eviction_count` for all four `Event` reasons, but
gates `object_cache_ram_tier_eviction_age_ms` on `Event::Evict` specifically
— that's the one it treats as the capacity-driven thrashing signal (#1281).
Left unguarded, every clean shutdown would emit both gauges for the *entire*
RAM tier (default `--ram-mb` 512, `cli.rs:24`) — indistinguishable from real
capacity thrashing. This isn't limited to the branch where `run` explicitly
calls `foyer.close()`: on the `remaining < MIN_POST_AXUM_BUDGET` early
return, and on any branch where the overall deadline fires before
`foyer.close()` starts, `run` returns without ever calling `close()` itself,
and it's `Arc<FoyerBackend>`'s drop-time `impl Drop for Inner` (the
Overview's baseline fallback) that ends up doing the full-tier flush instead
— on exactly the saturated-shutdown branch where this matters most. Making
the flag observable in time for that branch, not just the explicit-`close()`
branch, is why it must be set at the very top of `run`, before either the
early return or the timeout, rather than immediately before the `close()`
call (see the `run` snippet in Design §3). Making that check possible from
`on_leave` requires more than a field on `FoyerBackend`: `RamEvictionListener`
is a separate struct built and moved into the foyer builder (as an
`Arc<dyn EventListener>`, `foyer_backend.rs:314,317`) *before* `FoyerBackend`
itself is constructed (`:335-339`), and neither holds a reference to the
other, so a plain `FoyerBackend` field would be unreachable from `on_leave`.
Instead, create a `shutting_down: Arc<AtomicBool>` before the listener is
built, clone it into both `RamEvictionListener` and `FoyerBackend` —
mirroring the existing `tags: Arc<EvictionTagTable>` sharing between the
same two constructs (`:313-314`/`:336`) — and have `on_leave` check it and
skip emission when set, the same short-circuit shape already used for the
`is_prefetch` phantom-record case just above it. Because `main()` lives in a
different crate, the flag also needs a public setter —
`FoyerBackend::mark_shutting_down()` — called from the top of
`shutdown_sequence::run`, not immediately before `foyer.close()` (see the
snippet above and the `run` snippet in Design §3). `mkdocs/docs/admin/object-cache.md`
notes that these two gauges go quiet during the final close step rather than
spiking.

This is what actually delivers the Overview's claim: a demand fetch's bytes
are only recoverable after restart once this close step exists — and, per
Design §3, only on the best-effort branch where the overall deadline leaves
this stage enough time to run at all. A demand
fill's `put()` only lands in the RAM tier during normal operation (the
`WriteOnEviction` policy is unchanged by this plan), but that's irrelevant
to `close()`'s own flush: `close()`'s default eviction-to-zero sweep pipes
every *evictable* RAM entry — demand fills included, but not entries still
pinned by an outstanding `get`-derived handle (see the caveats above) — through the
write pipeline regardless of how (or whether) it got written during normal
operation. No change to the write policy is needed, and none is made.

**Post-close cache access by orphaned callers is unaddressed.** On the
branch where `close()` completes successfully, `get` still serves from RAM
and disk afterward, but a `put` lands in RAM and is then silently dropped
when the pipe tries to enqueue it
(`foyer-storage-0.22.3/src/engine/block/engine.rs:570-574` logs
`warn!("cannot enqueue new entry after closed")`); no error reaches the
caller. That's specifically the *successful*-close branch: `active` is set
`false` only inside `BlockEngine::close` (`engine/block/engine.rs:563`),
which runs *after* `memory.flush()` (`hybrid/cache.rs:327-329`) — so on the
branch where the overall deadline instead abandons `foyer.close()` mid-flush
(Design §3), the engine may still be active and a late `put` landing in that
window may actually still be written. The expected source of a post-close
`put` on the successful-close branch is an axum connection that survived
axum's own drain as an orphaned task (see the "Concurrent drain"
trade-off): it can still call `RangeCache::size`/`spawn_run_fetch` after the
fetch-task drain has already resolved to zero and `foyer.close()` has run.
No code change is made for this (see the matching Trade-offs note).

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
   `tokio::spawn`, then move it into the async block. `fetch.rs`'s existing
   `use super::scheduler::{...}` list (`fetch.rs:19-22`) doesn't include
   `FetchScheduler` — add it there; `mod.rs:22` already imports it, so only
   `fetch.rs` needs the new import.
4. **Expose on `RangeCache`** — `rust/object-cache/src/range_cache/mod.rs`:
   `outstanding_fetch_tasks()`, `wait_for_fetch_tasks_drain()`.
5. **Stop new prefetch intake on shutdown** — add `ShutdownFanout::receiver()
   -> watch::Receiver<bool>` to `rust/public/src/servers/shutdown.rs`
   (`self.tx.subscribe()`, alongside the existing `subscribe()`). In
   `rust/object-cache-srv/src/prefetch_queue.rs`, add a single new
   `shutdown: watch::Receiver<bool>` parameter to `spawn_prefetch_worker`
   (the function takes three parameters today — `cache: RangeCache,
   queue_capacity: usize, worker_concurrency: usize`,
   `prefetch_queue.rs:113-117` — this adds a fourth); derive the worker's
   `take_until` future from it (`rx.wait_for(|v| *v)`) and pass the cloned
   receiver into `warm_item`, which checks `*shutdown.borrow()` before
   pulling each window's next item from its `buffered` stream. The only new
   import this needs is `tokio::sync::watch` (not currently present,
   `prefetch_queue.rs:1-10`) — do not add `std::future::Future`: nothing in
   the final code names `Future` (`take_until`'s generic bound is on the
   trait, not a use of it), so it would be an unused import and fail
   `cargo clippy --workspace -- -D warnings`. Update the three
   now-inaccurate doc comments this touches: `prefetch_queue.rs:107-112`'s
   `spawn_prefetch_worker` doc (the handle no longer resolves only on
   channel closure) and `saturation_monitor.rs:160-163` (only the clause
   claiming the sampler's handle parallels the prefetch worker's is now
   stale — the sampler's own handle still needn't be awaited, per Design
   §3). `app_state.rs:28-31`'s `prefetch_tx` doc comment needs no change: it
   says the channel "closes only once the server shuts down," never that
   closure is what *stops* the worker, so it stays true verbatim. Also
   update the six existing call sites in
   `rust/object-cache-srv/tests/prefetch_tests.rs` (`:180`, `:247`, `:294`,
   `:447`, `:534`, `:597`) to pass a single never-firing receiver for the
   one new parameter, constructed as
   `let (_shutdown_tx, shutdown_rx) = watch::channel(false);` — a
   **retained, named `Sender`**, not `watch::channel(false).1` alone. The
   latter drops the `Sender` at the end of the statement, and a dropped
   `watch::Sender` is not equivalent to a signal that never fires: on a
   closed channel `rx.wait_for(|v| *v)` returns `Err(RecvError)`
   immediately (tokio 1.52.3 `sync/watch.rs:896-931`), and
   `TakeUntil::poll_next` polls that future before the stream and returns
   `Poll::Ready(None)` on the very first poll
   (`futures-util-0.3.32/src/stream/stream/take_until.rs:117-138`) — the
   worker would exit before consuming a single item, breaking all six call
   sites.
6. **Wire shutdown in `main()`** — split into a new lib module
   `rust/object-cache-srv/src/shutdown_sequence.rs` (exported from
   `lib.rs`) plus its call site in
   `rust/object-cache-srv/src/object_cache_srv.rs`, so the sequence is
   callable from the integration-test crate (see Testing Strategy's
   "Sequenced drain end to end" bullet, and B5 in design review —
   `main()` itself isn't unit-testable, mirroring why
   `cli::validate_write_tuning` was split out, `cli.rs:155-172`). The
   module exposes one async function, `async fn run(remaining: Duration,
   prefetch_worker: JoinHandle<()>, cache: RangeCache, foyer:
   Arc<FoyerBackend>) -> ()` — no budget struct, no per-stage arithmetic,
   and no axum inputs: axum is awaited directly in `main()`, outside `run`
   entirely, so it keeps the full `grace` for its own drain. `run` first
   checks `if remaining < MIN_POST_AXUM_BUDGET { warn!(...); return; }` — a
   `tokio::time::timeout` with too little duration would still poll the
   wrapped future once (Design §3), so this explicit early return, not the
   timeout, is what actually skips the three stages outright whenever
   axum's drain has left too little of the grace period to be worth
   attempting.
   Before either the early-return check above or the timeout below, `run`
   first calls `FoyerBackend::mark_shutting_down()` — so the flag is set on
   every branch, including the early return and a timeout that fires before
   `close()` starts, not only the branch that reaches `close()` itself
   (Design §4). Otherwise, `run` wraps its three stages (prefetch-worker await, fetch-task drain,
   `foyer.close()`, in that order) in one `tokio::time::timeout(remaining,
   ...)` (Design §3), and logs the surviving
   warnings from Design §3 (the elapsed-deadline warning, the
   abandoned-fetch-task count, a prefetch worker `JoinError`, a
   `foyer.close()` error) on whichever branch makes each observable; an
   elapsed deadline is logged and swallowed inside `run`, never
   propagated, so `run` itself returns `()`, not a `Result`. `main()`
   builds `ShutdownFanout` directly (wrapping `wait_for_sigterm()` in an
   `async move` that stamps a shared `signal_at: Arc<OnceLock<Instant>>`
   on fire — see the `main()` snippet above), passes one
   `fanout.receiver()` into the prefetch worker (gating both its channel
   `take_until` and `warm_item`'s per-window check) and one
   `fanout.subscribe()` into `serve_axum_with_graceful_shutdown`, which it
   awaits directly with `?` exactly as today — that call always resolves
   `Ok(())` (Design §3), so the `?` is inert and kept only for
   shape-consistency; retains the
   `Arc<FoyerBackend>` bound before it's passed into `RangeCache::new`; and,
   once axum's own drain returns, computes `remaining` from `signal_at` and
   calls `shutdown_sequence::run` once with it, letting `run` own the three
   post-axum stages.
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
   written from scratch — a shutdown case in `prefetch_tests.rs` (see
   Testing Strategy) exercising `take_until` and the window-loop shutdown
   check, and a new `shutdown_sequence_tests.rs` exercising the sequenced
   drain end to end (`Notify`-backed stub `JoinHandle` for the
   prefetch-worker-await stage, a gated `RangeCache` for the fetch-task
   drain, success path only for `close()` — there is no per-stage budget
   arithmetic left to test), mirroring `#1037`'s
   `graceful_shutdown_tests.rs` pattern rather than only
   threading a never-firing receiver into the other five
   `spawn_prefetch_worker` call sites — and a new `foyer_backend_tests.rs`
   case proving Design §4's load-bearing claim: a
   single `FillHint::Demand` `put()` with no RAM eviction pressure, then
   `close()`, then a fresh `FoyerBackend` reopened over the same directory,
   asserting `get()` still hits. Every existing put->close->get case in that
   file forces eviction first (a tiny `ram_bytes`) or uses
   `FillHint::Prefetch`'s `.force()` writer, so none of them today exercises
   a RAM-resident demand entry surviving *only* because `close()` flushed it.
9. **Docs** — update `mkdocs/docs/admin/object-cache.md`,
   `mkdocs/docs/admin/service-lifecycle.md`,
   `mkdocs/docs/architecture/caching.md`, and
   `rust/object-cache-srv/README.md` to describe the post-axum shutdown
   stages (prefetch drain, fetch-task drain, cache close); see
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
- `rust/public/src/servers/shutdown.rs` — add `ShutdownFanout::receiver()
  -> watch::Receiver<bool>`, a cheap-to-clone accessor for consumers (like
  the prefetch worker) that need a synchronous shutdown check alongside a
  `take_until`-style future derived from the same receiver.
- `rust/object-cache-srv/src/prefetch_queue.rs` — add a single new
  `shutdown: watch::Receiver<bool>` parameter to `spawn_prefetch_worker`
  (three parameters today, `cache`/`queue_capacity`/`worker_concurrency`);
  derive the worker's `take_until` future from it and pass the cloned
  receiver into `warm_item`, checked before each window; the only new
  import is `tokio::sync::watch`. Update the `spawn_prefetch_worker` doc
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
- `rust/object-cache-srv/src/shutdown_sequence.rs` (new) — `pub async fn
  run(remaining: Duration, prefetch_worker, cache, foyer) -> ()`. First calls
  `foyer.mark_shutting_down()`, before either of the checks below, so the
  flag is set on every branch — including the early return and a timeout
  that fires before `close()` starts — not just the branch that reaches
  `close()` itself (Design §4). Then
  checks `if remaining < MIN_POST_AXUM_BUDGET { warn!(...); return; }` — a
  `tokio::time::timeout` with too little duration would still poll the
  wrapped future once, so this explicit early return, not the timeout, is
  what actually skips the three stages outright whenever axum's drain has
  left too little of the grace period to be worth attempting (Design §3).
  Otherwise wraps the three post-axum stages (prefetch-worker await, fetch-task drain,
  `foyer.close()`, in that order) in one `tokio::time::timeout(remaining,
  ...)`, with no budget struct, no per-stage arithmetic, and no axum
  inputs — axum is awaited directly in `main()`, outside `run`; logs the
  elapsed-deadline and abandoned-fetch-count warnings on the timeout branch
  and swallows the `Elapsed` rather than propagating it (Design §3/§4, Step
  6).
- `rust/object-cache-srv/src/object_cache_srv.rs` — build `ShutdownFanout`
  directly, wrapping `wait_for_sigterm()` in an `async move` that stamps a
  `signal_at: Arc<OnceLock<Instant>>` on fire; pass one `fanout.receiver()`
  into the prefetch worker and one `fanout.subscribe()` into
  `serve_axum_with_graceful_shutdown`, awaited directly with `?` exactly as
  today — inertly, since that call always resolves `Ok(())` (Design §3);
  retain the `Arc<FoyerBackend>` bound before it's passed into
  `RangeCache::new` so `close()` has something to call it on (Design §4);
  once axum's drain returns, compute `remaining` from `signal_at` and call
  `shutdown_sequence::run(remaining, ...)` once, letting it own the three
  post-axum stages (Design §3).
- `rust/object-cache/src/foyer_backend.rs` — add a `shutting_down:
  Arc<AtomicBool>`, created before `RamEvictionListener` is built and cloned
  into both it and `FoyerBackend` (mirroring the existing `tags:
  Arc<EvictionTagTable>` sharing), plus a public `FoyerBackend::mark_shutting_down()`
  setter called from the very top of `shutdown_sequence::run`, before its
  `MIN_POST_AXUM_BUDGET` early-return check, not immediately before
  `close()`, so the flag is set on every branch, including ones where
  `close()` never runs; `RamEvictionListener::on_leave` checks the flag and
  skips emission, so `close()`'s full-tier flush — and foyer's drop-time
  close fallback on the branches where `run` never reaches `close()` itself
  — doesn't poison the #1281 eviction gauges (Design §4).
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
  `:597`) to pass a single never-firing receiver from a retained, named
  `watch::Sender` (`let (_shutdown_tx, shutdown_rx) = watch::channel(false);`
  — not `watch::channel(false).1` alone, which drops the `Sender` and makes
  the channel closed rather than never-firing) for the one new parameter;
  add a new shutdown case driven by a real `watch::Sender` (see Testing
  Strategy).
- `rust/object-cache-srv/tests/shutdown_sequence_tests.rs` (new) — a
  sequenced-drain test driving `shutdown_sequence::run` end to end: a
  `Notify`-backed stub `JoinHandle` for the prefetch-worker-await stage, a
  gated `RangeCache` for the fetch-task drain, and the success path only
  for `close()` (no trait seam exists to make `foyer.close()` block);
  asserts stage order as negative liveness (`run` must not complete while
  the stub `JoinHandle` is un-notified, then must not complete while the
  origin gate is closed — the `memory_budget_tests.rs:359-365` idiom) plus
  one observable post-condition standing in for "close ran after the drain":
  `FoyerBackend::ram_entry_count()` (already public, `foyer_backend.rs:356-358`)
  is nonzero while the gate holds the drain open and 0 once `run` returns.
  That nonzero count has to come from a pre-population step, not from the
  gated fetch itself: `CountingStore::with_gate` blocks every ranged GET
  before it returns anything, and `RangeCache` only calls `FoyerBackend::put`
  after the origin GET completes, so while the gate holds, nothing has been
  produced for the cache to store. Before calling `run`, `put()` a
  `FillHint::Demand` entry directly onto the same `Arc<FoyerBackend>` via
  `RangeCacheBackend::put` — the idiom `saturation_tests.rs:128-134` already
  uses in this test crate — independent of the gated fetch, so
  `ram_entry_count()` is nonzero from the start.
  Also asserts overall-deadline behavior — no per-stage budget arithmetic to
  test (Step 6, Testing Strategy). The gated origin-store
  double follows the in-crate precedent at
  `rust/object-cache-srv/tests/prefetch_tests.rs:35-50`'s `CountingStore`/
  `with_gate` (not `range_cache_tests.rs`'s copy, which lives in a
  different crate's test binary and is unreachable from here): either
  duplicate that double directly in the new file, or hoist it into a
  shared sibling module under `rust/object-cache-srv/tests/` (repo
  precedent for that shape: `rust/analytics/tests/test_helpers.rs`,
  `rust/tracing/tests/utils.rs`) and have both `prefetch_tests.rs` and
  `shutdown_sequence_tests.rs` import from it.
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
  that window on the branch where axum's drain actually finishes within
  `grace`. Because axum keeps the *full* `grace` for its own drain (Design
  §3), whatever's left of the single overall deadline for the later stages
  can be small or zero on a busy shutdown — that is the accepted best-effort
  cost of sequencing, not something this plan tries to bound with reserved
  per-stage time (an earlier draft did try to bound it that way; see the
  "One overall deadline" trade-off below for why it was dropped).
  On the *deadline* branch the same gap exists in a different shape: axum
  0.8.9 `tokio::spawn`s a task per accepted connection (`serve/mod.rs:389`),
  so dropping `serve_future` at the deadline kills only the accept loop —
  already-accepted connections survive as orphaned tasks and can still call
  `RangeCache::size`/`spawn_run_fetch` after the fetch drain has already
  started, or after the overall `grace` deadline has already elapsed and the
  whole sequence has been abandoned. Such a task is untracked by anything
  still running and is lost at teardown — an accepted risk, since tracking
  orphaned axum connections themselves is out of scope for this plan, and
  consistent with this plan's overall best-effort framing (Design §3).
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
- **One overall deadline vs. a per-stage budget split vs. reserving a slice
  for `foyer.close()`.** This plan wraps the three post-axum stages
  (prefetch-worker await, fetch-task drain, `foyer.close()`) in a single
  `tokio::time::timeout(remaining, ...)` (Design §3) — axum's own drain is
  awaited directly outside it, keeping the full `grace` — after four
  successive revisions of a per-stage budget split kept producing defects.
  Two alternatives were tried first and rejected:
  - *A per-stage budget split* (fixed percentages, or weighted with axum's
    share capped and `foyer.close()` absorbing the remainder, with or
    without letting an early-finishing stage donate its unused slack to a
    later one) — rejected as accident-prone: it required an absolute floor
    under every stage's `Duration` (`MIN_STAGE_BUDGET`, since a
    zero-duration per-stage budget doesn't skip that stage — `Timeout::poll`
    still polls the wrapped future once before reporting `Elapsed`
    (`tokio-1.52.3/src/time/timeout.rs:211-222`), which for a stage like
    `foyer.close()` can do real, unrecoverable damage on a single poll (see
    the `MIN_POST_AXUM_BUDGET` discussion in Design §3) — and
    `--shutdown-grace-period-seconds` has no minimum-value validation), a
    cap on axum's share, overflow-safe arithmetic splitting `grace` four
    ways, and its own dedicated arithmetic tests — and successive design
    reviews kept finding new holes in it (wrong percentages, floor
    interactions, cap-vs-remainder edge cases). It also broke
    `serve_axum_with_graceful_shutdown`'s existing log lines: they
    interpolate the `grace` value they're given (`shutdown.rs:85,91,112`),
    so handing axum a fraction of `grace` instead of the real value made
    those log lines report the wrong number for the one flag operators
    actually configure (`--shutdown-grace-period-seconds`).
  - *Reserving a guaranteed slice for `foyer.close()`* (letting the other
    three stages draw from a shared pool but ring-fencing a minimum for the
    close step, since it's Design §4's entire point and the last stage in
    line) — rejected because the reservation only ever bound on the branch
    that had slack to spare anyway: on a genuinely saturated shutdown (the
    branch the reservation exists to protect), every stage ahead of `close`
    was already using its full share, so the "guaranteed" slice was never
    actually available to take from them; on the branch where nothing was
    saturated, the reservation changed nothing either, since `close` would
    have gotten enough time regardless.

  The one-deadline design gives up the property both alternatives were
  chasing — that every stage gets *some* minimum time — in exchange for
  removing the arithmetic that kept breaking. The prices paid are stated
  plainly rather than mitigated (Design §3): axum keeps the full `grace`,
  exactly as today, so the later three stages can get little or nothing on
  a saturated shutdown, and cross-restart cache warmth from `foyer.close()`
  is explicitly best-effort (Overview, Design §3/§4).
- **Post-close access from orphaned callers, left unhandled.** On the
  branch where `close()` completes successfully, a `put` lands in RAM and is
  then silently dropped when foyer tries to enqueue it to disk
  (`foyer-storage-0.22.3/src/engine/block/engine.rs:570-574`); no error
  reaches that caller. (On the branch where the overall deadline instead
  abandons `foyer.close()` mid-flush, `active` may not have flipped to
  `false` yet — that happens inside `BlockEngine::close`, which runs after
  `memory.flush()` — so a late `put` there may actually still be written.)
  The expected source on the successful-close branch is an axum connection
  that survived axum's own drain as an orphaned task (see the "Concurrent
  drain" trade-off above) and calls `RangeCache::size`/`spawn_run_fetch`
  after the fetch-task drain has already resolved and `close()` has run.
  `get` is unaffected either way — it still serves from RAM and disk after
  `close()`. Handling this would mean either tracking orphaned axum
  connections past axum's own drain (out of scope, see the "Concurrent
  drain" trade-off above) or having `FoyerBackend` reject `put`s once
  `shutting_down` is set, which is unnecessary extra plumbing for a case
  that just means the state ends up how it would have if the request had
  lost its race with shutdown a moment earlier. No code change is made for
  this.
- **Close-time flush loss is left unmeasured, not approximated.** An
  earlier draft logged a coarse `ram_usage_bytes()`-before vs.
  `disk_stats().write_bytes`-after delta around the close call as a proxy
  for flush loss. It's removed: `ram_usage_bytes()` is logical cache weight
  (`foyer-memory-0.22.3/src/raw.rs:767-769`) while `disk_stats().write_bytes`
  is device I/O bytes (`foyer-storage-0.22.3/src/io/engine/monitor.rs:76-88`),
  and, more fundamentally, `BlockEngine::enqueue` intentionally *skips*
  writing any entry whose `Age` is `Young` (`engine/block/engine.rs:582-589`)
  — the age this repo stamps on most (~90% at the default
  `probation_ratio`) disk->RAM promotions (`promote_if_valid`,
  `foyer_backend.rs:412-417`; see caveat (4) above for the probation-block
  minority that isn't skipped, and for why a demand `put` is unaffected —
  it stamps `Age::Fresh`, not `Age::Young`) — because those bytes are
  already durable on disk. Those already-durable
  bytes count fully against "RAM usage before" and contribute nothing to
  "bytes written after," so on any warm process the comparison reads as a
  shortfall even on a completely lossless close,
  and its message would assert a cause ("silently dropped ... or was not
  evictable") that isn't actually established. Design §4 identifies the two
  real loss paths (queue-level `storage_queue_channel_overflow` and
  buffer-level `storage_queue_buffer_overflow`), and neither is wired into
  foyer's metrics registry in this repo. Rather than assert a cause this
  repo can't observe, or wire that registry (a new mitigation, out of scope
  here), this plan logs only what's genuinely known: the elapsed-deadline
  warning (Design §3) when the overall `grace` timeout catches the close
  call mid-flight, and a `warn` on an `Err` return from `close()` itself. A
  successful, in-time close makes no claim about how many bytes it flushed.
  This is a step back in observability from the earlier draft's
  approximation, accepted because a wrong, byte-counted warning is worse
  than no warning: it would train operators to distrust, or ignore, a
  signal that fires on essentially every clean shutdown of a warm cache.
- **`error!` + 503 on a closed prefetch channel during routine shutdown,
  left unreconciled with the debug/warn logging policy.** Once the prefetch
  worker's channel closes (Design §3), every `/prefetch` POST that arrives
  before axum's own drain finishes hits `handlers.rs:702-704`'s
  `TrySendError::Closed` arm, which logs `error!("prefetch queue worker is
  gone")` and returns 503. Before this plan that arm was unreachable outside
  a unit test, since nothing ever closed the channel in production; after
  this plan it is reachable for essentially the whole drain window on every
  shutdown. `mkdocs/docs/admin/object-cache.md:270` states routine graceful
  degradation should log at `debug`, reserving `warn`/`error` for genuinely
  unexpected conditions — a routine SIGTERM doesn't meet that bar. This plan
  does not change `handlers.rs` to fix the mismatch: doing so is a one-line
  change (drop the `error!` to `debug!`, or leave the log level and only
  reconsider the 503), but it's a client-visible behavior change to a code
  path this plan doesn't otherwise touch, so it's left as accepted, documented
  debt rather than folded into this plan's scope.
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
  also mention in-flight origin fetches and (best-effort) cache persistence,
  since that description becomes incomplete once the later stages exist.
  `:43-54` spells out the drain algorithm and the exact log strings (`drain
  completed`, `grace period of <N>s elapsed with work still in flight`);
  note there that the object cache runs three further stages after axum's
  own drain — prefetch-worker await, fetch-task drain, `foyer.close()` —
  bounded by a second deadline (`remaining`) covering whatever of that
  same `grace` period axum's own drain didn't use (Design §3), and that
  they log their own messages (`origin fetch tasks drained`, plus the
  elapsed-deadline and abandoned-fetch-count warnings). No caveat is needed
  about `:49-51`'s `<N>` meaning something different for object-cache-srv:
  axum keeps the *full*, unmodified `grace` value here exactly as it does
  for this helper's other three callers (Design §3), so the single-deadline
  description at `:49-51` already stays accurate as written and needs no
  change on that point.
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
- `mkdocs/docs/admin/object-cache.md:270-279` ("Tuning the write path")
  already documents the foyer write-throttle overflow WARN as the
  operator-facing landing spot for "a warning fired during shutdown/write
  tuning, what does it mean." Add the two shutdown-sequence WARNs that
  survive Part 1's simplification to this same section, at the same `warn`
  level (`:270`'s policy — routine at `debug`, unexpected at `warn`/`error`
  — applies here since an elapsed shutdown deadline and an abandoned close
  are both unexpected, not routine): (1) the elapsed-grace-period warning
  (fires when the overall `grace` deadline catches any stage — prefetch
  await, fetch-task drain, or `foyer.close()` — still running, Design §3),
  and (2) the abandoned-fetch-task-count warning that accompanies it when
  `outstanding_fetch_tasks() > 0`. Cross-reference `object-cache.md:40`/`:62`
  ("same-format restarts reuse the store warm") from wherever these are
  documented: if the elapsed-deadline warning fires while `foyer.close()`
  was the stage running, the flush was abandoned and the next restart will
  *not* reuse that warmth — the direct, documented consequence of
  best-effort persistence (Design §3).
- `mkdocs/docs/admin/object-cache.md:104-123` (Prefetch section) documents
  the bounded queue and its load-shedding counters but says nothing about
  shutdown; add a note that on `SIGTERM` the worker stops admitting new
  items once the drain begins and any queued backlog is abandoned (Step 5).
- `mkdocs/docs/admin/object-cache.md:156-170` (In-process L1 cache) and
  `mkdocs/docs/architecture/caching.md` (after `### What is intentionally not
  cached in L1`, `:107`, or after the eviction-policy rationale that closes
  out `### L1 and L2 are the same subsystem` at `:65` (`## Read-path
  mechanics` itself doesn't start until `:75`) —
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
    an async context panics. The precedent for *owning and dropping* a
    `Runtime` from a plain `#[test]` is
    `rust/analytics/tests/async_span_tests.rs:38-60` (explicit
    `drop(runtime)` at `:60`), but that test drops its runtime only *after*
    `block_on` has already completed, so it doesn't by itself show how to
    drop a runtime while a spawned task is still pending;
    `telemetry_tests.rs:70-74` doesn't drop its runtime early either and
    isn't an example of this either. This test needs its own explicit
    synchronization seam: `rt.block_on(async { spawn the fetch behind a
    `CountingStore::with_gate` double, never releasing the gate; loop
    `tokio::task::yield_now().await` until `counting.get_range_count() >= 1`,
    confirming the origin GET — and with it the spawned fetch task — has
    actually started })`, then `drop(rt)` *outside* the `block_on` call, on
    the still-gated fetch, to force the drop-without-poll-to-completion this
    test is about. Reading the captured log
    requires a new helper with no precedent in the repo (see Step 8);
    existing in-memory-sink assertions in this codebase all read
    `metrics_blocks` (`saturation_tests.rs:33,50`), not `log_blocks` — the
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
    find nothing. A second, independent way to silently break the same
    helper: adding a `properties:` argument to either `warn!` call routes it
    through `log_tagged` instead (`tracing/src/macros.rs:266-279`), which
    pushes a `TaggedLogString`, not a `LogStringEvent`
    (`dispatch.rs:759-764`) — a `LogStringEvent` matcher would miss that too.
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
  six existing `spawn_prefetch_worker` call sites thread a single
  never-firing receiver from a retained, named `watch::Sender` into the new
  `shutdown` parameter (Step 5) — `let (_shutdown_tx, shutdown_rx) =
  watch::channel(false);`, **not** `watch::channel(false).1` alone, which
  drops the `Sender` and makes the channel closed (causing `wait_for`/`take_until`
  to resolve immediately, terminating the worker before it consumes
  anything) rather than never-firing — so none of the six exercises
  `take_until` or the window-loop check; only an optional manual SIGTERM
  check does. Following #1037's `graceful_shutdown_tests.rs` precedent
  (driving drain from a controllable signal rather than real signals), add
  a case that: spawns the worker with a real `watch::Sender<bool>`-backed
  receiver, sends an item, flips the sender mid-flight, and asserts (a) the
  worker's `JoinHandle` completes, and (b) an item sent after the flip is
  not admitted (e.g. `tx.send(...)` returns an error, or the item is
  observably never warmed).
- **Sequenced drain end to end** (new `shutdown_sequence_tests.rs`): this,
  not the `prefetch_tests.rs` case above, is what exercises
  `shutdown_sequence::run` end to end (Step 6's rationale for extracting
  `shutdown_sequence.rs` for testability). Per Part 1 there is no per-stage
  budget arithmetic left to test — this exercises stage *order* and the
  overall-deadline behavior instead. The three post-axum stages need
  different seams, not one uniform stand-in, since two of them have no
  trait seam at all: use a `Notify`-backed stub `JoinHandle` for the
  prefetch-worker-await stage (a real `JoinHandle` from a task that awaits
  a test-controlled `Notify`); a gated origin store (`CountingStore::with_gate`,
  following the in-crate precedent at
  `rust/object-cache-srv/tests/prefetch_tests.rs:35-50` — not
  `range_cache_tests.rs`'s copy, which lives in a different crate's test
  binary and is unreachable from here; duplicate the double in this new
  file or hoist it into a shared sibling module, per the Files to Modify
  entry above) driving a real `RangeCache` for the fetch-task-drain stage,
  since `RangeCache` has no trait seam to substitute; and the success path
  only for `foyer.close()`, since `Arc<FoyerBackend>` has no trait seam
  either (`close()` and `mark_shutting_down()` are both inherent, and
  `RangeCacheBackend` has no `close` method) — there is no way to make a
  test drive `close()` into a controllable delay, so its *start* can't be
  observed directly. Assert the documented priority order as negative
  liveness instead, the `memory_budget_tests.rs:359-365` idiom: `run` must
  not complete while the stub `JoinHandle` sits un-notified, and, once it's
  notified, must not complete while the origin gate keeps the fetch-task
  drain open. Pair that with one observable post-condition standing in for
  "close ran only after the drain": `FoyerBackend::ram_entry_count()`
  (already public, `foyer_backend.rs:356-358`) is nonzero while the gate
  holds the drain open, and 0 once `run` returns (`close()`'s eviction-to-
  zero sweep is what drives it there). The gated fetch itself cannot supply
  that nonzero count: `CountingStore::with_gate` blocks every ranged GET
  before it returns, and `RangeCache` only calls `FoyerBackend::put` after
  the origin GET completes, so while the gate holds nothing has been
  produced to store yet. Pre-populate a separate entry before calling `run`
  instead: a direct `RangeCacheBackend::put(key, bytes, FillHint::Demand)`
  call on the same `Arc<FoyerBackend>` (the idiom
  `rust/object-cache-srv/tests/saturation_tests.rs:128-134` already uses in
  this test crate), independent of the gated fetch, so `ram_entry_count()`
  is nonzero from the start. Also assert that the whole sequence
  is bounded by the `remaining` duration passed to `shutdown_sequence::run`
  (a short `remaining` with the prefetch stand-in never notifying should
  time out rather than hang).
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
