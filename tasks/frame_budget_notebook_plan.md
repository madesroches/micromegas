# CPU Frame Budget Notebook

## Context

Need a notebook to compute per-function CPU frame budgets for Unreal Engine games. Each function's "frame budget" is the sum of its elapsed time within a single `FEngineLoop::Tick`. This requires:
1. Identifying frame boundaries from game thread spans
2. Assigning ALL threads' spans (including workers) to frames
3. Using inclusive duration (not self-time, which mostly measures missing instrumentation)

**Key constraint**: `thread_spans` is per-stream only (no process-level query). We create a new table function that fans out to all CPU streams at query time, following the `perfetto_trace_chunks` pattern.

## Part 1: Backend — `process_thread_spans` table function

Create a table function `process_thread_spans(process_id)` that queries all CPU streams' thread spans and returns them with stream identity columns. No new materialized views — this reuses existing per-stream `thread_spans` JIT materialization at query time.

### SQL interface
```sql
SELECT stream_id, thread_name, id, parent, depth, hash, begin, "end", duration, name, target, filename, line
FROM process_thread_spans('$process_id')
```

### Schema

Existing `thread_spans` columns + 2 new columns prepended:

| Column | Type | Description |
|--------|------|-------------|
| `stream_id` | `Dictionary(Int16, Utf8)` | Stream UUID |
| `thread_name` | `Dictionary(Int16, Utf8)` | Thread name from stream properties |
| `id` | `Int64` | Span identifier (unique within stream) |
| `parent` | `Int64` | Parent span identifier |
| `depth` | `UInt32` | Nesting depth in call tree |
| `hash` | `UInt32` | Span hash for deduplication |
| `begin` | `Timestamp(Nanosecond, UTC)` | Span start time |
| `end` | `Timestamp(Nanosecond, UTC)` | Span end time |
| `duration` | `Int64` | Span duration in nanoseconds |
| `name` | `Dictionary(Int16, Utf8)` | Function/scope name |
| `target` | `Dictionary(Int16, Utf8)` | Module/target |
| `filename` | `Dictionary(Int16, Utf8)` | Source file |
| `line` | `UInt32` | Line number |

### Implementation — follows `perfetto_trace_chunks` pattern

**Primary reference**: `rust/analytics/src/lakehouse/perfetto_trace_table_function.rs` + `perfetto_trace_execution_plan.rs`

The Perfetto trace generation already does exactly what we need:
1. Takes a `process_id` literal (table function)
2. Creates a `SessionContext` inside the execution plan
3. Queries blocks to find CPU streams via `get_process_thread_list` (line 316-371)
4. For each stream, queries `view_instance('thread_spans', stream_id)` in parallel (line 400-436)
5. Streams results back as RecordBatches

**New files:**

1. **`rust/analytics/src/lakehouse/process_thread_spans_table_function.rs`**
   - `ProcessThreadSpansTableFunction` — implements `TableFunctionImpl`
     - `call(exprs)`: parses `process_id` literal, creates `ProcessThreadSpansExecutionPlan`
   - `ProcessThreadSpansExecutionPlan` — implements `ExecutionPlan`
     - Holds: schema, process_id, lakehouse, view_factory, part_provider
     - `execute()`: spawns async stream that:
       1. Creates `SessionContext` via `make_session_context` (like perfetto does at line 242-252)
       2. Calls `get_process_thread_list` to find CPU streams (reuse from perfetto module, or extract to shared util)
       3. For each `(stream_id, thread_name)`, queries `view_instance('thread_spans', stream_id)` in parallel with `buffered(max_concurrent)` (same pattern as perfetto line 422-436)
       4. For each result batch, prepends `stream_id` and `thread_name` constant columns
       5. Yields the augmented batch
   - `ProcessThreadSpansTableProvider` — wraps execution plan (same pattern as `PerfettoTraceTableProvider`)

**Modified files:**

