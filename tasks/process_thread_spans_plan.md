# process_thread_spans Table Function Plan

## Overview

Add a `process_thread_spans(process_id)` table function that returns thread spans from all CPU streams of a process, with `stream_id` and `thread_name` columns prepended. This enables cross-thread span analysis (e.g., frame budget notebooks) without requiring per-stream queries from the client.

No new materialized views are created. The function reuses existing per-stream `thread_spans` JIT materialization at query time, following the `perfetto_trace_chunks` pattern.

## Current State

- `thread_spans` is per-stream only. `view_instance('thread_spans', stream_id)` requires a literal stream UUID. There is no process-level access (`thread_spans_view.rs:67-69` explicitly rejects `"global"`).
- `perfetto_trace_chunks` already solves the same fan-out problem: it takes a `process_id`, finds all CPU streams, queries each stream's `thread_spans` in parallel, and streams the results. See `perfetto_trace_execution_plan.rs:400-471`.
- `async_events` supports process-level access (`view_instance('async_events', process_id)`) via a materialized view. We intentionally avoid that approach here to keep it lightweight.

### Key existing code

| Purpose | File | Lines |
|---------|------|-------|
| Find CPU streams for a process | `perfetto_trace_execution_plan.rs` | `get_process_thread_list` (316-371) |
| Parallel per-stream query + streaming | `perfetto_trace_execution_plan.rs` | `generate_thread_spans_with_writer` (400-471) |
| Table function → execution plan pattern | `perfetto_trace_table_function.rs` | Full file |
| Execution plan → TableProvider wrapper | `perfetto_trace_execution_plan.rs` | `PerfettoTraceTableProvider` (563-606) |
| Thread spans schema | `span_table.rs` | `get_spans_schema` (50-84) |
| Session context creation | `query.rs` | `make_session_context` (167-) |
| UDTF registration | `query.rs` | `register_lakehouse_functions` (94-144) |

## Design

### SQL interface

```sql
SELECT stream_id, thread_name, begin, "end", duration, name, depth, ...
FROM process_thread_spans('process-uuid')
```

The time range is provided out of band via the session's `query_range`, following the same pattern as `ViewInstanceTableFunction`. The `query_range` flows from the FlightSQL service through `register_lakehouse_functions` into the table function at construction time. The execution plan passes it to the inner `make_session_context` call, which is required because `ThreadSpansView::jit_update` bails if `query_range` is `None`.

### Output schema

`stream_id` and `thread_name` prepended to the existing `thread_spans` schema:

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

### Architecture

```
process_thread_spans('pid')
        |
        v
ProcessThreadSpansTableFunction::call()     -- parses process_id; has query_range from constructor
        |
        v
ProcessThreadSpansTableProvider              -- wraps execution plan
        |
        v  (on scan)
ProcessThreadSpansExecutionPlan::execute()   -- async stream
        |
        v
make_session_context(query_range)            -- full query context with time range
        |
        v
get_process_thread_list()                    -- find CPU streams from blocks table
        |
        v
for each (stream_id, thread_name):           -- parallel with buffered()
    ctx.sql("SELECT * FROM view_instance('thread_spans', stream_id)")
        |
        v
    augment_batch(batch, stream_id, thread_name)  -- prepend identity columns
        |
        v
    yield augmented batch
```

### Batch augmentation

For each batch returned by a per-stream query, prepend two constant dictionary columns:

```rust
fn augment_batch(
    batch: &RecordBatch,
    schema: SchemaRef,
    stream_id: &str,
    thread_name: &str,
) -> Result<RecordBatch> {
    let n = batch.num_rows();
    let stream_id_array: DictionaryArray<Int16Type> =
        std::iter::repeat_n(Some(stream_id), n).collect();
    let thread_name_array: DictionaryArray<Int16Type> =
        std::iter::repeat_n(Some(thread_name), n).collect();
    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(stream_id_array),
        Arc::new(thread_name_array),
    ];
    columns.extend(batch.columns().iter().cloned());
    RecordBatch::try_new(schema, columns)
}
```

### Shared utilities

`get_process_thread_list` is currently a private function in `perfetto_trace_execution_plan.rs`. It needs to be made `pub(crate)` so the new module can reuse it.

`format_thread_spans_query` is **not** reused — it selects only 6 columns (`begin, end, name, filename, target, line`) for Perfetto rendering, but the output schema requires all 11 span columns. The new module uses `SELECT * FROM view_instance('thread_spans', stream_id)` instead.

## Implementation Steps

### Step 1: Expose shared utilities

In `perfetto_trace_execution_plan.rs`, change visibility of:
- `get_process_thread_list` → `pub(crate)`

### Step 2: Create the table function module

New file: `rust/analytics/src/lakehouse/process_thread_spans_table_function.rs`

Contains three types:

