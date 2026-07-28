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
run in sequence, bounded by whatever of that grace period axum didn't use —
the prefetch worker is aborted and joined so it can no longer spawn fetch
work, then the fetch-task drain runs, then the cache close. That order is
forced by producer-before-consumer correctness, not by a priority ranking;
on a saturated shutdown the later stages can get little or no time, so
cross-restart warmth from the close step is best-effort, not guaranteed.

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
/// currently in flight to origin, for the shutdown path's
/// abandoned-fetch-count warning.
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

### 3. Stop the prefetch producer, then run the shutdown sequence under one overall deadline
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
prefetch worker — from spawning new fetch work before it waits for
`outstanding_fetch_tasks()` to reach zero.

**Stop the prefetch producer by aborting its task, then joining it.** No
shutdown signal is threaded into `prefetch_queue.rs` at all. `main()` retains
the worker's `JoinHandle` (today it's bound to `_prefetch_worker` and never
looked at again), and the first stage of the shutdown sequence calls `abort()`
on it and then awaits it. Awaiting a `JoinHandle` after `abort()` observes the
task's future as already dropped (`tokio-1.52.3/src/task/mod.rs:146-150`,
`tokio-1.52.3/src/runtime/task/join.rs:25-27`), which is exactly the guarantee
the fetch-task drain needs: once that await returns, the worker can no longer
pull an item from the channel, and no `warm_item` call can spawn one more
`spawn_run_fetch` task. A `warm_item` future in flight is dropped mid-stream,
abandoning the windows it hadn't started yet — the same outcome a
shutdown-signal check between windows would have produced; the
`spawn_run_fetch` tasks it already dispatched are separate detached tasks,
each carrying its own `FetchTaskGuard`, so they are still counted and still
drained by the next stage. `abort()` is synchronous and the join then resolves
promptly, so this stage costs essentially none of the shutdown budget.

This is why the worker's handle must be joined *before* the fetch-task drain
rather than discarded: without it there is a window where `warm_item`'s
in-flight window fetch has been dispatched (tracked by `FetchTaskGuard`) but
`join_prefetch` hasn't yet reached the `remove_entry`/`guard.disarm()` tail of
the spawned fetch task, and the *next* window's future isn't even constructed
yet — so `outstanding_tasks` can legitimately read 0 between two windows of
the same still-running `warm_item` call. Abort-then-join establishes that the
last prefetch producer has actually exited before
`wait_for_fetch_tasks_drain()` is trusted to mean "no more origin GETs will be
spawned."

`warm_item`, `spawn_prefetch_worker`, and `ShutdownFanout` are therefore all
left untouched by this plan. In particular `main()` keeps today's shape: it
passes the shutdown-signal future straight into
`serve_axum_with_graceful_shutdown`, which builds its own `ShutdownFanout`
internally (`shutdown.rs:86`), so no fanout is constructed in `main()` and no
new accessor is added to the shared `rust/public` crate. The only change to
the signal itself is the `signal_at` stamp that `remaining` needs (see the
`main()` snippet below). `spawn_saturation_monitor` likewise takes no shutdown
parameter and is left unchanged — it simply dies with the runtime, like today,
and its `prefetch_tx` clone is irrelevant either way now that `abort()`, not
channel closure, is what stops the worker.

Aborting the worker drops its `ReceiverStream`, which closes `prefetch_tx`'s
receiving end, so a `/prefetch` POST arriving after that point hits
`handlers.rs:702-704`'s `TrySendError::Closed` arm (`error!` + 503). Because
the abort happens only *after* axum's own drain has returned, the only
requests that can still reach that arm are ones served by connections that
outlived the drain as orphaned tasks (see the "Concurrent drain" trade-off) —
a narrow enough window that the log-level mismatch noted in Trade-offs is
close to unobservable.

This does not wait for the prefetch queue's backlog to empty — a full queue at
shutdown is simply abandoned, exactly as it would be today.
`Priority::Prefetch` and `Priority::Demand` fetch tasks that are already
running are **not** distinguished by the drain: both are tracked by the same
`outstanding_tasks` counter and are equally drained if they finish before the
deadline, or equally abandoned (and logged) if they don't.

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

