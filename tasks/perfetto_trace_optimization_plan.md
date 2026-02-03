# Perfetto Trace Optimization Plan for Multi-Thread Processes

## Problem Statement

Generating Perfetto traces for processes with ~50 threads is slow due to sequential processing in the current implementation.

## Current Architecture - End-to-End Latency Path

### 1. Web App Request (analytics-web-app)
```
User clicks "Open in Perfetto" or "Download"
    ↓
POST /api/perfetto/{process_id}/generate
    Body: { time_range, include_async_spans, include_thread_spans }
```
**Location**: `analytics-web-app/src/routes/PerformanceAnalysisPage.tsx:584-641`

### 2. Backend HTTP Handler (analytics-web-srv)
```
generate_trace() → generate_trace_stream()
    ↓
Creates FlightSQL client connection
    ↓
Executes streaming SQL query
```
**Location**: `rust/analytics-web-srv/src/main.rs:598-695`

### 3. FlightSQL Query Execution
```sql
SELECT chunk_id, chunk_data
FROM perfetto_trace_chunks('{process_id}', '{span_types}', TIMESTAMP '{begin}', TIMESTAMP '{end}')
```
**Location**: `rust/public/src/client/perfetto_trace_client.rs:87-146`

### 4. PerfettoTraceTableFunction (DataFusion UDTF)
```
Parses SQL arguments → Creates PerfettoTraceExecutionPlan
```
**Location**: `rust/analytics/src/lakehouse/perfetto_trace_table_function.rs`

### 5. PerfettoTraceExecutionPlan::execute() ← **MAIN BOTTLENECK AREA**
```
generate_perfetto_trace_stream()
    ↓
Spawns background task → generate_streaming_perfetto_trace()
    ↓
Streams chunks via mpsc channel (16 chunk capacity, 8KB per chunk)
```
**Location**: `rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs:134-222`

### 6. Trace Generation Flow (THE CRITICAL PATH)
```rust
// rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs:225-285

async fn generate_streaming_perfetto_trace() {
    // 1. Create DataFusion session context
    let ctx = make_session_context(...).await?;

    // 2. Get process exe name (single query)
    let process_exe = get_process_exe(&process_id, &ctx).await?;

    // 3. Get thread list ← First sequential query
    let threads = get_process_thread_list(&process_id, &ctx).await?;

    // 4. Emit thread descriptors (fast, in-memory)
    for (stream_id, thread_id, thread_name) in &threads {
        writer.emit_thread_descriptor(...).await?;
    }

    // 5. ⚠️ BOTTLENECK: Sequential per-thread span queries
    if matches!(span_types, SpanTypes::Thread | SpanTypes::Both) {
        generate_thread_spans_with_writer(..., &threads).await?;  // ← SLOW
    }

    // 6. Async spans (single query with self-join)
    if matches!(span_types, SpanTypes::Async | SpanTypes::Both) {
        generate_async_spans_with_writer(...).await?;
    }
}
```

### 7. Thread Spans Generation ← **PRIMARY BOTTLENECK**
```rust
// rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs:373-435

async fn generate_thread_spans_with_writer(..., threads: &Vec<(String, i32, String)>) {
    for (stream_id, _, _) in threads {  // ← SEQUENTIAL LOOP
        writer.set_current_thread(stream_id);

        // Each iteration executes:
        let sql = format!(r#"
            SELECT "begin", "end", name, filename, target, line
            FROM view_instance('thread_spans', '{stream_id}')  // ← TRIGGERS JIT MATERIALIZATION
            WHERE begin <= TIMESTAMP '{end}' AND end >= TIMESTAMP '{begin}'
            ORDER BY begin
        "#);

        let df = ctx.sql(&sql).await?;  // ← SQL PARSE + PLAN
        let mut stream = df.execute_stream().await?;  // ← JIT UPDATE + PARTITION READ

        while let Some(batch_result) = stream.next().await {
            // Emit spans to writer (10-span batches)
        }
    }
}
```

### 8. view_instance() Table Function → JIT Materialization
```rust
// rust/analytics/src/lakehouse/view_instance_table_function.rs:50-81

impl TableFunctionImpl for ViewInstanceTableFunction {
    fn call(&self, exprs: &[Expr]) -> Result<Arc<dyn TableProvider>> {
        let view = self.view_factory.make_view(&view_set_name, &view_instance_id)?;
        Ok(Arc::new(MaterializedView::new(...)))
    }
}
```

