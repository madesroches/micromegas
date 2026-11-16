# Concurrent View Materialization in the Every-Second Pass Plan

Issue: #1793

## Overview

The every-second maintenance task materializes every view one after another, so a pass costs
the sum of all the views' costs. With about 24 views a pass takes about 1.75 s against a 1 s
period, and passes stay under the 10 s skip threshold only because the runner lets 5 of them
overlap. This plan runs the views of one `update_group` concurrently, up to a bounded limit, for
the every-second task only. The group boundary stays a barrier. The minute, hourly and daily tasks
run with concurrency 1, which keeps their current behavior. Two metrics are added so the effect
can be measured and the skip threshold can be alerted on.

## Current State

- `rust/public/src/servers/maintenance.rs:53` `materialize_all_views(lakehouse, views,
  insert_range, partition_time_delta)` loops over `views` (already sorted by `update_group` in
  `sorted_views_from_registry`, `maintenance.rs:30`). When the group changes it refetches the
  `PartitionCache` (`PartitionCache::fetch_overlapping_insert_range`). It calls
  `materialize_partition_range` (`rust/analytics/src/lakehouse/batch_update.rs:233`) for each view
  in turn. A failing view is logged (`error!`), counted in `materialize_view_failure`, and pushed
  to `failures`. The pass returns a single aggregated `Err` listing the failed views in view
  order. A failure to fetch the cache aborts the pass with `?`.
- The four tasks (`EveryDayTask`, `EveryHourTask`, `EveryMinuteTask`, `EverySecondTask`,
  `maintenance.rs:125-318`) each compute their range and call `materialize_all_views`.
  `EverySecondTask::run` skips the pass when `Utc::now() - task_scheduled_time > 10 s`.
- `daemon()` (`maintenance.rs:434`) builds the tasks and runs the seconds task with
  `run_tasks_forever(vec![every_second], 5, ...)`. Up to 5 passes can be in flight at once.
