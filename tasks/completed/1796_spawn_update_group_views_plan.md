# Spawn Update-Group View Work Across Workers Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1796

## Overview
`run_update_groups` drives each update group with `buffer_unordered`, which polls every in-flight view update from the one task awaiting the collect. On a multi-threaded runtime the group overlaps I/O but gets about one core of CPU, so CPU-heavy stretches (plan building, parquet decode, merge execution) delay every other view and the rest of the cron task. Spawn each view update as its own tokio task, still capped by `concurrency`, so CPU-bound work spreads across workers.

## Current State
- `rust/public/src/servers/maintenance.rs:43-80`: `run_update_groups` builds all futures of a group eagerly (lines 61-65), runs them through `futures::stream::iter(futs).buffer_unordered(concurrency)` (66-69), collects `(index, Result)`, and sorts failures by global index (78).
- Only caller: `materialize_all_views` (same file, ~113). Its `run` closure already clones `Arc`s into an `async move` block; the future is close to `Send + 'static`, but `run_update_groups` does not require those bounds.
- `rust/public/tests/run_update_groups_tests.rs` covers serial order, the concurrency bound, failure isolation, group barrier, and context-fetch failure. These tests use `#[tokio::test]` (current-thread runtime).
- `micromegas_tracing::prelude::spawn_with_context` (`rust/tracing/src/spans/instrumented_future.rs:100`) spawns a task while keeping the async span parent, so spans of spawned view updates stay attached to the tick.

## Design
Change `run_update_groups` to spawn each item:

- Bounds: `RunFut: Future<Output = Result<()>> + Send + 'static`, `C: Send + Sync + 'static`. The `run` closure stays `Fn(&T, Arc<C>) -> RunFut`, so `items` need not be `'static`.
- Per group, keep the futures built eagerly in a `Vec`/iterator (they are lazy until polled) and spawn lazily: start up to `concurrency` tasks, then each time one finishes, spawn the next. This keeps the cap without a semaphore and preserves the existing start order (input order), so `concurrency == 1` stays serial and ordered.
- Spawn with `spawn_with_context` and keep each task's `AbortHandle` in a small guard (a `Vec<AbortHandle>` aborted in `Drop`), so dropping the pass still cancels in-flight work as `buffer_unordered` did.
- Group barrier: the group loop awaits all of a group's tasks before fetching the next group's context. Unchanged.
- Failure isolation and ordering: each task returns `(global_index, Result<()>)`; failures are still sorted by index at the end.
- Panics: a `JoinError` for item `i` (in practice a panic) is converted into a failure `(i, anyhow!("update task failed: {e}"))`, so it is reported through the same aggregated error in `materialize_all_views` and does not abort sibling items. Panicked views are reported by name in the aggregated `Err` (plus the panic hook's log), not counted in the `materialize_view_failure` metric, because the `error!`/`imetric!` calls live inside the per-view closure; this is accepted as-is. Each handle is mapped to its index (`handle.map(move |r| (i, r))`) and pushed into a `FuturesUnordered`.
- `fetch_group_context` errors still abort the pass.
- `materialize_all_views` needs no signature change; verify its closure compiles under the new bounds (all captures are `Arc`s or `Copy`).

## Implementation Steps
1. `rust/public/src/servers/maintenance.rs`: rewrite the group body of `run_update_groups` with lazy spawning, abort-on-drop, and `JoinError` → failure mapping; add the `Send`/`'static` bounds; update the doc comment (spawned, concurrency cap, panic reporting) and drop the stale `buffer_unordered` comment; also reword or remove the eager-build comment at `maintenance.rs:59-60` (it refers to a stream that no longer exists).
2. Confirm `materialize_all_views` compiles under the new bounds.
3. Add tests in `rust/public/tests/run_update_groups_tests.rs` (below).
4. `CHANGELOG.md`: add an entry; note the added `Send + 'static` bounds on `run_update_groups` as a **Minor breaking change** (Rust API only).

## Files to Modify
- `rust/public/src/servers/maintenance.rs`
- `rust/public/tests/run_update_groups_tests.rs`
- `CHANGELOG.md`

## Decisions
- Lazy spawning chosen over spawn-all-behind-a-semaphore (keeps start order, no extra primitive).
- AFK: Panicked views are reported by name in the aggregated `Err` (plus the panic hook's log) but not counted in `materialize_view_failure`; accepted as-is.
- Interface stability: the new bounds are a Rust API change only; no SQL surface is touched.
- spawn_blocking / merge instrumentation deferred until post-change measurements.

## Documentation
No `mkdocs/` page describes this internal scheduling. CHANGELOG only.

## Testing Strategy
All no-DB unit tests in `run_update_groups_tests.rs`:
- Existing tests keep passing unchanged (serial order at concurrency 1, bound honored, isolation, barrier, context error).
- New, `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]`: N=2 or 3 items, concurrency = N, each increments a shared `AtomicUsize` then busy-waits (no `.await`) until it reaches N or an `Instant` deadline passes, returning `Err` on timeout. Succeeds only if items run on separate workers — fails (rather than hangs) under `buffer_unordered`, passes with spawning.
- New: a panicking item is reported as a failure at its index while siblings and later groups still run.
- New: dropping the `run_update_groups` future aborts in-flight spawned work (an item holds a tokio `oneshot::Sender` and parks on `pending()`; the test waits for the item to start, drops the `run_update_groups` future, then awaits the receiver under a generous timeout and asserts `RecvError`).

## Manual Verification
Per the issue: run the maintenance daemon against a populated lake, compare per-thread CPU during the minute-merge window and `task_tick_latency` max / minute-merge total before and after. Not automated because it needs production-like load and a real runtime profile.
