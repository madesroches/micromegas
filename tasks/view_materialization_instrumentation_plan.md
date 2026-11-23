# View Materialization Instrumentation Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1805

## Overview

Add spans and metrics so CPU and wall time spent in view materialization can be attributed to a
specific step. Today `sql_partition_spec::extract_stream` and `count_query` have ~100% self time,
and the SQL-view writer task has no span of its own, so a regression such as CPU throttling in the
maintenance daemon inflates every span uniformly and cannot be traced to its source. The change is
deliberately generous; spans that never carry meaningful time are removed in a follow-up once real
traces exist. No SQL-visible schema changes: spans are ordinary telemetry rows.

## Current State

- `rust/analytics/src/lakehouse/sql_partition_spec.rs`: `write`/`write_file` (`#[span_fn]`) call
  `begin_write` (logs, then `extract_plan`), spawn the writer with `spawn_with_context`, then run
  `extract_stream`, which loops `stream.next()` → `transform_batch` (transformer +
  `get_time_bounds(ctx.read_batch(..))`) → `tx.send` (channel capacity 1), then awaits
  `join_handle`. Only `extract_plan`, `extract_stream`, `count_query`, `fetch_sql_partition_spec`,
  `write`, `write_file` have spans. `execute_extract_query` / `plan_sorted_extract` do not.
- `rust/analytics/src/lakehouse/write_partition.rs`: only `retire_expired_partitions*` have
  `#[span_fn]`. `write_partition_from_rows`, `write_partition_file`, `write_rows_and_track_times`,
  `finalize_partition_write`, `insert_partition(_transaction)`, `retire_partitions`,
  `check_non_nullable_columns` have none.
- `rust/analytics/src/lakehouse/block_partition_spec.rs:106,159,167` is the precedent for async
  spans on channel send and writer join: `instrument_named!(fut, "row_set_channel_send")`,
  `instrument_named!(join_handle, "write_partition_from_rows_join")`.
- `rust/analytics/src/lakehouse/batch_update.rs`: `materialize_partition` has `#[span_fn(view=..)]`
  and so does `materialize_partition_range` (`batch_update.rs:234`). `SqlBatchView`'s
  `make_batch_partition_spec` has `#[span_fn]` (`sql_batch_view.rs:279`), but the block-based
  implementations (log_view, metrics_view, thread_spans_view, async_events_view, net_spans_view...)
  and their shared `fetch_partition_source_data` (`partition_source_data.rs:250`) have none.
  `merge.rs::create_merged_partition` has a span; plan / execute / write inside it are not
  separated.
- `rust/public/src/servers/maintenance.rs::run_update_groups` spawns one task per view with
  `spawn_with_context` and waits for each group to drain (the barrier); the barrier wait has no
  span. Each per-view task already has a per-view root span from `materialize_partition_range`.
- `rust/telemetry-sink/src/system_monitor.rs` already emits process RSS and jemalloc stats from
  `send_system_metrics_forever` every `SLOW_SAMPLE_INTERVAL` (5 s) via `emit_process_memory_stats`.
- Span primitives (`rust/tracing/src/macros.rs`): `#[span_fn(key = value)]` for functions,
  `instrument_named!(fut, "name", |out, span| span.set_property(..))` for async blocks,
  `span_scope!(name = "x", key = value)` for sync scopes (thread spans).

## Design

### Conventions

- Async sub-steps use `instrument_named!` (precedent: `block_partition_spec.rs`); whole functions
  use `#[span_fn(view = ...)]`. Where a span is on a function with a view name available, attach
  `view`; for `instrument_named!` sub-steps pass `view` through the `on_end` closure's
  `span.set_property("view", ..)`.
- Span names are static strings, namespaced like the existing ones (`extract_next_batch`, not
  `next`).
- `write_partition.rs` free functions that have no view name in scope get plain `#[span_fn]`; the
  writer-task root spans (`write_partition_from_rows`, `write_partition_file`) take `view` from
  `view_metadata`.

### Async spans

`sql_partition_spec.rs`
- `begin_write`: `#[span_fn(view = ..)]`.
- `execute_extract_query`: `#[span_fn(view = ..)]`; inside, `instrument_named!` around
  `plan_sorted_extract(..)` as `plan_sorted_extract` and make `execute_stream` a sync
  `span_scope!(name = "execute_stream", ..)`, since `execute_stream` is not async. (`execute_stream`
  only builds the stream; row production is in `stream.next()`.)
- `extract_stream` loop: wrap `stream.next()` as `extract_next_batch`, `transform_batch` as
  `transform_batch` (with children `transformer_transform` and `get_time_bounds` around the two
  awaits inside it), `tx.send(..)` as `extract_channel_send`, and the final `join_handle.await`
  (both the success and error paths) as `extract_writer_join`.
