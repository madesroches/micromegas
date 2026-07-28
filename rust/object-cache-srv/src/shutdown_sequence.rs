//! Post-axum graceful shutdown for `object-cache-srv`: stop the prefetch
//! producer, drain in-flight origin fetches, then close the foyer cache --
//! see `tasks/1291_graceful_shutdown_detached_fetch_plan.md` (Design §3/§4).
//!
//! Axum's own HTTP drain is awaited directly in `main()`, outside this
//! module entirely, and keeps the full shutdown grace period. `run` owns a
//! single `tokio::time::timeout` around the three stages that come after it,
//! bounded by whatever of that grace period axum's drain didn't use.

use std::sync::Arc;
use std::time::Duration;

use micromegas::object_cache::foyer_backend::FoyerBackend;
use micromegas::object_cache::range_cache::RangeCache;
use micromegas::tracing::prelude::*;
use tokio::task::JoinHandle;

/// Runs, in order: abort-and-join the prefetch worker (so it can no longer
/// spawn fetch work), wait for every outstanding detached fetch task to
/// drain, then close `foyer` so a drained fetch's bytes survive a restart.
/// All three are wrapped in one `tokio::time::timeout(remaining, ...)`; if it
/// elapses, the block is dropped mid-stage and whatever it was doing is
/// abandoned, exactly like today's runtime-teardown drop of an in-flight
/// task. Never returns an error: an elapsed deadline is logged and swallowed
/// here, never propagated.
pub async fn run(
    remaining: Duration,
    prefetch_worker: JoinHandle<()>,
    cache: RangeCache,
    foyer: Arc<FoyerBackend>,
) {
    // `cache` itself (not a clone) is moved into the timed block for the
    // drain; a clone is kept outside so `outstanding_fetch_tasks()` is still
    // readable for the elapsed-deadline warning below even if the block was
    // dropped mid-drain.
    let cache_for_warning = cache.clone();
    let finished = tokio::time::timeout(remaining, async move {
        // Stage 1: stop the prefetch producer. `abort()` is synchronous;
        // awaiting the handle afterwards observes the worker future as
        // already dropped, so it can no longer pull an item from the channel
        // and spawn one more fetch task. `JoinHandle::Output` is
        // `Result<(), JoinError>`, `#[must_use]`, so it must be handled --
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
        info!("origin fetch tasks drained");

        // Stage 3: persist the RAM tier. `mark_shutting_down()` is set
        // immediately before the close -- the only place it's needed, so
        // `RamEvictionListener::on_leave` can tell this flush apart from
        // capacity-driven thrashing (#1281).
        foyer.mark_shutting_down();
        match foyer.close().await {
            Ok(()) => info!("foyer cache closed"),
            Err(e) => warn!("foyer cache close failed: {e:#}"),
        }
    })
    .await
    .is_ok();

    if !finished {
        warn!(
            "shutdown grace period elapsed before the post-axum sequence finished; \
             whichever stage was still running was abandoned"
        );
        let outstanding = cache_for_warning.outstanding_fetch_tasks();
        if outstanding > 0 {
            warn!("abandoning {outstanding} in-flight origin fetch task(s) at shutdown");
        }
    }
}
