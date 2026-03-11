# process_spans Table Function Plan — IMPLEMENTED

## Overview

Replace `process_thread_spans(process_id)` with `process_spans(process_id, types)` where `types` is `'thread'`, `'async'`, or `'both'`. This returns thread spans and/or async spans in a unified schema, enabling the flame chart cell to show both span types from a single query.

The async span query reuses the begin/end self-join pattern already proven in `perfetto_trace_execution_plan.rs:423-457`.

## Current State

- `process_thread_spans(process_id)` returns thread spans only — it finds CPU streams via `get_process_thread_list`, queries `view_instance('thread_spans', stream_id)` per stream, and augments batches with `stream_id`/`thread_name`.
- `perfetto_trace_chunks(process_id, types, begin, end)` already handles `SpanTypes::{Thread, Async, Both}` (defined in `perfetto_trace_execution_plan.rs:39-43`). For async spans it self-joins `view_instance('async_events', process_id)` on `span_id` to pair begin/end events.
- The flame chart cell currently uses `process_thread_spans` with a manual async SQL alternative documented in `tasks/flame/plan.md:172-186`.

### Key existing code

| Purpose | File | Lines |
|---------|------|-------|
| Current thread-only table function | `process_thread_spans_table_function.rs` | Full file |
| SpanTypes enum (Thread/Async/Both) | `perfetto_trace_execution_plan.rs` | 39-43 |
| Async self-join SQL | `perfetto_trace_execution_plan.rs` | 423–457 |
| Thread list lookup | `process_streams.rs` | `get_process_thread_list` |
| Thread spans schema | `span_table.rs` | `get_spans_schema` (50-84) |
| UDTF registration | `query.rs` | 138-145 |
| Batch augmentation | `process_thread_spans_table_function.rs` | `augment_batch` (51-68) |
| Async events schema & builder | `async_events_table.rs` | Full file |
| Async events block processor | `async_events_block_processor.rs` | `on_begin_async_scope`, `on_end_async_scope` |
| Scope hash computation | `scope.rs` | `compute_scope_hash` (32-37) |

## Design

### SQL interface

```sql
-- Thread spans only (backward-compatible behavior)
SELECT * FROM process_spans('process-uuid', 'thread')

-- Async spans only
SELECT * FROM process_spans('process-uuid', 'async')

-- Both combined
SELECT name, begin, end, depth, thread_name as lane
FROM process_spans('process-uuid', 'both')
ORDER BY lane, begin
```

### Output schema

Same as current `process_thread_spans` — `stream_id` and `thread_name` prepended to thread spans schema:

```
stream_id:   Dictionary(Int16, Utf8)
thread_name: Dictionary(Int16, Utf8)
id:          Int64
parent:      Int64
depth:       UInt32
hash:        UInt32
begin:       Timestamp(Nanosecond, UTC)
end:         Timestamp(Nanosecond, UTC)
duration:    Int64
name:        Dictionary(Int16, Utf8)
target:      Dictionary(Int16, Utf8)
filename:    Dictionary(Int16, Utf8)
line:        UInt32
```

For async spans, the columns map as follows:
- `stream_id` → empty string `""`
- `thread_name` → `"async"`
- `id` → `span_id` from async events
- `parent` → `parent_span_id` from async events
- `depth` → `depth` from async events
- `hash` → `hash` from async events (stored at materialization time)
- `begin` → begin event `time`
- `end` → end event `time`
- `duration` → `end - begin`
- `name`, `target`, `filename`, `line` → from begin event

### Architecture

```
process_spans('pid', 'both')
        |
        v
ProcessSpansTableFunction::call()       -- parses process_id + types
        |
        v
ProcessSpansTableProvider               -- wraps execution plan
        |
        v  (on scan)
ProcessSpansExecutionPlan::execute()    -- async stream
        |
        v
make_session_context(query_range)
        |
        ├─ if thread|both:
        │   get_process_thread_list()
        │   for each (stream_id, thread_name):           -- parallel with buffered()
        │       ctx.sql("SELECT * FROM view_instance('thread_spans', stream_id)")
        │       augment_batch(batch, stream_id, thread_name)
        │       yield augmented batch
        │
        └─ if async|both:
            ctx.sql(async_join_query)                    -- self-join on span_id
            augment_batch(batch, "", "async")
            yield augmented batch
```

### Async spans SQL

Executed through `ctx.sql()` inside the execution plan, same pattern as thread span queries:

```sql
SELECT
    b.span_id as id,
    b.parent_span_id as parent,
    b.depth,
    b.hash,
    b.time as "begin",
    e.time as "end",
    arrow_cast(e.time, 'Int64') - arrow_cast(b.time, 'Int64') as duration,
    b.name,
    b.target,
    b.filename,
    b.line
FROM (SELECT * FROM view_instance('async_events', '{process_id}')
      WHERE event_type = 'begin') b
INNER JOIN (SELECT * FROM view_instance('async_events', '{process_id}')
      WHERE event_type = 'end') e
ON b.span_id = e.span_id
WHERE b.time < e.time
ORDER BY b.time
```