Four stages need to run in that order — axum drain, prefetch-worker
abort-and-join, fetch-task drain, `foyer.close()` — and that order is forced
by producer-before-consumer correctness, not by any priority ranking. By the
time the drain stage runs, no joiner is alive: a live request stays pending
until the coalesced fetch completes (`handlers.rs:390` awaits the first
chunk; `mod.rs:426` -> `fetch.rs:128` `join_demand` ->
`scheduler.rs:129-141` awaits the watch channel) and axum's graceful shutdown
waits for connections *and* streamed response bodies
(`axum-0.8.9/src/serve/mod.rs:296-303,404-410`; hyper `http1.rs:127-137`),
while the only non-axum joiners live in the prefetch worker, stopped by the
stage before. So the drain stage is not about bytes a caller is waiting for —
it exists only to let in-flight origin GETs land in the cache, best-effort,
the same category as `foyer.close()` itself. Four successive
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

// `remaining`, not `grace`, is what bounds these three stages, since axum
// may have already spent some (or all) of `grace` on its own drain. `run`
// returns `()`, not a `Result`: an elapsed deadline is logged at `warn` and
// swallowed inside `run`, never propagated, so this call is `.await;`, not
// `.await?`.
shutdown_sequence::run(remaining, prefetch_worker, cache_for_drain, foyer).await;
```

```rust
// inside shutdown_sequence::run (`async fn run(...) -> ()`):
if tokio::time::timeout(remaining, async {
    // Stage 1: stop the prefetch producer. `abort()` is synchronous;
    // awaiting the handle afterwards observes the worker future as already
    // dropped, so it can no longer spawn fetch tasks. `JoinHandle::Output`
    // is `Result<(), JoinError>`, `#[must_use]`, so it must be handled --
    // and after an `abort()` an `Err` is the *expected* outcome
    // (cancellation), so only a real panic is worth a warning.
    prefetch_worker.abort();
    match prefetch_worker.await {
        Ok(()) => {}
        Err(e) if e.is_panic() => warn!("prefetch worker panicked: {e:#}"),
        Err(_) => {} // cancelled by the abort above -- expected
    }
    // Stage 2: let in-flight origin GETs land in the cache.
    cache.wait_for_fetch_tasks_drain().await;
    // Stage 3: persist the RAM tier. `mark_shutting_down()` is called here,
    // immediately before the close, because that is the only place it is
    // needed (Design §4).
    foyer.mark_shutting_down();
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
`remaining` is `Duration::ZERO`. That is not specially guarded against:
`Timeout::poll` polls the wrapped future unconditionally *before* checking
its deadline (`tokio-1.52.3/src/time/timeout.rs:211-222`), so
`tokio::time::timeout(remaining, ..)` still polls the wrapped future once
*regardless of how small (or zero) `remaining` is* — enough for the
abort-and-join stage or `wait_for_fetch_tasks_drain()` to resolve
immediately if there's nothing to wait for, and, worse, for
`foyer.close()`'s first poll to reach `closed.fetch_or(true,
Ordering::Relaxed)` (`foyer-0.22.3/src/hybrid/cache.rs:319-321`) *before*
its first await point — latching `closed` and then abandoning the future,
which permanently disables the drop-time `impl Drop for Inner` close
fallback the Overview describes as today's baseline. On a saturated
shutdown this is accepted, best-effort loss, not something this design
adds a threshold to protect against: the timeout takes its one poll
regardless of how little of `grace` axum's drain left behind, and the three
post-axum stages simply get whatever `remaining` gives them — one poll
each, in the worst case. This is a best-effort cache, not a
correctness-critical store: the cost of that is some cold reads after
restart, not lost or corrupted data, and the Overview already describes the
drop-time close fallback itself as a long shot.
Cross-restart cache warmth — the benefit Design §4 adds — is therefore
explicitly **best-effort**, never a guarantee, and the Overview and this
section should be read that way.

If the post-axum timeout fires, the `async` block wrapping the three stages
is dropped mid-stage, and whatever it was doing is simply abandoned — the
same fate every in-flight future has today at process-exit runtime teardown.
Concretely:
- It effectively cannot fire during the abort-and-join stage: `abort()` is
  synchronous and the join then resolves as soon as the runtime drops the
  worker future, so this stage consumes no meaningful part of `remaining`.
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
- If the post-axum timeout elapses, log once at `warn` (an elapsed shutdown
  deadline is not routine — the same level
  `serve_axum_with_graceful_shutdown`'s own deadline arm already uses) that
  the grace period elapsed before the sequence finished, so whichever stage
  was still running was abandoned.
- On that same branch, if `cache.outstanding_fetch_tasks()` — read from a
  clone held outside the `async` block, since the block itself was dropped
  — is still nonzero, log a second `warn` reporting that count. This is the
  one per-stage-shaped warning worth keeping: "how many origin fetches were
  abandoned" is the one number an operator can actually act on.
- A `prefetch_worker` `JoinError` that reports a *panic* (`e.is_panic()`) and
  an `Err` from `foyer.close()` both stay `warn`, exactly as before, on the
  branch where the sequence finishes within `remaining` (so the block is
  still alive to log them). A `JoinError` from the `abort()` itself is the
  expected outcome of that stage and is not logged.
- Successful completion of each stage stays an `info!`, as today
  (`origin fetch tasks drained`, `foyer cache closed`) — this isn't the
  routine/unexpected distinction the `debug`/`warn` policy is about.

```rust
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use micromegas::servers::shutdown::{serve_axum_with_graceful_shutdown, wait_for_sigterm};
use micromegas_object_cache_srv::shutdown_sequence;
...
let grace = args.common.grace();

// Stamped the instant the shutdown signal fires, so `remaining` below can
// measure from when shutdown actually started rather than from process
// boot. Wrapping `wait_for_sigterm()` this way means the stamp happens
// exactly once, inside the same future `serve_axum_with_graceful_shutdown`
// hands to its own internal `ShutdownFanout` -- no fanout is built here.
let signal_at: Arc<OnceLock<Instant>> = Arc::new(OnceLock::new());
let shutdown_signal = {
    let signal_at = signal_at.clone();
    async move {
        wait_for_sigterm().await;
        let _ = signal_at.set(Instant::now());
    }
};

// Unchanged call, except that the handle is retained instead of bound to
// `_prefetch_worker`: `shutdown_sequence::run` aborts and joins it.
let (prefetch_tx, prefetch_worker) = spawn_prefetch_worker(
    cache.clone(),
    args.prefetch_queue_capacity,
    args.prefetch_worker_concurrency,
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
    shutdown_signal,
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

`shutdown_sequence::run` owns the single `tokio::time::timeout(remaining,
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
  persist, independent of any write throttle; (2) `close()`'s
  eviction-to-zero sweep (`foyer-memory-0.22.3/src/raw.rs:119-137`'s
  `evict(0, ...)`) pops from the eviction container and stops on the first
  `None`, so entries still pinned by an outstanding `get`-derived handle are not
  swept — the sweep is therefore not *every* RAM entry, only every
  evictable one; and (3) most, but not all, entries already on disk are
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
  Caveat (1)'s `storage_queue_channel_overflow`
  counter is not wired into this repo's telemetry — `FoyerBackend`
  never calls foyer's `with_metrics_registry`, and the counter is not
  reflected in `BackendDiskStats`/`backend_disk_stats` — so today this loss
  is invisible, and this plan does not attempt to measure it (see the
  matching Trade-offs entry, "Close-time flush loss is left unmeasured, not
  approximated" — an earlier draft logged a coarse RAM-usage-before vs.
  disk-write-bytes-after delta as a proxy, and that heuristic is removed:
  the two quantities are incommensurable, and caveat (3) above means the
  comparison reads as a shortfall on nearly every clean shutdown of a warm
  cache even when nothing was actually lost). The new `foyer_backend_tests.rs`
  case below (Step 6) proves the close flush works on the unpinned,
  unsaturated path; it does not exercise any of these three caveats.

```rust
// Set immediately before the close, the only place it is needed, so
// `RamEvictionListener::on_leave` (see below) can tell this flush apart from
// capacity-driven thrashing.
foyer.mark_shutting_down();
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
capacity thrashing. The flag is therefore set immediately before
`foyer.close()`, which is the only place it is needed: on the branch where the
overall deadline fires before that call is reached, nothing is flushed and
nothing is emitted, so there is nothing to suppress. (`impl Drop for Inner`
spawns `close_inner` onto the `Spawner` captured at store-build time
(`foyer-storage-0.22.3/src/store.rs:443`) and discards the handle; when
`main()` returns, the runtime is dropped and that queued-but-unpolled task is
cancelled before `memory.flush()` — itself an await on `store.wait()`,
`cache.rs:245` — ever runs. That is the same "racing and typically losing"
teardown the Overview describes, and it emits no eviction events.) Making
`on_leave`'s check possible requires more than a field on `FoyerBackend`:
`RamEvictionListener` is a separate struct built and moved into the foyer builder (as an
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
`FoyerBackend::mark_shutting_down()` — called immediately before
`foyer.close()` (see the snippet above and the `run` snippet in
Design §3). `mkdocs/docs/admin/object-cache.md`
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
   `scheduler.rs:2`). Initialize both new fields in `FetchScheduler::new`
   (`scheduler.rs:180-187`), whose exhaustive struct literal has no `Default`
   impl to fall back on.
3. **Wire the guard into both spawn sites** — `rust/object-cache/src/range_cache/fetch.rs`
   (`spawn_run_fetch`) and `rust/object-cache/src/range_cache/mod.rs`
   (`RangeCache::size`'s owner branch): construct `FetchTaskGuard` *before*
   `tokio::spawn`, then move it into the async block. `fetch.rs`'s existing
   `use super::scheduler::{...}` list (`fetch.rs:19-22`) doesn't include
   `FetchScheduler` — add it there; `mod.rs:22` already imports it, so only
   `fetch.rs` needs the new import.
4. **Expose on `RangeCache`** — `rust/object-cache/src/range_cache/mod.rs`:
   `outstanding_fetch_tasks()`, `wait_for_fetch_tasks_drain()`.
5. **Wire shutdown in `main()`** — split into a new lib module
   `rust/object-cache-srv/src/shutdown_sequence.rs` (exported from `lib.rs`)
   plus its call site in
   `rust/object-cache-srv/src/object_cache_srv.rs`. The split is not for
   testability — no test drives `run` (see Testing Strategy) — it just keeps
   `main()` thin and puts the sequence where the rest of this crate's logic
   already lives, in the lib rather than the binary. The
   module exposes one async function, `async fn run(remaining: Duration,
   prefetch_worker: JoinHandle<()>, cache: RangeCache, foyer:
   Arc<FoyerBackend>) -> ()` — no budget struct, no per-stage arithmetic,
   and no axum inputs: axum is awaited directly in `main()`, outside `run`
   entirely, so it keeps the full `grace` for its own drain. `run` wraps its
   three stages in one `tokio::time::timeout(remaining, ...)` (Design §3):
   `prefetch_worker.abort()` followed by awaiting the handle (warning only on
   `e.is_panic()`, since a cancellation `JoinError` is the expected outcome of
   the abort), then `cache.wait_for_fetch_tasks_drain()`, then
   `foyer.mark_shutting_down()` immediately followed by `foyer.close()`
   (Design §4). It logs the surviving
   warnings from Design §3 (the elapsed-deadline warning, the
   abandoned-fetch-task count, a panicking prefetch worker, a
   `foyer.close()` error) on whichever branch makes each observable; an
   elapsed deadline is logged and swallowed inside `run`, never
   propagated, so `run` itself returns `()`, not a `Result`. `main()` keeps
   today's shape otherwise: it wraps `wait_for_sigterm()` in an `async move`
   that stamps a shared `signal_at: Arc<OnceLock<Instant>>` on fire and passes
   that future straight into `serve_axum_with_graceful_shutdown` (which builds
   its own `ShutdownFanout` internally), awaited directly with `?` exactly as
   today — that call always resolves `Ok(())` (Design §3), so the `?` is inert
   and kept only for shape-consistency; retains the prefetch worker's
   `JoinHandle` (today `_prefetch_worker`) and the
   `Arc<FoyerBackend>` bound before it's passed into `RangeCache::new`; and,
   once axum's own drain returns, computes `remaining` from `signal_at` and
   calls `shutdown_sequence::run` once with it, letting `run` own the three
   post-axum stages.
6. **Tests** — see Testing Strategy below: a new panic-on-`get_opts`
   `ObjectStore` test double (panicking on the ranged-GET branch) for the
   panic-log half, the fetch-task drain tests, and a new
   `foyer_backend_tests.rs`
   case proving Design §4's load-bearing claim: a
   single `FillHint::Demand` `put()` with no RAM eviction pressure, then
   `close()`, then a fresh `FoyerBackend` reopened over the same directory,
   asserting `get()` still hits. Every existing put->close->get case in that
   file forces eviction first (a tiny `ram_bytes`) or uses
   `FillHint::Prefetch`'s `.force()` writer, so none of them today exercises
   a RAM-resident demand entry surviving *only* because `close()` flushed it.
7. **Docs** — update `mkdocs/docs/admin/object-cache.md`,
   `mkdocs/docs/admin/service-lifecycle.md`,
   `mkdocs/docs/architecture/caching.md`, and
   `rust/object-cache-srv/README.md` to describe the post-axum shutdown
   stages (prefetch-worker abort, fetch-task drain, cache close); see
   Documentation below.
8. **Changelog** — append a bullet to the existing `## Unreleased` →
    `**Caching:**` subsection (`CHANGELOG.md:32`) noting the grace period
    now also drains in-flight origin fetches, not just HTTP connections,
    ending the bullet with `(#1291)` per this file's existing convention
    (every `**Caching:**` bullet ends with an issue reference, e.g. `:33`,
    `:37`).
9. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and the
    object-cache / object-cache-srv test suites.

## Files to Modify
- `rust/object-cache/src/range_cache/scheduler.rs` — accurate panic/shutdown
  log; `FetchTaskGuard` + counter/`Notify` on `FetchScheduler`.
- `rust/object-cache/src/range_cache/fetch.rs` — construct + hold
  `FetchTaskGuard` around `spawn_run_fetch`.
- `rust/object-cache/src/range_cache/mod.rs` — construct + hold
  `FetchTaskGuard` around `size()`'s owner branch; expose
  `outstanding_fetch_tasks()` / `wait_for_fetch_tasks_drain()`.
- `rust/object-cache-srv/src/saturation_monitor.rs` — doc comment only
  (`:157-159`): drop the clause claiming the returned handle parallels
  `spawn_prefetch_worker`'s detached worker, which is no longer true now that
  the prefetch handle is retained, aborted, and joined. The sampler itself is
  unchanged — no `shutdown` parameter, no new gauge (Design §3).
- `rust/object-cache-srv/src/lib.rs` — declare the new `shutdown_sequence`
  module alongside the crate's other modules, keeping `main()` thin (Design
  §3/Step 5).
- `rust/object-cache-srv/src/shutdown_sequence.rs` (new) — `pub async fn
  run(remaining: Duration, prefetch_worker, cache, foyer) -> ()`. Wraps the
  three post-axum stages in one `tokio::time::timeout(remaining, ...)`:
  `prefetch_worker.abort()` then awaiting the handle, the fetch-task drain,
  then `foyer.mark_shutting_down()` immediately followed by `foyer.close()`.
  No budget struct, no per-stage arithmetic, and no axum
  inputs — axum is awaited directly in `main()`, outside `run`; logs the
  elapsed-deadline and abandoned-fetch-count warnings on the timeout branch
  and swallows the `Elapsed` rather than propagating it (Design §3/§4, Step
  5).
- `rust/object-cache-srv/src/object_cache_srv.rs` — wrap `wait_for_sigterm()`
  in an `async move` that stamps a `signal_at: Arc<OnceLock<Instant>>` on
  fire and pass that future straight into
  `serve_axum_with_graceful_shutdown` (which builds its own `ShutdownFanout`
  internally), awaited directly with `?` exactly as
  today — inertly, since that call always resolves `Ok(())` (Design §3);
  retain the prefetch worker's `JoinHandle` (today `_prefetch_worker`) and
  the `Arc<FoyerBackend>` bound before it's passed into
  `RangeCache::new` so `close()` has something to call it on (Design §4);
  once axum's drain returns, compute `remaining` from `signal_at` and call
  `shutdown_sequence::run(remaining, ...)` once, letting it own the three
  post-axum stages (Design §3).
- `rust/object-cache/src/foyer_backend.rs` — add a `shutting_down:
  Arc<AtomicBool>`, created before `RamEvictionListener` is built and cloned
  into both it and `FoyerBackend` (mirroring the existing `tags:
  Arc<EvictionTagTable>` sharing), plus a public `FoyerBackend::mark_shutting_down()`
  setter called immediately before `foyer.close()` in
  `shutdown_sequence::run`; `RamEvictionListener::on_leave` checks the flag and
  skips emission, so `close()`'s full-tier flush doesn't poison the #1281
  eviction gauges (Design §4).
- `rust/object-cache/tests/range_cache_tests.rs` — drain/panic-distinction
  regression tests; new panic-on-`get_opts` `ObjectStore` double (panicking
  on the ranged-GET branch).
- `rust/object-cache/tests/foyer_backend_tests.rs` — new case: a
  `FillHint::Demand` `put()` with no RAM eviction pressure, `close()`, reopen
  a `FoyerBackend` over the same directory, assert `get()` hits (Design §4).
- `mkdocs/docs/admin/object-cache.md` — shutdown-behavior note on the grace
  period; note that the eviction gauges go quiet during the close-time flush.
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
  (prefetch-worker abort, fetch-task drain, `foyer.close()`) in a single
  `tokio::time::timeout(remaining, ...)` (Design §3) — axum's own drain is
  awaited directly outside it, keeping the full `grace` — after four
  successive revisions of a per-stage budget split kept producing defects.
  Two alternatives were tried first and rejected:
  - *A per-stage budget split* (fixed percentages, or weighted with axum's
    share capped and `foyer.close()` absorbing the remainder, with or
    without letting an early-finishing stage donate its unused slack to a
    later one) — rejected as accident-prone: a zero-duration per-stage
    budget doesn't skip that stage — `Timeout::poll` still polls the wrapped
    future once before reporting `Elapsed`
    (`tokio-1.52.3/src/time/timeout.rs:211-222`), which for a stage like
    `foyer.close()` can do real, unrecoverable damage on a single poll (see
    Design §3) — so this approach needed a cap on axum's share, overflow-safe
    arithmetic splitting `grace` four ways, and its own dedicated arithmetic
    tests — and successive design reviews kept finding new holes in it
    (wrong percentages, floor interactions, cap-vs-remainder edge cases). It
    also broke
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
  `foyer_backend.rs:412-417`; see caveat (3) above for the probation-block
  minority that isn't skipped, and for why a demand `put` is unaffected —
  it stamps `Age::Fresh`, not `Age::Young`) — because those bytes are
  already durable on disk. Those already-durable
  bytes count fully against "RAM usage before" and contribute nothing to
  "bytes written after," so on any warm process the comparison reads as a
  shortfall even on a completely lossless close,
  and its message would assert a cause ("silently dropped ... or was not
  evictable") that isn't actually established. Design §4 identifies the real
  loss path (the queue-level `storage_queue_channel_overflow` drop), and it is
  not wired into
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
- **`error!` + 503 on a closed prefetch channel, left as-is.** Aborting the
  prefetch worker closes its channel, making `handlers.rs:702-704`'s
  `TrySendError::Closed` arm (`error!("prefetch queue worker is gone")` +
  503) reachable in production for the first time. Because the abort happens
  only after axum's own drain has returned, the only requests that can reach
  it come from connections that outlived that drain, so the window is narrow
  and the mismatch with `mkdocs/docs/admin/object-cache.md:270`'s
  routine-at-`debug` policy is near-moot. `handlers.rs` is left untouched.
- **Not modifying `serve_axum_with_graceful_shutdown` itself.** It's shared
  by `ingestion.rs`, `analytics-web-srv/web_server.rs`, and
  `object_cache_srv.rs` itself (three production callers); the other two have no
  detached-task concept, so adding one there would be dead complexity for
  both of them. object-cache-srv keeps calling the helper exactly as today and
  simply runs its own extra stages after it returns, so nothing shared has to
  learn about detached fetch tasks.

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
  own drain — prefetch-worker abort, fetch-task drain, `foyer.close()` —
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
  this plan keeps to this same section, at the same `warn`
  level (`:270`'s policy — routine at `debug`, unexpected at `warn`/`error`
  — applies here since an elapsed shutdown deadline and an abandoned close
  are both unexpected, not routine): (1) the elapsed-grace-period warning
  (fires when the overall `grace` deadline catches the fetch-task drain or
  `foyer.close()` still running, Design §3),
  and (2) the abandoned-fetch-task-count warning that accompanies it when
  `outstanding_fetch_tasks() > 0`. Cross-reference `object-cache.md:40`/`:62`
  ("same-format restarts reuse the store warm") from wherever these are
  documented: if the elapsed-deadline warning fires while `foyer.close()`
  was the stage running, the flush was abandoned and the next restart will
  *not* reuse that warmth — the direct, documented consequence of
  best-effort persistence (Design §3).
- `mkdocs/docs/admin/object-cache.md:104-123` (Prefetch section) documents
  the bounded queue and its load-shedding counters but says nothing about
  shutdown; add a note that on `SIGTERM` the worker is aborted once axum's
  drain finishes, so any queued backlog — and any partially warmed key — is
  abandoned (Design §3).
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
- **Panic log distinction** (`range_cache_tests.rs`): only the panic branch is
  unit-tested. Drive a fetch that panics (the new panic-on-`get_opts`
  `ObjectStore` test double from Step 6, panicking on the ranged-GET
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
  The shutdown branch's wording is left unit-untested: nothing public survives
  that path (`InFlight`, `FulfillGuard`, and `FetchScheduler` are all
  `pub(super)`, the joiner future dies with the runtime, and no in-repo helper
  extracts message text from `MemSinkState::log_blocks` — both existing
  accessors are count-only). Verifying which of two literals a `Drop` impl
  picks is not worth a from-scratch log-text helper plus a hand-built runtime
  dropped mid-task; the manual SIGTERM check below covers it.
- **Drain waits for outstanding tasks**: drive `RangeCache::prefetch_blocks`
  (public, and — unlike `get_range`, which resolves `size()` via a separate
  HEAD before calling `fetch_blocks` — takes `file_size` directly, so no
  HEAD call happens at all; that HEAD, when it does happen, is itself a
  tracked task whose guard can transiently overlap the block fetch's guard,
  making an `== 1` assertion racy) against a `CountingStore::with_gate` origin double
  (existing infrastructure, `range_cache_tests.rs:65`/`:91`, same pattern as
  the gated fetches at `:666`/`:959` — not `:739`/`:768`/`:819`, which use
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
  Step 6): a single `FillHint::Demand` `put()` with no RAM eviction pressure
  (a `ram_bytes` generous enough that nothing evicts), `close()`, then a
  fresh `FoyerBackend::new_with_shards` reopened over the same directory,
  asserting `get()` still hits. This is the one case load-bearing for Design
  §4's persistence claim that no existing test covers: every current
  `Demand` put->`close()`->`get` case forces RAM eviction first with a tiny
  `ram_bytes` (its own comment notes "the disk write is triggered by memory
  eviction, not by insert itself"), and every close-without-eviction case
  uses `FillHint::Prefetch`'s `.force()` storage-only writer instead.
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