### 9. MaterializedView::scan() → JIT Update + Partition Read
```rust
// rust/analytics/src/lakehouse/materialized_view.rs:66-99

async fn scan(...) {
    // ⚠️ JIT UPDATE: Checks/creates partitions - potentially VERY SLOW
    self.view.jit_update(self.lakehouse.clone(), self.query_range).await?;

    // Fetch partition list
    let partitions = self.part_provider.fetch(...).await?;

    // Create execution plan for reading partitions
    make_partitioned_execution_plan(...)
}
```

### 10. ThreadSpansView::jit_update() → Partition Materialization
```rust
// rust/analytics/src/lakehouse/thread_spans_view.rs:224-272

async fn jit_update(&self, lakehouse, query_range) {
    let stream = find_stream(&pool, self.stream_id).await?;  // DB query
    let process = find_process(&pool, &stream.process_id).await?;  // DB query
    let convert_ticks = make_time_converter_from_db(&pool, &process).await?;  // DB query

    // Generate JIT partition specs
    let partitions = generate_stream_jit_partitions(...).await?;  // Multiple DB queries

    // ⚠️ SEQUENTIAL: Update each partition
    for part in &partitions {
        update_partition(lake, view_meta, schema, &convert_ticks, part).await?;
    }
}
```

### 11. Parquet File Reading
```rust
// rust/analytics/src/lakehouse/reader_factory.rs

// ReaderFactory with caching layers:
// - MetadataCache: Caches parquet metadata (reduces DB fetches)
// - FileCache: 200MB LRU cache for file contents (reduces object storage reads)

impl AsyncFileReader for ParquetReader {
    fn get_bytes(&mut self, range) -> BoxFuture<Result<Bytes>> {
        // CachingReader handles cache lookup/population
        inner.get_bytes(range).await
    }
}
```

## Identified Bottlenecks (50 Threads)

| Step | Operation | Relative Cost |
|------|-----------|---------------|
| 1 | SQL parse + plan | Low |
| 2 | JIT partition check | Medium |
| 3 | Partition materialization (if needed) | High |
| 4 | Parquet read (cache miss) | High |
| 5 | Parquet read (cache hit) | Low |
| 6 | Span emission | Low |

Cold cache with new partitions is significantly slower than warm cache with existing partitions.

## Proposed Optimization: Reduce Per-Query Overhead

### Constraint: Sequential JIT Materialization

The `view_instance()` table function uses **optimistic locking** for JIT partition materialization. Concurrent calls to `view_instance()` for different threads would cause lock conflicts. This rules out parallel approaches.

### Why Sequential is Required

| Approach | JIT Safety | Fragility |
|----------|------------|-----------|
| Sequential loop | ✅ Safe | None - explicit control |
| UNION ALL | ⚠️ Risky | DataFusion could parallelize/reorder branches in future versions |
| Parallel prefetch | ❌ Lock conflicts | N/A |

**UNION ALL was considered but rejected**: DataFusion's execution order for UNION ALL branches is an implementation detail, not a guarantee. Future versions could parallelize or reorder branches for optimization, breaking the optimistic locking assumption.

### Current Implementation

```rust
for (stream_id, _, _) in threads {
    let sql = format!("SELECT ... FROM view_instance('thread_spans', '{stream_id}') ...");
    let df = ctx.sql(&sql).await?;  // Parse + plan
    let stream = df.execute_stream().await?;  // JIT + read
    // ... emit spans
}
```

Each iteration incurs:
- SQL parsing overhead
- String allocation for query
- JIT materialization (sequential, required) - **dominant cost**

### Solution: Pipelined Query Planning

JIT materialization happens during `execute_stream()`, not during `ctx.sql()`. This means we can **pipeline** SQL parsing with data streaming:

- While streaming thread N's data → plan thread N+1's query
- JIT executions remain sequential (required for locking)
- SQL parsing overhead is hidden behind streaming

```rust
// Proposed change to perfetto_trace_execution_plan.rs

use futures::{stream, StreamExt};

async fn generate_thread_spans_with_writer(
    ctx: &SessionContext,
    writer: &mut PerfettoTraceWriter,
    threads: &[(String, i32, String)],
    time_range: &TimeRange,
) -> anyhow::Result<()> {
    // Create stream of planning futures, buffered 1 ahead
    let mut planned = stream::iter(threads.iter())
        .map(|(stream_id, _, _)| {
            let ctx = ctx.clone();
            let sql = format_thread_query(stream_id, time_range);
            async move {
                let df = ctx.sql(&sql).await?;
                Ok::<_, anyhow::Error>((stream_id.clone(), df))
            }
        })
        .buffered(10);  // Plan up to 10 ahead while streaming

    // Process each planned query sequentially
    while let Some(result) = planned.next().await {
        let (stream_id, df) = result?;

        // JIT happens here in execute_stream - sequential as required
        writer.set_current_thread(&stream_id);
        let mut data_stream = df.execute_stream().await?;

        while let Some(batch_result) = data_stream.next().await {
            emit_batch_spans(writer, &batch_result?).await?;
        }
    }

    Ok(())
}
```

