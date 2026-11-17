# Spawn Update-Group View Work Across Workers Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1796

## Overview
`run_update_groups` drives each update group with `buffer_unordered`, which polls every in-flight view update from the one task awaiting the collect. On a multi-threaded runtime the group overlaps I/O but gets about one core of CPU, so CPU-heavy stretches (plan building, parquet decode, merge execution) delay every other view and the rest of the cron task. Spawn each view update as its own tokio task, still capped by `concurrency`, so CPU-bound work spreads across workers.

## Current State
- `rust/public/src/servers/maintenance.rs:43-80`: `run_update_groups` builds all futures of a group eagerly (lines 61-65), runs them through `futures::stream::iter(futs).buffer_unordered(concurrency)` (66-69), collects `(index, Result)`, and sorts failures by global index (78).
- Only caller: `materialize_all_views` (same file, ~113). Its `run` closure already clones `Arc`s into an `async move` block; the future is close to `Send + 'static`, but `run_update_groups` does not require those bounds.
- `rust/public/tests/run_update_groups_tests.rs` covers serial order, the concurrency bound, failure isolation, group barrier, and context-fetch failure. These tests use `#[tokio::test]` (current-thread runtime).
- `micromegas_tracing::prelude::spawn_with_context` (`rust/tracing/src/spans/instrumented_future.rs:100`) spawns a task while keeping the async span parent, so spans of spawned view updates stay attached to the tick.
- `create_merged_partition` (`rust/analytics/src/lakehouse/merge.rs:400`) is already `#[span_fn(view = view.get_view_set_name())]`, so the "no view name on the merge span" gap appears closed by the span-property change. The merge's query execution still has no child spans.

## Design
Change `run_update_groups` to spawn each item:

- Bounds: `RunFut: Future<Output = Result<()>> + Send + 'static`, `C: Send + Sync + 'static`. The `run` closure stays `Fn(&T, Arc<C>) -> RunFut`, so `items` need not be `'static`.
- Per group, keep the futures built eagerly in a `Vec`/iterator (they are lazy until polled) and spawn lazily: start up to `concurrency` tasks, then each time one finishes, spawn the next. This keeps the cap without a semaphore and preserves the existing start order (input order), so `concurrency == 1` stays serial and ordered.
- Spawn with `spawn_with_context` and hold the handles in a small guard that aborts remaining tasks on drop (or use a `JoinSet` if span context can be carried through it), so dropping the pass still cancels in-flight work as `buffer_unordered` did.
- Group barrier: the group loop awaits all of a group's tasks before fetching the next group's context. Unchanged.
- Failure isolation and ordering: each task returns `(global_index, Result<()>)`; failures are still sorted by index at the end.
- Panics / cancellation: a `JoinError` for item `i` is converted into a failure `(i, anyhow!("view update task panicked/cancelled: {e}"))`, so it is reported through the same aggregated error in `materialize_all_views` and does not abort sibling items. The task index must be known when the join completes, so the spawned future returns its index and the abort-guard/`JoinSet` maps join errors back to the index (e.g. via `JoinSet::join_next_with_id` plus an id→index map).
- `fetch_group_context` errors still abort the pass (and abort any in-flight tasks via the guard).
- `materialize_all_views` needs no signature change; verify its closure compiles under the new bounds (all captures are `Arc`s or `Copy`).

Out of scope here: `spawn_blocking`/dedicated runtime for the merge, and instrumenting merge query execution. Spawning removes the single-task serialization; whether a further split is needed should be judged from measurements after this lands.

## Implementation Steps
1. `rust/public/src/servers/maintenance.rs`: rewrite the group body of `run_update_groups` with lazy spawning, abort-on-drop, and `JoinError` → failure mapping; add the `Send`/`'static` bounds; update the doc comment (spawned, concurrency cap, panic reporting) and drop the stale `buffer_unordered` comment.
2. Confirm `materialize_all_views` and `EveryDayTask`/other callers compile; fix any non-`Send` capture.
3. Add tests in `rust/public/tests/run_update_groups_tests.rs` (below).
4. `CHANGELOG.md`: add an entry; note the added `Send + 'static` bounds on `run_update_groups` as a **Minor breaking change** (Rust API only).

## Files to Modify
- `rust/public/src/servers/maintenance.rs`
- `rust/public/tests/run_update_groups_tests.rs`
- `CHANGELOG.md`

## Trade-offs
- `JoinSet` vs. handles + abort guard: `JoinSet` aborts on drop and offers `join_next_with_id`, but `JoinSet::spawn` does not carry tracing async-span context. Pick whichever keeps `spawn_with_context` semantics; both satisfy the design.
- Lazy spawning vs. spawn-all-behind-a-semaphore: lazy spawning keeps start order and needs no extra primitive; chosen.
- `spawn_blocking`/separate runtime for merges: heavier restructuring with unproven benefit; deferred until measurements after this change.

## Decisions
- Interface stability: the new bounds are a Rust API change only; no SQL surface is touched.

## Documentation
No `mkdocs/` page describes this internal scheduling. CHANGELOG only.

## Testing Strategy
All no-DB unit tests in `run_update_groups_tests.rs`:
- Existing tests keep passing unchanged (serial order at concurrency 1, bound honored, isolation, barrier, context error).
- New, `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]`: items that block their thread (e.g. a barrier of `std::sync::Barrier` with N parties inside the future) complete only if they run on separate workers — fails under `buffer_unordered`, passes with spawning.
- New: a panicking item is reported as a failure at its index while siblings and later groups still run.
- New: dropping the `run_update_groups` future aborts in-flight spawned work (a task holding a drop-guard flag observes cancellation).

## Manual Verification
Per the issue: run the maintenance daemon against a populated lake, compare per-thread CPU during the minute-merge window and `task_tick_latency` max / minute-merge total before and after. Not automated because it needs production-like load and a real runtime profile.

## Open Questions
- Does merge query execution still need `spawn_blocking` or child-span instrumentation once spawning lands? Decide from the manual measurements; tracked as a follow-up, not part of this plan.