- `count_query`: split into `count_query_plan` (`ctx.sql`) and `count_query_collect`
  (`df.collect()`).
- `fetch_sql_partition_spec`: no new spans: `SqlBatchView::make_batch_partition_spec` already has a
  span (`sql_batch_view.rs:279`), context construction is covered by `make_session_context`'s span
  (`query.rs:329`), and the remaining steps are trivial.

`write_partition.rs`
- `write_partition_from_rows` and `write_partition_file`: `#[span_fn(view = ..)]`. They run
  inside `spawn_with_context`, which carries the span context, so they appear as the writer task's
  root.
- `write_rows_and_track_times`: `#[span_fn]`; children `check_non_nullable_columns` (sync
  `span_scope!`), `arrow_writer_write` and `arrow_writer_flush` (`instrument_named!` async spans:
  the writer is an `AsyncArrowWriter`, so a `!Send` `span_scope!` guard cannot be held across the
  await; encode CPU is covered by the writer task's poll-time metrics, see CPU attribution).
- `finalize_partition_write`: `#[span_fn]`; children `arrow_writer_close` (`instrument_named!`;
  final encode + upload completion) and `delete_empty_file` around the empty-file
  `object_store.delete` calls.
- `insert_partition`, `insert_partition_transaction`: `#[span_fn]`; inside the latter,
  `db_pool.begin` (`instrument_named!`, pool-acquire wait) and `transaction.commit`.
- `retire_partitions`: `#[span_fn]`.

`batch_update.rs` / `partition_source_data.rs` / `merge.rs`
- `fetch_partition_source_data` (`partition_source_data.rs:250`, shared by the block-based
  `make_batch_partition_spec` impls): `#[span_fn]` with `view` where available. No new span at the
  `materialize_partition` call site; `verify_overlapping_partitions` is already `#[span_fn]`.
- `create_merged_partition`: `merge_plan` (`instrument_named!`) around
  `view.merge_partitions(..).await`, which does the planning and `execute_stream`. In the
  `merged_stream.next()` loop, mirror the extract loop: `merge_next_batch`, `get_time_bounds`, and
  `merge_channel_send` (`tx.send`, capacity-1 channel). `merge_writer_join` on both writer join
  paths. The writer's own spans carry the write cost.

Daemon loop (`public/src/servers/maintenance.rs`)
- Per-view cost already groups by view via the existing `materialize_partition_range` span.
- Group drain: wrap the whole per-group spawn/drain loop in `run_update_groups`
  (`maintenance.rs:84-103`) in one `instrument_named!` span named `update_group`.

### CPU attribution

1. **Thread spans on sync CPU work.** `span_scope!` around `check_non_nullable_columns`, the only
   genuinely synchronous CPU-heavy call on the write path. `ArrowWriter` write / flush / close are
   async (`AsyncArrowWriter`) and DataFusion execution inside `stream.next()` is polled, so both
   are covered by the per-task poll metrics below rather than by thread spans.
2. **Per-task poll-time metrics.** Add the `tokio-metrics` crate (`0.5.2`, workspace dependency)
   and one `TaskMonitor` per task family: view update (the per-view task in
   `materialize_all_views`), writer task, and block processor (the per-block
   `block_processor.process` tasks at `block_partition_spec.rs:139`). The writer-family monitor is
   applied once, inside `write_partition_file`'s body (`write_partition_from_rows` calls it), so no
   spawn site changes and nothing is double-counted. A small helper in `rust/analytics` (e.g.
   `lakehouse/task_monitors.rs`) holds named static monitors and exposes `instrument(name, fut)`
   and `emit_task_metrics()`. `emit_task_metrics` reads each monitor's `cumulative()` and emits
   `imetric!` for `total_poll_duration` (ns), `total_scheduled_duration`, and `total_poll_count`,
   tagged with a `task_family` property. The maintenance daemon calls it on a 5 s interval,
   alongside its other periodic work.
3. **cgroup `cpu.stat`.** In `telemetry-sink/src/system_monitor.rs`, add
   `emit_cgroup_cpu_stats()` next to `emit_process_memory_stats`, called from the slow-sample branch.
   It reads `/sys/fs/cgroup/cpu.stat` (cgroup v2), parses with a pure
   `parse_cpu_stat(&str) -> Option<CgroupCpuStat>` for testability, and emits `imetric!`s:
   `cgroup_cpu_usage_usec`, `cgroup_cpu_nr_throttled`, `cgroup_cpu_throttled_usec` (cumulative
   counters, as the file reports them). A missing file (non-Linux, cgroup v1, no cgroup) emits
   nothing.

## Implementation Steps

1. `system_monitor.rs`: `CgroupCpuStat`, `pub parse_cpu_stat`, `emit_cgroup_cpu_stats`, wired into
   the slow-sample branch; parser tests in `rust/telemetry-sink/tests/system_monitor_tests.rs`.
2. `sql_partition_spec.rs`: all spans listed above.
3. `write_partition.rs`: all spans listed above, including the async `arrow_writer_*` spans and the
   sync `span_scope!` on `check_non_nullable_columns`.
4. `batch_update.rs`, `partition_source_data.rs`, `merge.rs`: spans listed above.
5. `maintenance.rs`: `update_group` span.
6. `tokio-metrics` dependency, task-monitor helper, wrapping the three task families, periodic
   emission from the maintenance daemon; helper test in a new file under `rust/analytics/tests/`.
7. Docs and changelog.
8. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test` in `rust/`.

## Files to Modify

- `rust/telemetry-sink/src/system_monitor.rs`
- `rust/analytics/src/lakehouse/sql_partition_spec.rs`
- `rust/analytics/src/lakehouse/write_partition.rs`
- `rust/telemetry-sink/tests/system_monitor_tests.rs`
- `rust/analytics/src/lakehouse/partition_source_data.rs`
- `rust/analytics/src/lakehouse/merge.rs`
- `rust/analytics/src/lakehouse/block_partition_spec.rs` (wrap the per-block `block_processor.process`
  tasks with the block processor monitor)
- `rust/analytics/src/lakehouse/task_monitors.rs` (new) and `lakehouse/mod.rs`
- a new test file under `rust/analytics/tests/` (task-monitor helper)
- `rust/public/src/servers/maintenance.rs`
- `rust/Cargo.toml` (`tokio-metrics = "0.5.2"`, alphabetical), `rust/analytics/Cargo.toml`
- `CHANGELOG.md`, `mkdocs/docs/admin/maintenance.md`, `mkdocs/docs/admin/memory-allocator.md` or
  `mkdocs/docs/admin/telemetry-sink-tuning.md`

## Trade-offs

- **Spans per step vs. a profiler.** A sampling profiler would attribute CPU without code changes
  but is not available in the deployed container; spans and metrics flow through the existing
  telemetry path and are queryable in SQL.
- **Generous now, prune later** (the issue's stated approach): the cost is telemetry volume per
  batch (`extract_next_batch` etc. fire per record batch), accepted until real traces show which
  spans are noise.
- **`tokio-metrics` vs. hand-rolled poll timing:** a hand-rolled wrapper future would avoid a
  dependency but duplicates a maintained, tokio-team crate.

## Decisions

- `TaskMonitor` overhead is accepted: a few `Instant::now` calls and atomics per task poll,
  negligible next to per-batch work.

## Documentation

- `mkdocs/docs/admin/maintenance.md`: list the new span names and the `task_family` poll metrics.
- `cgroup_cpu_*` metrics come from the shared system monitor in every process: document them
  alongside the other shared-sampler metrics (`mkdocs/docs/admin/memory-allocator.md` "What to
  watch" style, or `telemetry-sink-tuning.md`), with a short note on identifying throttling
  (`cgroup_cpu_nr_throttled` / `cgroup_cpu_throttled_usec` rising).
- `CHANGELOG.md`: user-visible new metrics and spans.

## Testing Strategy

- Unit (`rust/telemetry-sink/tests/system_monitor_tests.rs`): `parse_cpu_stat` over a realistic
  `cpu.stat` body, a body missing fields, and garbage.
- Unit (new file under `rust/analytics/tests/`): the task-monitor helper — a monitor instrumented around a trivial future reports a non-zero
  poll count after `emit`'s pure computation step.
- No test asserts span names (they are telemetry and carry no behavior); existing
  `sql_partition_spec` / `write_partition` tests must still pass unchanged, which covers that the
  wrapping did not alter control flow or error propagation (including the
  `extract_err` → writer-join path).

## Manual Verification

1. Start services (`python3 local_test_env/ai_scripts/start_services.py`), let the maintenance
   daemon run one pass, then query `view_instance('async_events', '<daemon process_id>')` using
   the begin/end join pattern in `mkdocs/docs/query-guide/async-performance-analysis.md` to get
   span durations, and confirm the new span names appear and that `extract_stream` self time
   (duration minus children) is a small residue. Check `view_instance('thread_spans', ..)` only
   for the `check_non_nullable_columns` thread span. Not automated: needs a live daemon and real
   data.
2. Query `measures` for `cgroup_cpu_usage_usec` and `task_family` poll metrics and confirm they
   are non-zero and increasing. Skip the cgroup check on a host without `/sys/fs/cgroup/cpu.stat`.
