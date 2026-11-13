# Materialize Partition Spans Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1788

## Overview

In a thread-spans flamegraph of the maintenance daemon, `batch_update::materialize_partition` and
`materialize_partition_range` are wide but mostly lack child spans, so a slow partition cannot be
explained from a trace. This plan adds spans around the uninstrumented steps. It changes no behavior
and no SQL surface.

## Current State

`materialize_partition` (`rust/analytics/src/lakehouse/batch_update.rs:141`) runs, in order:

1. `view.make_batch_partition_spec(...)`. `SqlBatchView::make_batch_partition_spec`
   (`sql_batch_view.rs:264`) is not instrumented. It filters the partition cache, builds a session
   context and calls `fetch_sql_partition_spec` (`sql_partition_spec.rs:214`), which plans and runs
   the count query without any span.
2. `verify_overlapping_partitions` (`batch_update.rs:23`), not instrumented.
3. `SqlPartitionSpec::write` (`sql_partition_spec.rs:167`), not instrumented: extract planning
   (`ctx.sql`, `plan_sorted_extract`) and the `stream.next()` loop.
4. `create_merged_partition` (`merge.rs:400`), not instrumented.

DataFusion runs its plans on other tasks, so waiting on a query shows up as a gap on the calling thread.

## Design

Use the existing macros from `micromegas_tracing::prelude::*`: `#[span_fn]` for whole functions and
`instrument_named!(future, "name")` for awaited sections (precedent: `metadata.rs:300`).

- `fetch_sql_partition_spec`: `#[span_fn]`, plus `instrument_named!` around the count query's
  `ctx.sql(..)` + `df.collect()` as `count_query`.
- `SqlBatchView::make_batch_partition_spec`: `#[span_fn]`.
- `verify_overlapping_partitions`: `#[span_fn]`.
- `SqlPartitionSpec::write`: `#[span_fn]`, plus `instrument_named!` spans `extract_plan` (around
  `ctx.sql` and `execute_extract_query`) and `extract_stream` (around the stream-consume loop,
  including the final `join_handle.await`).
- `create_merged_partition`: `#[span_fn]`.

No span properties are added: the existing `materialize_partition` log lines already carry the view
and range. If the new spans show that per-bucket `filter_insert_range` cloning is significant, that
is a follow-up issue, not part of this change.

## Implementation Steps

1. Add the attributes and named spans listed above in the five files.
2. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`.

## Files to Modify

- `rust/analytics/src/lakehouse/sql_partition_spec.rs`
- `rust/analytics/src/lakehouse/sql_batch_view.rs`
- `rust/analytics/src/lakehouse/batch_update.rs`
- `rust/analytics/src/lakehouse/merge.rs`
- `CHANGELOG.md`

## Trade-offs

- Spans on whole functions plus named spans on the awaited phases, rather than finer-grained spans,
  because the issue's stated first step is attribution; the trace then shows what to refine.

## Decisions

## Documentation

None; the spans are observability detail. Add a `CHANGELOG.md` entry.

## Testing Strategy

The change is behavior-neutral instrumentation, so the existing test suite is the check. No new
tests.

## Manual Verification

1. Start services (`python3 local_test_env/ai_scripts/start_services.py`), ingest some data, and let the
   maintenance daemon materialize.
2. Run `micromegas-query "SELECT name, count(*) FROM view_instance('thread_spans', '<daemon thread>') GROUP BY name"`
   or inspect the daemon's thread spans; expect `count_query`, `extract_plan`, `extract_stream`,
   `verify_overlapping_partitions` and `create_merged_partition`. This is not automated because it
   needs a running daemon and a missing span is not a silent failure.

## Open Questions

None.