- `materialize_partition_range` only reads `existing_partitions_all_views: Arc<PartitionCache>`.
  Same-group views have no inter-dependencies (documented in `materialize_all_views`, and enforced
  by `SqlBatchView::new`'s view factory, which only resolves views from earlier groups).
- Postgres: the lake pool comes from `pool_options(policy).connect(..)`
  (`rust/ingestion/src/data_lake_connection.rs:347`) with sqlx's default `max_connections` (10)
  and acquire timeout (30 s). Each view holds a connection only for short queries. The advisory
  lock in `insert_partition_transaction` (`write_partition.rs:489`) is held only for the
  retire + insert transaction, after the parquet file has been written. Its key is per
  `(view_set, instance, range)`, so different views in one pass never contend on it.
- `#[span_fn]` async spans set up their context on every poll (`SpanContextFuture`,
  `rust/tracing/src/spans/instrumented_future.rs`), so futures that interleave on one task keep
  correct nesting. `block_partition_spec.rs:148` already uses `buffer_unordered` under spans.
- Existing coverage: `rust/public/tests/materialize_fail_isolation_tests.rs` (`#[ignore]`,
  live DB) checks that two failing same-group views are both reported.
- Daemon env knobs follow the `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` pattern: read once,
  documented in the env table of `mkdocs/docs/admin/maintenance.md`. `resolve_u64_env`
  (`rust/public/src/settings.rs:411`) is the shared warn-and-default parser.

## Design

### 1. A generic grouped runner (the testable core)

Pull the group/barrier/fan-out logic out of `materialize_all_views` into a `pub` generic
function in `maintenance.rs`. Nothing in it is specific to views, so it can be unit-tested with
plain values and fake futures, with no DB:

```rust
/// Runs `run` over `items`, grouped by contiguous runs of equal `group_of`, with at most
/// `concurrency` items in flight. Each group starts after the previous one has fully finished,
/// and gets a fresh `fetch_group_context()` result.
/// Returns per-item failures in input order. A `fetch_group_context` error aborts the pass.
pub async fn run_update_groups<T, K, C, FetchFut, RunFut>(
    items: &[T],
    group_of: impl Fn(&T) -> K,
    concurrency: NonZeroUsize,
    mut fetch_group_context: impl FnMut() -> FetchFut,
    run: impl Fn(&T, Arc<C>) -> RunFut,
) -> Result<Vec<(usize, anyhow::Error)>>
where
    K: PartialEq,
    FetchFut: Future<Output = Result<C>>,
    RunFut: Future<Output = Result<()>>,
```

Logic:

- `items.chunk_by(|a, b| group_of(a) == group_of(b))` gives the groups, in order (the input is
  already sorted by group).
- For each group: `let ctx = Arc::new(fetch_group_context().await?);` then
  `futures::stream::iter(group.iter().enumerate()).map(|(i, item)| run(item, ctx.clone()).map(move |r| (offset + i, r))).buffer_unordered(concurrency.get())`,
  and collect the `Err`s with their global index.
- Sort the failures by index before returning. `buffer_unordered` finishes them in any order,
  and sorting keeps the aggregated error text the same as the serial path.

With `concurrency == 1`, `buffer_unordered(1)` polls exactly one future at a time in input order.
That is the current serial loop, so the minute, hourly and daily tasks keep their behavior with no
separate code path.

`buffer_unordered` rather than `tokio::spawn` / `JoinSet`: the work is I/O bound, the futures
borrow `group` and the closures (no `'static` bound), and they stay under the
`materialize_all_views` span. DataFusion still spawns its own tasks for execution, so the CPU part
of each view is not serialized on this task.

### 2. `materialize_all_views` gets a `concurrency` parameter

```rust
pub async fn materialize_all_views(
    lakehouse: Arc<LakehouseContext>,
    views: Arc<Vec<Arc<dyn View>>>,
    insert_range: TimeRange,
    partition_time_delta: TimeDelta,
    concurrency: NonZeroUsize,
) -> Result<()>
```

The body becomes:

- `run_update_groups(&views, |v| v.get_update_group(), concurrency, || PartitionCache::fetch_overlapping_insert_range(&pool, insert_range), |view, cache| async { ... })`.
- The `run` closure calls `materialize_partition_range` and, on error, does the existing `error!`
  and `imetric!("materialize_view_failure", ...)` before returning the error. The metric and log
  are emitted when the view fails, as they are today.
- The returned failures become `"{view_set_name} {view_instance_id}"` lines and feed the same
  `bail!` message as today.
- The empty-views early return disappears, because `chunk_by` on an empty slice yields no groups
  and so no cache fetch happens. Keep the doc comment's failure-policy paragraph. Add one sentence
  saying that views within a group may run concurrently and that the group boundary is a barrier.
  Move the inline "no inter-dependencies" comment onto `run_update_groups`'s call site.

The parameter is a `NonZeroUsize` with no default, so the compiler lists every caller (the four
tasks and the ignored live test). This is a Rust API break, recorded in the changelog.

### 3. Setting: `MICROMEGAS_SECONDS_MATERIALIZATION_CONCURRENCY`

- `pub const` name plus a default of `8` in `maintenance.rs`. `daemon()` reads it once with
  `resolve_u64_env`, and maps `0` to `1` (`NonZeroUsize::new(..).unwrap_or(NonZeroUsize::MIN)`)
  because 0 has no meaning here.
- `EverySecondTask` gains `pub concurrency: NonZeroUsize` and passes it through. The other three
  tasks pass `NonZeroUsize::MIN`.
- The monolith runs `daemon()`, so it picks up the setting with no extra work.

### 4. `max_parallelism` stays at 5

Total in-flight view work is at most `5 × concurrency` (40 with the default). In steady state a
concurrent pass should fit in its 1 s period, so overlap rarely engages and only about
`concurrency` views run at a time. The 5-slot overlap stays as a buffer for a slow pass, for
example an object-store hiccup. The product is documented rather than tuned.

### 5. Telemetry

Both metrics are emitted in `EverySecondTask::run`, the only task the change targets:

- `imetric!("every_second_task_delay", "ms", delay)`, emitted before the skip check so that
  skipped passes are reported too. This is the value the 10 s threshold applies to, so alerts
  should use it.
- `imetric!("every_second_pass_duration", "ms", elapsed)` around `materialize_all_views`,
  emitted on success and on failure. This measures the effect of the change.

## Implementation Steps

1. `rust/public/src/servers/maintenance.rs`
   - Add `run_update_groups` (section 1).
   - Rewrite `materialize_all_views` on top of it with the new `concurrency` parameter (section 2).
   - Add the env const and default, `EverySecondTask.concurrency`, the read in `daemon()`, and
     `NonZeroUsize::MIN` in the other three tasks (section 3).
   - Add the two metrics in `EverySecondTask::run` (section 5).
2. `rust/public/tests/materialize_fail_isolation_tests.rs`: pass `NonZeroUsize::new(2)` so that
   the existing live test also runs the two failing views concurrently.
3. New `rust/public/tests/run_update_groups_tests.rs` with the unit tests below.
4. Docs and changelog (see Documentation).
5. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test -p micromegas`.

## Files to Modify

- `rust/public/src/servers/maintenance.rs`
- `rust/public/tests/materialize_fail_isolation_tests.rs`
- `rust/public/tests/run_update_groups_tests.rs` (new)
- `mkdocs/docs/admin/maintenance.md`
- `CHANGELOG.md`

## Trade-offs

- **Env var vs. a registered DB setting (`SETTINGS` / `DbOverrides`).** A DB setting could be
  tuned from Admin → Settings without a restart. However, it needs a new `SettingKind` range,
  `DbOverrides` field, loader and accessor, and the daemon would be its only reader. An env var
  read at startup matches `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` and the other daemon knobs.
  It can be promoted later if runtime tuning turns out to be needed.
- **Raise `max_parallelism` instead.** That adds more overlapping passes but does not shorten any
  of them. Overlapping passes of the same view also serialize on that view's advisory lock and
  redo each other's freshness checks. Concurrency within a pass fixes the pass duration itself.
- **`buffer_unordered` vs. `buffered`.** `buffered` keeps output order but counts finished,
  unyielded results against the limit, so one slow view would hold up the next ones. Sorting the
  failures afterwards gives deterministic error text without that cost.
- **Generic `run_update_groups` vs. testing `materialize_all_views` directly.** Testing it directly
  needs a live Postgres (the cache fetch) and real views. With the generic core, the concurrency,
  barrier and failure-ordering logic is fully covered by no-DB tests. The view-specific shell
  that is left is thin.
- **Concurrency for the minute, hourly and daily tasks too.** Not done: their buckets are much
  larger, so concurrent merges and extracts need much more memory. That is a separate decision to
  make with memory data, as the issue states.

## Decisions

- No new live-DB test for "concurrent views produce the same partitions as the serial path":
  this is new-feature acceptance, not a bug seen in the wild. The project's testing rule sends it
  to no-DB unit tests (the grouped runner) and manual verification. The existing ignored isolation
  test is switched to concurrency 2 at no extra cost.
- The lake pool size (sqlx default of 10) is left unchanged. The default concurrency of 8 stays
  under it in steady state, and connections are held only for short queries. If acquire waits
  show up under overlap, making the pool size configurable is a follow-up.
- Default concurrency of 8 confirmed by the user.

## Documentation

- `mkdocs/docs/admin/maintenance.md`
  - Add a row for `MICROMEGAS_SECONDS_MATERIALIZATION_CONCURRENCY` to the env table: default
    `8`, every-second task only, views in one update group run concurrently up to this limit,
    and with up to 5 overlapping passes the in-flight view work is bounded by 5 × this value.
  - In the "What it does" table, add to the "Every second" row that views of one update group
    are materialized concurrently.
  - Add a short section for `every_second_task_delay` and `every_second_pass_duration` next to
    `materialize_view_failure`, saying to alert on the delay approaching 10 s.
  - Adjust the failure-isolation paragraph: unchanged semantics, failures still listed per view.
- `CHANGELOG.md` (Unreleased): a **Lakehouse** entry for #1793 covering the new env var, the
  metrics, and a **Minor breaking change** clause:
  `materialize_all_views` takes a `concurrency: NonZeroUsize` argument.

## Testing Strategy

No-DB unit tests in `rust/public/tests/run_update_groups_tests.rs`, driving `run_update_groups`
with integer items, an integer group key, and a counter as the group context. To check
concurrency without sleeps, each `run` future increments an in-flight counter, records the
maximum, and yields (`tokio::task::yield_now()`) a few times before decrementing. Where a test
needs to prove that items are concurrent, the futures wait on a gate that opens once N of them
have started. That test is wrapped in `tokio::time::timeout` so a serial regression fails instead
of hanging.

- **Concurrency 1 is serial and ordered:** the maximum in-flight count is 1, and the start order
  equals the input order.
- **Concurrency is reached and bounded:** a group of 6 with concurrency 3 reaches an in-flight
  count of exactly 3 (gate) and never more.
- **Group barrier:** no item of group k+1 starts before every item of group k has finished
  (start and finish events in a shared log). `fetch_group_context` is called once per group,
  before that group's first item, and each item gets its own group's context value.
- **Failure isolation:** failing items do not stop the other items in the same group or in later
  groups, and every item runs.
- **Failure order is input order:** the first failing item is made to finish last, and the
  returned failures are still sorted by index.
- **Context fetch error aborts:** an error from the second fetch returns `Err`, and no item of
  the second group runs.
- **Empty input:** no fetch, no failures.

Existing `materialize_fail_isolation_tests.rs` (ignored, live DB), run at concurrency 2: it shows
that the real `View` / `PartitionCache` wiring still isolates and reports both failures when they
run concurrently. A fake cannot reproduce that wiring.

## Manual Verification

Performance acceptance (a pass costs about the slowest view, not the sum) depends on real
I/O latency, so no unit test can show it.

1. `python3 local_test_env/ai_scripts/start_services.py` with
   `MICROMEGAS_SECONDS_MATERIALIZATION_CONCURRENCY=1` exported, let ingestion run for a few
   minutes, then
   `micromegas-query "SELECT avg(value), max(value) FROM measures WHERE name = 'every_second_pass_duration'" --begin 5m`.
   Expected: roughly the sum of per-view costs (the current baseline).
2. Stop the services, restart with the variable unset (default 8), and run the same query.
   Expected: a clearly lower average, close to the slowest view plus one cache fetch per group.
3. Run `micromegas-query "SELECT max(value) FROM measures WHERE name = 'every_second_task_delay'" --begin 5m`.
   Expected: well under 10 000 ms. Also check `/tmp/daemon.log` for `materialize_all_views`
   errors or pool acquire timeouts. Expected: none.

