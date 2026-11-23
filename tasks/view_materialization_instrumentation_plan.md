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
  but its internal steps (`make_batch_partition_spec`, `verify_overlapping_partitions`) are only
  covered by the latter's own span. `merge.rs::create_merged_partition` has a span; plan / execute /
  write inside it are not separated.
- `rust/public/src/servers/maintenance.rs::run_update_groups` spawns one task per view with
  `spawn_with_context` and waits for each group to drain (the barrier); neither the per-view task
  nor the barrier wait has a span.
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
- `fetch_sql_partition_spec`: the post-`count_query` bookkeeping is trivial; no new span there.
  (The "source hash computation, partition list, session context" work the issue mentions
  happens in the callers of `fetch_sql_partition_spec` that build `count_src_sql` and `ctx`; the
  implementer inspects `sql_batch_view.rs`'s `make_batch_partition_spec` and adds
  `#[span_fn(view = ..)]` spans around the context construction and partition-list steps found
  there, and none if there is nothing substantial.)

`write_partition.rs`
- `write_partition_from_rows` and `write_partition_file`: `#[span_fn(view = ..)]`. They run
  inside `spawn_with_context`, which carries the span context, so they appear as the writer task's
  root.
- `write_rows_and_track_times`: `#[span_fn]`; children `check_non_nullable_columns` (sync
  `span_scope!`), `arrow_writer_write` and `arrow_writer_flush` (see CPU attribution).
- `finalize_partition_write`: `#[span_fn]`; children `arrow_writer_close` (final encode + upload
  completion) and the empty-file delete (`delete_if_orphan`).
- `insert_partition`, `insert_partition_transaction`: `#[span_fn]`; inside the latter,
  `db_pool.begin` (`instrument_named!`, pool-acquire wait) and `transaction.commit`.
- `retire_partitions`: `#[span_fn]`.
- `lake.warm_object`: `instrument_named!` at its call site(s) in the write path.

`batch_update.rs` / `merge.rs`
- `materialize_partition`: `instrument_named!` around `make_batch_partition_spec` (spec
  construction) and the existing `verify_overlapping_partitions` call is already `#[span_fn]`;
  add nothing redundant.
- `create_merged_partition`: `instrument_named!` for `merge_plan`, `merge_execute`, `merge_write`
  around the three phases.

Daemon loop (`public/src/servers/maintenance.rs`)
- Per-view span: wrap the future passed to `spawn_with_context` in `materialize_all_views`'s closure
  with `instrument_named!(.., "materialize_view", |_, span| span.set_property("view", ..))` so
  per-pass cost groups by view without joining on children. (`run_update_groups` stays generic.)
- Group barrier: inside `run_update_groups`, wrap the `running.next().await` drain of each group
  as `update_group_wait`.

### CPU attribution

1. **Thread spans on sync CPU work.** `span_scope!` around the genuinely synchronous, CPU-heavy
   calls reachable from the extract/write paths: `ArrowWriter::write` and `flush` /`close`
   encoding in `write_rows_and_track_times`/`finalize_partition_write`, and
   `check_non_nullable_columns`. These show up in the tokio worker's CPU stream. DataFusion
   execution inside `stream.next()` is polled, not a sync function, so it is covered by the
   per-task poll metrics below rather than by thread spans.
2. **Per-task poll-time metrics.** Add the `tokio-metrics` crate (workspace dependency) and one
   `TaskMonitor` per task family: view update (the per-view task in `materialize_all_views`),
   writer task (`write_partition_from_rows` / `write_partition_file`), and block processor
   (`block_partition_spec` writer). A small helper in `rust/analytics` (e.g.
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

1. `system_monitor.rs`: `CgroupCpuStat`, `parse_cpu_stat`, `emit_cgroup_cpu_stats`, wired into the
   slow-sample branch; unit tests for the parser.
2. `sql_partition_spec.rs`: all spans listed above.
3. `write_partition.rs`: all spans listed above, including the thread spans around the sync
   `ArrowWriter` calls and `check_non_nullable_columns`.
4. `batch_update.rs`, `merge.rs`: spans listed above.
5. `maintenance.rs`: `materialize_view` span, `update_group_wait` span.
6. `tokio-metrics` dependency, task-monitor helper, wrapping the three task families, periodic
   emission from the maintenance daemon; unit test for `emit`-side computation if it is factored
   into a pure function.
7. Docs and changelog.
8. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test` in `rust/`.

## Files to Modify

- `rust/telemetry-sink/src/system_monitor.rs`
- `rust/analytics/src/lakehouse/sql_partition_spec.rs`
- `rust/analytics/src/lakehouse/write_partition.rs`
- `rust/analytics/src/lakehouse/batch_update.rs`
- `rust/analytics/src/lakehouse/merge.rs`
- `rust/analytics/src/lakehouse/block_partition_spec.rs` (wrap writer task with its monitor)
- `rust/analytics/src/lakehouse/task_monitors.rs` (new) and `lakehouse/mod.rs`
- `rust/public/src/servers/maintenance.rs`
- `rust/Cargo.toml`, `rust/analytics/Cargo.toml` (tokio-metrics)
- `CHANGELOG.md`, `mkdocs/docs/admin/maintenance.md`

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

## Documentation

- `mkdocs/docs/admin/maintenance.md`: list the new span names, the `task_family` poll metrics, and
  the `cgroup_cpu_*` metrics with a short note on identifying throttling
  (`cgroup_cpu_nr_throttled` / `cgroup_cpu_throttled_usec` rising).
- `CHANGELOG.md`: user-visible new metrics and spans.

## Testing Strategy

- Unit: `parse_cpu_stat` over a realistic `cpu.stat` body, a body missing fields, and garbage.
- Unit: the task-monitor helper — a monitor instrumented around a trivial future reports a non-zero
  poll count after `emit`'s pure computation step.
- No test asserts span names (they are telemetry and carry no behavior); existing
  `sql_partition_spec` / `write_partition` tests must still pass unchanged, which covers that the
  wrapping did not alter control flow or error propagation (including the
  `extract_err` → writer-join path).

## Manual Verification

1. Start services (`python3 local_test_env/ai_scripts/start_services.py`), let the maintenance
   daemon run one pass, then run
   `micromegas-query "SELECT name, count(*), sum(duration) FROM view_instance('thread_spans', ...) ..."`
   on the daemon's async span stream and confirm the new span names appear and that
   `extract_stream` self time (duration minus children) is a small residue. Not automated:
   needs a live daemon and real data.
2. Query `measures` for `cgroup_cpu_usage_usec` and `task_family` poll metrics and confirm they
   are non-zero and increasing. Skip the cgroup check on a host without `/sys/fs/cgroup/cpu.stat`.

## Open Questions

- Whether `tokio-metrics`' `TaskMonitor` overhead is acceptable on the per-batch hot path: it only
  wraps whole tasks (not per-poll calls we control), so expected negligible; confirm during review.
