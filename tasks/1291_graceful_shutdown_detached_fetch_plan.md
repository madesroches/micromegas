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
foyer's write pipeline has its flusher torn down with the runtime before it
can reach disk. This plan (1) makes the log message accurate by
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
also into `warm_item` itself. Gating only the channel is not enough:
`warm_item` pulls its `BlockWindows` through `.buffered(WINDOW_CONCURRENCY)`
with `WINDOW_CONCURRENCY = 1` (`prefetch_queue.rs:21`), so window N+1's
future isn't even created until window N resolves — a guaranteed point,
once per window, where a `warm_item` call that's already "started" can still
spawn a brand-new `spawn_run_fetch` task after the drain below has already
observed zero outstanding tasks and returned. The worker wraps its receiver
in `.take_until(shutdown)` so it stops pulling *new* items the moment the
signal fires, and `warm_item` checks a shared `shutting_down: Arc<AtomicBool>`
flag before pulling its next window so it stops between windows instead of
only between items (any window already dispatched still runs to completion
and is tracked via `FetchTaskGuard` like demand fetches). Since the flag
must be checked synchronously from every concurrent `warm_item` call rather
than awaited once, `spawn_prefetch_worker` takes a *second* shutdown future
(a second `fanout.subscribe()` from the call site) driving a tiny detached
task that flips the flag — cheaper to clone into every `warm_item` call than
the shutdown future itself. The sampler exits its loop on its own subscribe
call, which drops its own `prefetch_tx` clone instead of holding it forever:

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

**The worker's `JoinHandle` must be awaited before the fetch-task drain
starts**, not discarded. `main()` currently binds it to `_prefetch_worker`
and never looks at it again; that leaves a window where `warm_item`'s
in-flight window fetch has been dispatched (tracked by `FetchTaskGuard`) but
`join_prefetch` hasn't yet reached the `remove_entry`/`guard.disarm()` tail
of the spawned fetch task, and the *next* window's future isn't even
constructed yet — so `outstanding_tasks` can legitimately read 0 between two
windows of the same still-running `warm_item` call. Awaiting
`_prefetch_worker` first (bounded by the same shared `remaining` deadline)
establishes that the last prefetch producer has actually exited before
`wait_for_fetch_tasks_drain()` is trusted to mean "no more origin GETs will
be spawned."

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

let (prefetch_tx, prefetch_worker) = spawn_prefetch_worker(
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
    grace,
)
.await;

// axum's own drain can spawn fetch tasks up until the moment it returns, so
// only now is it safe to wait for the fetch-task count to reach zero.
let signal_instant = signal_at.get().copied().unwrap_or_else(Instant::now);
let remaining = grace.saturating_sub(signal_instant.elapsed());

// The prefetch worker is the *other* fetch-task producer, and its
// `JoinHandle` must actually be awaited (not discarded, as `main()` does
// today) before `wait_for_fetch_tasks_drain()` is trusted: `warm_item`'s
// window loop can still be between iterations -- guard dropped, next
// window's future not yet spawned -- when `outstanding_tasks` transiently
// reads 0, so only the worker's own exit proves no further window will be
// dispatched. Bounded by the same shared deadline as the fetch drain below.
if tokio::time::timeout(remaining, prefetch_worker).await.is_err() {
    warn!("prefetch worker did not exit within the grace period; proceeding anyway");
}
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
- **A close call, bounded by whatever grace remains.** After the fetch-task
  drain (Design §3) completes or times out, call `foyer.close()`, racing it
  against the remaining grace the same way the two drains above do. Whether
  `FoyerBackend::new_with_shards` should also set `.with_flush_on_close(false)`
  (leaving foyer's default `flush_on_close = true` otherwise applies) is the
  operational tradeoff left open below, not decided here:

```rust
let remaining = grace.saturating_sub(signal_instant.elapsed());
match tokio::time::timeout(remaining, foyer.close()).await {
    Ok(Ok(())) => info!("foyer cache closed"),
    Ok(Err(e)) => warn!("foyer cache close failed: {e:#}"),
    Err(_) => warn!("grace period of {}s elapsed before foyer cache close finished", grace.as_secs()),
}
```

This is what actually delivers the Overview's claim: a demand fetch's bytes
are only recoverable after restart once (a) this close step exists *and*
(b) the write policy admits demand fills to the write pipeline at all — see
the foyer flush-semantics question in Open Questions, which this step
depends on but does not resolve.

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
   add `shutdown` and `window_shutdown` future parameters to
   `spawn_prefetch_worker`; wrap the receiver in `.take_until(shutdown)` and
   have `window_shutdown` drive a small detached task that flips a shared
   `shutting_down: Arc<AtomicBool>`, which `warm_item` (now taking that flag)
   checks before pulling each window's next item from its `buffered` stream.
   Update the three now-inaccurate doc comments this touches:
   `prefetch_queue.rs:107-112`'s `spawn_prefetch_worker` doc (the handle no
   longer resolves only on channel closure), `saturation_monitor.rs:160-163`
   (no longer true that the handle needn't be awaited, nor that it parallels
   the prefetch worker), and `app_state.rs:28-31` (channel closure is no
   longer what stops the worker, even though the sentence stays literally
   true). Also update the six existing call sites in
   `rust/object-cache-srv/tests/prefetch_tests.rs` (`:180`, `:247`, `:294`,
   `:447`, `:534`, `:597`), threading a never-firing
   `std::future::pending::<()>()` for both new parameters so the tests keep
   compiling and behaving as before.