**How `buffered(10)` pipelines:**

```
Thread 1:  [plan]──[JIT+stream data]
Thread 2:  [plan]─────────────────[JIT+stream data]
Thread 3:  [plan]────────────────────────────────[JIT+stream]
...
Thread 10: [plan]────────────────────────────────...
           ↑
           all 10 plans start immediately, execute sequentially
```

### Expected Improvement

SQL parsing overhead per thread is hidden behind the previous thread's data streaming, which typically takes longer than parsing.

### Remaining Bottleneck

After pipelining, the dominant cost is **JIT materialization** (varies significantly based on partition state):

1. JIT partition checks (DB queries to verify partition state)
2. Partition materialization (if partitions don't exist)
3. Parquet file reads (cache misses are much slower than hits)

These costs are sequential due to optimistic locking. Further optimization requires changes to JIT itself (see Future Improvements).

## Implementation Status

### ✅ Completed

#### 1. Pipelined Query Planning
**File**: `rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs`

Implemented `buffered(10)` pipelining for SQL parsing:
- Pre-compute all query strings upfront to avoid lifetime issues
- Use `stream::iter(queries).map(...).buffered(10)` to plan up to 10 DataFrames ahead
- JIT materialization remains sequential in `execute_stream()` (required for locking)
- SQL parsing overhead is hidden behind data streaming

#### 2. Removed Unnecessary `arrow_cast` Calls
**File**: `rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs`

Removed `arrow_cast(..., 'Utf8')` from all queries:
- Thread spans query: `name`, `filename`, `target` columns
- Async spans query: `name`, `filename`, `target` columns
- Process exe query: `exe` column
- Thread list query: `stream_id`, `thread_name`, `thread_id` columns

This preserves dictionary encoding, reducing memory usage and avoiding unnecessary string materialization.

#### 3. Extended Dictionary Key Type Support
**File**: `rust/analytics/src/dfext/string_column_accessor.rs`

Made `DictionaryStringAccessor` generic over `ArrowDictionaryKeyType`:
- Now supports `Int8`, `Int16`, `Int32`, `Int64` dictionary keys
- Previously only supported `Int32`, causing runtime errors with `Int16` keys

## Metrics to Track

1. **Trace generation time**: End-to-end from request to complete trace
2. **Per-thread time breakdown**: SQL parsing vs JIT vs data streaming
3. **JIT materialization time**: Time spent checking/creating partitions (dominant cost)
4. **File cache hit rate**: `file_cache_hit / (file_cache_hit + file_cache_miss)`
5. **Object storage read latency**: Time for cache misses

## Testing Strategy

1. Create test process with 50+ threads and known span counts
2. Benchmark cold cache (first trace after restart)
3. Benchmark warm cache (repeated trace generation)
4. Compare before/after for pipelined query planning
5. Verify JIT executions remain sequential (no lock conflicts)

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Planning completes before streaming finishes | No harm - just awaits the already-completed future |
| Error in next query planning | Errors surface when we await the future, after current thread completes |

## Future Improvements

The following optimizations are deferred for future iterations:

### JIT Partition Metadata Caching (High Impact)

Cache partition state metadata to avoid repeated DB queries during JIT checks. Currently each `view_instance()` call queries the database to verify partition state. Caching this for the duration of a trace request would reduce per-thread overhead significantly.

### Batch Partition State Query (Medium Impact)

Instead of checking partition state per-thread, query all thread partition states in a single DB call at the start of trace generation. This trades one larger query for N smaller ones.

### File Cache Capacity (Low Effort, Medium Impact)

Increase default cache size from 200MB to 500MB+ for trace workloads. Make cache size configurable.

### Async Spans Query Optimization (Medium Impact)

Rewrite the self-join on `async_events` as a window function or aggregation for ~2x speedup:

```sql
SELECT span_id,
       MIN(CASE WHEN event_type = 'begin' THEN time END) as begin_time,
       MAX(CASE WHEN event_type = 'end' THEN time END) as end_time,
       ...
FROM view_instance('async_events', '{process_id}')
GROUP BY span_id
```

### Progress Reporting (UX Improvement)

Report progress through the streaming response for visibility into trace generation:
```json
{ "type": "progress", "message": "Processing thread 15/50: main-thread" }
```
