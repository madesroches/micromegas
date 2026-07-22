# jit_update: redundant unbounded metadata scans

## Problem

Traced via `process_spans(process_id, 'async')` on a `flight-sql-srv`/monolith process
during a `query_duration_total` spike (13.68s query, dominant 7.6s window). Root cause
identified in `thread_spans_view::jit_update`, but the same pattern applies to
`async_events_view::jit_update` and `metrics_view::jit_update` since they call the same
metadata helpers.

A query against `thread_spans_view` for a process with 20 CPU-thread streams triggers
20 independent `jit_update` calls (one per stream), run concurrently. They don't
parallelize cleanly — they contend on a shared bottleneck:

| span | count | avg | total (serial) |
|---|---|---|---|
| `thread_spans_view::jit_update` | 20 | 7.4s | 149s |
| `metadata::find_stream_from_view` | 20 | 3.2s | 65s |
| `metadata::find_process_with_latest_timing` | 22 | 2.4s | 53s |
| `partition_metadata::load_partition_metadata` | 30,678 | 2.1ms | 65s |

~76% of each 7.6s `jit_update` call (5.6s) is spent in the two metadata lookups above.
The reason they're slow is the `load_partition_metadata` count: **30,678 calls in a
single query**.

## Root cause

`find_stream_from_view` and `find_process_with_latest_timing`
(`rust/analytics/src/metadata.rs:174,274`) both call `make_session_context(...)` and
run `SELECT ... FROM streams/processes WHERE id = '{x}'` through the full DataFusion
lakehouse stack, passing `query_range: None`.

`streams` and `processes` are `SqlBatchView`s (`rust/analytics/src/lakehouse/streams_view.rs`,
`processes_view.rs`) — daily time-partitioned, unbounded history, merged from `blocks`.
With no time range to prune on, DataFusion loads partition metadata for **every
partition ever materialized** for that view just to find one row by primary key.

This is compounded twice:
1. **Unbounded scan per call** — no time bound, so it scans the view's entire history
   instead of a recent window.
2. **Done redundantly 20x per query** — every stream in the process independently
   re-resolves the *same* process's metadata (and re-scans the entire `streams`/
   `processes` partition catalog to do it), instead of being looked up once and shared.

The data already lives in indexed raw Postgres tables
(`rust/ingestion/src/sql_telemetry_db.rs:26-63`):
- `processes(process_id)` — indexed on `process_id`
- `streams(stream_id, process_id)` — indexed on both

The code pays for a full lakehouse/Parquet-partition scan to answer a query that a
plain indexed `sqlx` lookup against Postgres would answer in low single-digit
milliseconds.

## Solution directions

- Pass a bounded `query_range` to `find_stream_from_view` / `find_process_with_latest_timing`
  so partition pruning applies instead of `None`. (Querying the raw Postgres
  `streams`/`processes` tables directly via `sqlx`, bypassing `make_session_context`/
  DataFusion, was considered and rejected: those tables are transient ingestion
  bookkeeping — `delete_old_data` prunes rows from them directly
  (`rust/analytics/src/delete.rs:152-170`) — not a durable historical source, so
  lookups should stay inside the lakehouse/DataFusion query path rather than
  depending on Postgres retention.)
- Memoize `find_process_with_latest_timing` (and the process-lookup portion of
  `find_stream_from_view`) per `process_id` per top-level query, since all N
  per-stream `jit_update` calls in one query ask for the same process's data.

## Files to change

- `rust/analytics/src/metadata.rs`
  - `find_stream_from_view` (line ~174)
  - `find_process_with_latest_timing` (line ~274)
- Callers in `rust/analytics/src/lakehouse/thread_spans_view.rs`,
  `async_events_view.rs`, `metrics_view.rs` (`jit_update` methods) — may need to
  thread a shared/cached process lookup through instead of calling per-stream.