2. **`rust/analytics/src/lakehouse/query.rs`**
   - Register the new table function alongside existing ones:
     ```rust
     ctx.register_udtf(
         "process_thread_spans",
         Arc::new(ProcessThreadSpansTableFunction::new(lakehouse, view_factory, part_provider)),
     );
     ```

3. **`rust/analytics/src/lakehouse/mod.rs`** — Add `mod process_thread_spans_table_function;`

### Key logic: augmenting batches with stream identity

For each stream's result batch, add two constant columns:
```rust
fn augment_batch(batch: RecordBatch, stream_id: &str, thread_name: &str) -> Result<RecordBatch> {
    let n = batch.num_rows();
    // Create constant dictionary arrays for stream_id and thread_name
    let stream_id_array = DictionaryArray::from_iter(std::iter::repeat_n(Some(stream_id), n));
    let thread_name_array = DictionaryArray::from_iter(std::iter::repeat_n(Some(thread_name), n));
    // Prepend to existing columns
    let mut columns = vec![Arc::new(stream_id_array), Arc::new(thread_name_array)];
    columns.extend(batch.columns().iter().cloned());
    RecordBatch::try_new(output_schema, columns)
}
```

### Functions to reuse
- `get_process_thread_list` from `perfetto_trace_execution_plan.rs:316` — finds CPU streams for a process
- `format_thread_spans_query` from `perfetto_trace_execution_plan.rs:374` — builds per-stream SQL
- `make_session_context` from `query.rs` — creates query context with all views registered
- Parallel query pattern from `generate_thread_spans_with_writer` (perfetto line 400-436)

### Reference implementations
- `rust/analytics/src/lakehouse/perfetto_trace_table_function.rs` — table function pattern
- `rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs` — execution plan + per-stream parallel queries
- `rust/public/src/client/frame_budget_reporting.rs` — frame assignment logic (fully-contained policy)

---

## Part 2: Notebook Design

### Cell Structure

#### Section: Setup
| # | Name | Type | DataSource | Purpose |
|---|------|------|------------|---------|
| 1 | intro | Markdown | — | Title + instructions |
| 2 | process_id | Variable (Combobox) | server | Select process |
| 3 | game_thread_name | Variable (Text) | — | Default: `GameThread` |
| 4 | frame_span_name | Variable (Text) | — | Default: `FEngineLoop::Tick` |

#### Section: Data Fetch
| # | Name | Type | DataSource | Purpose |
|---|------|------|------------|---------|
| 5 | all_spans | Table | server | Fetch ALL spans for process |
| 6 | frames | Table | notebook | Extract frame boundaries |

#### Section: Frame Overview
| # | Name | Type | DataSource | Purpose |
|---|------|------|------------|---------|
| 7 | frame_time_chart | Chart | notebook | Frame time over time |
| 8 | frame_stats | Transposed Table | notebook | Frame time statistics |

#### Section: Game Thread Budget
| # | Name | Type | DataSource | Purpose |
|---|------|------|------------|---------|
| 9 | gt_frame_budget | Table | notebook | Per-function budget (game thread) |
| 10 | gt_budget_chart | Chart | notebook | Top functions bar chart |

#### Section: Cross-Thread Budget
| # | Name | Type | DataSource | Purpose |
|---|------|------|------------|---------|
| 11 | all_thread_budget | Table | notebook | Per-function budget (all threads) |
| 12 | thread_breakdown | Table | notebook | Budget grouped by thread |

### Key SQL Queries

**Cell 2 (process_id combobox):**
```sql
SELECT arrow_cast(process_id, 'Utf8') as process_id,
       arrow_cast(exe, 'Utf8') as exe,
       arrow_cast(computer, 'Utf8') as computer
FROM processes
ORDER BY start_time DESC
LIMIT 50
```

**Cell 5 (all_spans — server query using new table function):**
```sql
SELECT stream_id, thread_name, id, parent, depth, begin, "end", duration, name
FROM process_thread_spans('$process_id.process_id')
```