6. **Wire shutdown in `main()`** — `rust/object-cache-srv/src/object_cache_srv.rs`:
   build `ShutdownFanout` directly (recording the signal instant), pass two
   independent `subscribe()`s into the prefetch worker (`shutdown` and
   `window_shutdown`) and one into axum's drain; run the axum drain to
   completion, then await the prefetch worker's `JoinHandle` (bounded by
   whatever remains of `grace`) before running the fetch-task drain bounded
   by whatever remains after that. Then (Design §4): retain the
   `Arc<FoyerBackend>` bound before it's passed into `RangeCache::new`, and
   after the fetch-task drain call `foyer.close()` bounded by whatever
   remains of `grace`.
7. **Saturation gauge** — `rust/object-cache-srv/src/saturation_monitor.rs`:
   emit `outstanding_fetch_tasks()` as a new `object_cache_outstanding_fetch_tasks`
   gauge alongside `inflight_len()` (already sampled), so a stuck drain is
   visible in telemetry, not just at shutdown; add the corresponding row to
   the Saturation table in `mkdocs/docs/admin/object-cache.md` and a
   `rust/object-cache-srv/tests/saturation_tests.rs` test driving `sample_once`
   directly, mirroring the existing per-gauge tests there (e.g. the
   `object_cache_ram_tier_entries` precedent, #1322).
8. **Tests** — see Testing Strategy below, including a new panic-on-`get_range`
   `ObjectStore` test double, and a new `log_blocks`/`LogStringEvent`
   text-extraction helper for the shutdown-drop test — no in-repo precedent
   exists (unlike the metrics-block helpers already in `saturation_tests.rs`),
   so it must be written from scratch.
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
- `rust/object-cache-srv/src/prefetch_queue.rs` — add `shutdown` and
  `window_shutdown` future parameters to `spawn_prefetch_worker`;
  `.take_until(shutdown)` on the receiver stream; a `shutting_down:
  Arc<AtomicBool>` flag flipped by `window_shutdown` and checked by
  `warm_item` before each window; update the `spawn_prefetch_worker` doc
  comment (`:107-112`) to stop claiming the handle resolves only on channel
  closure.
- `rust/object-cache-srv/src/saturation_monitor.rs` — new
  `object_cache_outstanding_fetch_tasks` gauge in `sample_once` (no
  `shutdown` parameter — see Design §3/Step 5 for why); update the
  `spawn_saturation_monitor` doc comment (`:160-163`), which currently
  claims its handle parallels the prefetch worker's.
- `rust/object-cache-srv/src/app_state.rs` — update the `prefetch_tx` doc
  comment (`:28-31`): channel closure is no longer what stops the worker,
  even though the sentence stays literally true.
- `rust/object-cache-srv/src/object_cache_srv.rs` — build `ShutdownFanout`
  directly with a recorded signal instant; wire two independent
  subscriptions into the prefetch worker (`shutdown`, `window_shutdown`)
  and one into axum's drain; sequence the axum drain, then await the
  prefetch worker's `JoinHandle`, then the fetch-task drain, then
  `foyer.close()`, all against the shared deadline; retain the
  `Arc<FoyerBackend>` bound before it's passed into `RangeCache::new` so
  `close()` has something to call it on (Design §4).
- `rust/object-cache/tests/range_cache_tests.rs` — drain/panic-distinction
  regression tests; new panic-on-`get_range` `ObjectStore` double.
- `rust/object-cache-srv/tests/saturation_tests.rs` — new
  `object_cache_outstanding_fetch_tasks` gauge test.
- `rust/object-cache-srv/tests/prefetch_tests.rs` — update the six existing
  `spawn_prefetch_worker` call sites (`:180`, `:247`, `:294`, `:447`, `:534`,
  `:597`) to pass the two new parameters (a never-firing
  `std::future::pending::<()>()` for each).
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
  that window on the branch where axum's drain actually finishes; it costs
  some of the shared grace budget in the worst case (slow HTTP drain *and*
  slow fetches), which the shared-deadline design below already accounts
  for. On the *deadline* branch the window narrows but does not fully close:
  axum 0.8.9 `tokio::spawn`s a task per accepted connection
  (`serve/mod.rs:389`), so dropping `serve_future` at the deadline kills only
  the accept loop — already-accepted connections survive as orphaned tasks
  and can still call `RangeCache::size`/`spawn_run_fetch` after the fetch
  drain has already started. Practical impact is small because `remaining`
  is itself `~= 0` in exactly that branch, leaving little time for an
  orphaned connection to spawn and lose new work.
- **25s default grace period, unchanged.** `rust/public/src/config.rs:9-21`
  is the single source of the default (`default_value = "25"`), already set
  deliberately against a 30s orchestrator termination window (ECS
  `stopTimeout`, Kubernetes `terminationGracePeriodSeconds`) — see
  `mkdocs/docs/admin/service-lifecycle.md:92-95`, which already prescribes
  the escape hatch for slow origins: raise **both** the service grace period
  and the orchestrator window together. Two facts specific to this feature
  confirm 25s doesn't need revisiting for it: the cache client's own
  `CACHE_REQUEST_TIMEOUT` is 15s with fallback-to-direct-store
  (`object-cache/src/client.rs:19-25`), so no caller is listening past 15s
  regardless of how long the server-side drain runs; and there is no
  per-fetch timeout on the origin GET at all, so no finite grace could
  *guarantee* draining an 8 MiB coalesced run — which is exactly why
  "abandon and warn at the deadline" (rather than lengthening the default)
  is the right shape for this drain. Operators with unusually slow origins
  should use the existing escape hatch above rather than a bespoke default
  change here.
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
- `mkdocs/docs/admin/object-cache.md:104-123` (Prefetch section) documents
  the bounded queue and its load-shedding counters but says nothing about
  shutdown; add a note that on `SIGTERM` the worker stops admitting new
  items once the drain begins and any queued backlog is abandoned (Step 5).
- `mkdocs/docs/admin/object-cache.md:156-170` (In-process L1 cache) and/or
  `service-lifecycle.md:14-17`: add a sentence clarifying that the L1
  `RangeCache` used by FlightSQL/the monolith (`l1_store.rs:101`, wired via
  `lakehouse_context.rs` and `static_tables_configurator.rs`) gets the same
  accurate panic/shutdown log from Step 1, but **not** this plan's
  fetch-task drain — that drain is object-cache-srv-specific. Without this,
  a reader could infer from the new object-cache row in the
  service-lifecycle table that L1 drains too.
- `mkdocs/docs/admin/monolith.md`, `ingestion.md`, and `maintenance.md` need
  no change — none of them run or front an object cache.

## Testing Strategy
- **Panic vs. shutdown log distinction** (`range_cache_tests.rs`), split into
  two halves since nothing public survives the shutdown-drop path (`InFlight`,
  `FulfillGuard`, and `FetchScheduler` are all `pub(super)`, and the joiner
  future dies with the runtime):
  - *Panic half*: drive a fetch that panics (the new panic-on-`get_range`
    `ObjectStore` test double from step 8) through a public entry point
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
  monolith runs no object-cache *server* — it does run the in-process L1
  `RangeCache`, which is unaffected by this plan), issuing a slow/streamed request
  against the cache, then sending SIGTERM to the `micromegas-object-cache-srv`
  PID and confirming via `/tmp/object_cache.log` that fetch tasks drain (or
  are abandoned with the new accurate wording) before the process exits, with
  no more spurious "(likely a panic)" during a clean shutdown.
- Full gate: `cargo fmt`, `cargo clippy --workspace -- -D warnings`,
  `cargo test -p micromegas-object-cache -p micromegas-object-cache-srv -p micromegas`.

## Open Questions
- **Which foyer flush semantics should the new shutdown close step (Design
  §4) use?** Both shapes are implementable and neither has an in-repo
  precedent (the only existing `FoyerBackend::close()` callers are tests):
  (a) `close()` with foyer's default `flush_on_close = true` — best
  cross-restart warmth, but flushes the whole RAM tier (`--ram-mb` defaults
  to 512, `cli.rs:24-25`), consuming real wall-clock inside the same 25s
  budget the HTTP and fetch drains already share; or (b)
  `with_flush_on_close(false)` + `close()` — only drains the write pipeline,
  bounding shutdown latency, but helps only prefetch-hinted fills unless the
  cache policy is also changed to `WriteOnInsertion`. This is an operational
  tradeoff between shutdown latency and post-restart cache warmth with no
  in-repo convention or infra constraint to decide it; flagging for the
  reviewer rather than guessing.