The `WHERE b.time < e.time` filter mirrors the `begin_ns < end_ns` guard in the perfetto code (`perfetto_trace_execution_plan.rs:482`).

The result batches are augmented with `stream_id=""` and `thread_name="async"` to match the output schema.

### SpanTypes enum

Move the existing `SpanTypes` enum from `perfetto_trace_execution_plan.rs` to a shared location (e.g., `process_spans_table_function.rs`) and reuse it from the perfetto code. Or duplicate it — it's 5 lines.

### No backward compatibility needed

`process_thread_spans` is a recent unreleased feature — just rename to `process_spans` and update all call sites.

## Implementation Steps

1. **Rename the table function**: `ProcessThreadSpansTableFunction` → `ProcessSpansTableFunction`, add second `types` argument parsing (match `'thread'`/`'async'`/`'both'`, same as `perfetto_trace_table_function.rs:85-95`)

2. **Add SpanTypes to execution plan**: Store `span_types: SpanTypes` in `ProcessSpansExecutionPlan`, gate thread/async code paths with `matches!(span_types, SpanTypes::Thread | SpanTypes::Both)` / `matches!(span_types, SpanTypes::Async | SpanTypes::Both)` — same pattern as `perfetto_trace_execution_plan.rs:272-283`

3. **Add `hash` column to async events table**: Add `hash: u32` field to `AsyncEventRecord`, `async_events_table_schema()`, and `AsyncEventRecordBuilder`. Store `scope.hash` in the block processor (`on_begin_async_scope`/`on_end_async_scope`). Bump `SCHEMA_VERSION` to `2` in `async_events_view.rs`. The view is JIT-materialized so no migration needed — stale partitions will be rebuilt on next query.

4. **Add async span streaming**: After the thread spans loop, if async is requested, execute the self-join SQL through `ctx.sql()`, iterate the result stream, augment each batch with `augment_batch(batch, schema, "", "async")`, yield

5. **Handle schema mismatch**: The async SQL uses `arrow_cast` for exact type matching on `duration` (Int64). The `hash` column is natively `UInt32` from the view. The string columns (`name`, `target`, `filename`) come from the `async_events` view which already stores them as `Dictionary(Int16, Utf8)`, so they should pass through the self-join as-is. If DataFusion decodes them to plain `Utf8` after the JOIN, re-encode them in the augment step using `StringDictionaryBuilder`.

6. **Register the new function**: In `query.rs`, replace the `process_thread_spans` registration with `process_spans`.

7. **Update default SQL**: In `notebook-utils.ts`, change the flamegraph default query from `process_thread_spans('$process_id')` to `process_spans('$process_id', 'both')`.

8. **Update perfetto code**: Change `perfetto_trace_execution_plan.rs` to import `SpanTypes` from the shared location instead of defining its own.

9. **Update documentation**: Update all mkdocs pages that reference `process_thread_spans` or async span queries to use `process_spans`. See files list below.

## Files to Modify

| File | Change |
|------|--------|
| `rust/analytics/src/async_events_table.rs` | Add `hash: UInt32` field to record, schema, and builder |
| `rust/analytics/src/lakehouse/async_events_block_processor.rs` | Store `scope.hash` in `AsyncEventRecord` |
| `rust/analytics/src/lakehouse/async_events_view.rs` | Bump `SCHEMA_VERSION` to `2` |
| `rust/analytics/src/lakehouse/process_thread_spans_table_function.rs` | Rename to `process_spans_table_function.rs`, add types arg, add async streaming |
| `rust/analytics/src/lakehouse/mod.rs` | Update module name |
| `rust/analytics/src/lakehouse/query.rs` | Replace `process_thread_spans` registration with `process_spans` |
| `rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs` | Import `SpanTypes` from shared location |
| `rust/analytics/src/lakehouse/perfetto_trace_table_function.rs` | Import `SpanTypes` from shared location |
| `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` | Update default flamegraph SQL |
| `tasks/flame/plan.md` | Update SQL examples |
| `mkdocs/docs/query-guide/functions-reference.md` | Replace `process_thread_spans` with `process_spans(process_id, types)` |
| `mkdocs/docs/query-guide/schema-reference.md` | Update `process_thread_spans` references |
| `mkdocs/docs/query-guide/async-performance-analysis.md` | Replace manual async self-join examples with `process_spans(id, 'async')` |
| `CHANGELOG.md` | Update `process_thread_spans` reference to `process_spans` |

## Testing

1. `cargo build` — verify compilation
2. `cargo clippy --workspace -- -D warnings` — no warnings
3. `cargo fmt` — formatting
4. `cargo test` — existing tests pass
5. Manual: `SELECT * FROM process_spans('pid', 'thread') LIMIT 10` — thread spans only
6. Manual: `SELECT * FROM process_spans('pid', 'async') LIMIT 10` — async spans with `thread_name='async'`
7. Manual: `SELECT * FROM process_spans('pid', 'both') LIMIT 10` — both types present
8. Manual: flame chart cell renders both thread and async spans with `process_spans('$process_id', 'both')`