**Cell 6 (frames — WASM, extract frame boundaries):**
```sql
SELECT
  ROW_NUMBER() OVER (ORDER BY begin) as frame_number,
  begin as frame_begin,
  "end" as frame_end,
  duration / 1000000.0 as frame_time_ms
FROM all_spans
WHERE thread_name = '$game_thread_name'
  AND name = '$frame_span_name'
  AND depth = 0
ORDER BY begin
```

**Cell 9 (gt_frame_budget — WASM, game thread per-function inclusive budget):**
```sql
SELECT
  s.name as function_name,
  COUNT(DISTINCT f.frame_number) as frames_present,
  CAST(COUNT(*) AS DOUBLE) / COUNT(DISTINCT f.frame_number) as avg_calls_per_frame,
  SUM(s.duration) / 1000000.0 / COUNT(DISTINCT f.frame_number) as avg_ms_per_frame,
  MAX(s.duration) / 1000000.0 as max_call_ms,
  SUM(s.duration) / 1000000.0 as total_ms
FROM all_spans s
JOIN frames f ON s.begin >= f.frame_begin AND s."end" <= f.frame_end
WHERE s.thread_name = '$game_thread_name'
GROUP BY s.name
ORDER BY avg_ms_per_frame DESC
LIMIT 50
```

**Cell 11 (all_thread_budget — WASM, cross-thread per-function budget):**
```sql
SELECT
  s.name as function_name,
  COUNT(DISTINCT f.frame_number) as frames_present,
  CAST(COUNT(*) AS DOUBLE) / COUNT(DISTINCT f.frame_number) as avg_calls_per_frame,
  SUM(s.duration) / 1000000.0 / COUNT(DISTINCT f.frame_number) as avg_ms_per_frame,
  SUM(s.duration) / 1000000.0 as total_ms
FROM all_spans s
JOIN frames f ON s.begin >= f.frame_begin AND s."end" <= f.frame_end
GROUP BY s.name
ORDER BY avg_ms_per_frame DESC
LIMIT 50
```

**Cell 12 (thread_breakdown — WASM, budget by thread):**
```sql
SELECT
  s.thread_name,
  COUNT(DISTINCT f.frame_number) as frames_active,
  SUM(s.duration) / 1000000.0 / (SELECT COUNT(*) FROM frames) as avg_ms_per_frame,
  SUM(s.duration) / 1000000.0 as total_ms
FROM all_spans s
JOIN frames f ON s.begin >= f.frame_begin AND s."end" <= f.frame_end
GROUP BY s.thread_name
ORDER BY avg_ms_per_frame DESC
```

### Frame assignment policy

Uses "fully contained" policy: `span.begin >= frame.begin AND span.end <= frame.end`. This matches the existing Rust reference implementation in `frame_budget_reporting.rs:218-219`. Spans crossing frame boundaries are excluded — this is conservative but avoids double-counting.

### Inclusive duration (no self-time)

Uses `duration` directly — each function's budget is its inclusive time (self + children). Self-time would mostly measure missing instrumentation since not everything is instrumented, making it misleading.

### Note on span ID uniqueness

Span `id` and `parent` values are only unique within a stream/thread. Any query joining on these columns must also join on `stream_id` to avoid cross-thread mismatches.

---

## Verification

1. **Backend**: `cd rust && cargo build` — verify new table function compiles
2. **Tests**: `cd rust && cargo test` — run existing tests
3. **Lint**: `cd rust && cargo clippy --workspace -- -D warnings`
4. **Format**: `cd rust && cargo fmt`
5. **Manual test**: Start services, query `SELECT * FROM process_thread_spans('some-process-id') LIMIT 10` to verify stream_id and thread_name columns are present
6. **Notebook**: Create the notebook in the web app, verify frame chart renders and budget tables populate correctly