**`ProcessThreadSpansTableFunction`** (implements `TableFunctionImpl`)
- Holds: `lakehouse`, `view_factory`, `part_provider`, `query_range` (same fields as `ViewInstanceTableFunction`)
- `call(exprs)`: parses single `process_id` string argument, constructs `ProcessThreadSpansExecutionPlan`, wraps in `ProcessThreadSpansTableProvider`
- Defines `output_schema()`: builds schema with stream_id + thread_name + `get_spans_schema()` fields

**`ProcessThreadSpansExecutionPlan`** (implements `ExecutionPlan`)
- Holds: `schema`, `process_id`, `query_range`, `lakehouse`, `view_factory`, `part_provider`, `properties`
- `execute()`: returns a `RecordBatchStreamAdapter` wrapping an async stream that:
  1. Calls `make_session_context` with `query_range` (same as perfetto does at line 242-252, using `NoOpSessionConfigurator`)
  2. Calls `get_process_thread_list` to discover CPU streams
  3. For each stream, builds SQL: `SELECT * FROM view_instance('thread_spans', '{stream_id}')`
  4. Runs queries in parallel using `stream::iter().map().buffered(max_concurrent)` (same pattern as perfetto line 422-436)
  5. For each stream's result batches, calls `augment_batch` to prepend stream identity
  6. Yields augmented batches

**`ProcessThreadSpansTableProvider`** (implements `TableProvider`)
- Same boilerplate as `PerfettoTraceTableProvider` (wraps execution plan, applies `GlobalLimitExec` if limit is provided)

### Step 3: Register the table function

In `query.rs`, add to `register_lakehouse_functions`:
```rust
ctx.register_udtf(
    "process_thread_spans",
    Arc::new(ProcessThreadSpansTableFunction::new(
        lakehouse.clone(),
        view_factory.clone(),
        part_provider.clone(),
        query_range,
    )),
);
```

### Step 4: Add module declaration

In `lakehouse/mod.rs`, add:
```rust
/// Table function returning thread spans from all CPU streams of a process
pub mod process_thread_spans_table_function;
```

### Step 5: Update documentation

Add `process_thread_spans` entry to `mkdocs/docs/query-guide/functions-reference.md` after the `perfetto_trace_chunks` entry, and add it to `mkdocs/docs/query-guide/schema-reference.md` under thread_spans.

## Files to Modify

| File | Change |
|------|--------|
| `rust/analytics/src/lakehouse/process_thread_spans_table_function.rs` | **New** — table function, execution plan, table provider |
| `rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs` | Make `get_process_thread_list` `pub(crate)` |
| `rust/analytics/src/lakehouse/query.rs` | Register `process_thread_spans` UDTF in `register_lakehouse_functions` |
| `rust/analytics/src/lakehouse/mod.rs` | Add `pub mod process_thread_spans_table_function;` |
| `mkdocs/docs/query-guide/functions-reference.md` | Document the new table function |
| `mkdocs/docs/query-guide/schema-reference.md` | Note process-level access for thread spans |

## Trade-offs

**Table function vs. materialized view (like async_events)**:
The materialized view approach would create new per-process partitions with stream identity baked in. This adds storage cost, partition management complexity, and the tricky problem of grouping contiguous blocks per stream for correct call trees. The table function approach reuses existing per-stream materialization with zero storage overhead. The tradeoff is slightly higher query latency since streams are discovered and queried at runtime. For notebook use cases where this runs once and results are cached in the WASM engine, this is acceptable.

**Sharing `get_process_thread_list` from perfetto module vs. duplicating**:
Making `get_process_thread_list` `pub(crate)` is cleaner than duplicating. This function is general-purpose (find CPU streams for a process) and not Perfetto-specific. `format_thread_spans_query` is not reused because it selects only the 6 columns needed for Perfetto rendering, not the full span schema. If more consumers appear, shared functions could move to a dedicated module, but `pub(crate)` is sufficient for now.

## Documentation

- `mkdocs/docs/query-guide/functions-reference.md` — add `process_thread_spans(process_id)` entry (note: time range is provided out of band via the query's begin/end parameters)
- `mkdocs/docs/query-guide/schema-reference.md` — note under `thread_spans` that process-level access is available via `process_thread_spans()`

## Testing Strategy

1. `cargo build` — compiles
2. `cargo clippy --workspace -- -D warnings` — no warnings
3. `cargo fmt` — formatted
4. `cargo test` — existing tests pass
5. Manual: start services, run `SELECT * FROM process_thread_spans('some-process-id') LIMIT 10` and verify:
   - `stream_id` and `thread_name` columns are present and populated
   - Multiple distinct `stream_id` values appear (one per thread)
   - Span data matches what `view_instance('thread_spans', stream_id)` returns for individual streams

## Open Questions

None — design decisions were resolved during the earlier design discussion.
