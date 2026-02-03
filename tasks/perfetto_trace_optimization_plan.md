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
    let ctx = make_session_context(...).await?;  // ~50-100ms

    // 2. Get process exe name (single query)
    let process_exe = get_process_exe(&process_id, &ctx).await?;  // ~20ms

    // 3. Get thread list ← First sequential query
    let threads = get_process_thread_list(&process_id, &ctx).await?;  // ~50-200ms for 50 threads

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

| Step | Operation | Est. Time per Thread | Total (50 threads) |
|------|-----------|---------------------|-------------------|
| 1 | SQL parse + plan | ~5ms | 250ms |
| 2 | JIT partition check | ~10-50ms | 500ms-2.5s |
| 3 | Partition materialization (if needed) | ~100-500ms | 5-25s |
| 4 | Parquet read (cache miss) | ~50-200ms | 2.5-10s |
| 5 | Parquet read (cache hit) | ~5-20ms | 250ms-1s |
| 6 | Span emission | ~1-10ms | 50-500ms |

**Worst case (cold cache, new partitions)**: 8-38 seconds
**Best case (warm cache, existing partitions)**: 0.5-2 seconds

## Proposed Optimizations

### Phase 1: Parallel Thread Span Queries with Buffered Ordered Streams (High Impact)

**Problem**: Sequential `for` loop over threads in `generate_thread_spans_with_writer()`

**Solution**: Use `futures::stream::StreamExt::buffered()` to execute span queries concurrently while preserving order for sequential emission to the writer.

**Why buffered ordered streams?**
- `buffered(n)` executes up to `n` futures concurrently but yields results **in original order**
- This is ideal because we need parallel I/O but must emit spans thread-by-thread to maintain writer state
- Simpler than manual batching with `join_all` - the stream handles backpressure automatically
- Memory-efficient: only buffers `n` results at a time, not all threads

```rust
// Proposed change to perfetto_trace_execution_plan.rs

use futures::{StreamExt, stream};

async fn generate_thread_spans_with_writer(...) {
    const CONCURRENCY: usize = 10;  // Configurable parallelism

    // Create a stream of futures that fetch spans for each thread
    let span_fetches = stream::iter(threads.iter())
        .map(|(stream_id, _, _)| {
            let ctx = ctx.clone();  // SessionContext is Arc-wrapped internally
            let stream_id = stream_id.clone();
            let time_range = time_range.clone();
            async move {
                let spans = fetch_thread_spans(&ctx, &stream_id, &time_range).await;
                (stream_id, spans)
            }
        })
        .buffered(CONCURRENCY);  // Execute up to N concurrently, yield in order

    // Pin the stream for iteration
    tokio::pin!(span_fetches);

    // Process results in order - parallel fetch, sequential emit
    while let Some((stream_id, spans_result)) = span_fetches.next().await {
        writer.set_current_thread(&stream_id);
        let batches = spans_result?;

        for batch in batches {
            emit_batch_spans(writer, &batch).await?;
        }
    }
}

async fn fetch_thread_spans(
    ctx: &SessionContext,
    stream_id: &str,
    time_range: &TimeRange
) -> anyhow::Result<Vec<RecordBatch>> {
    let sql = format!(
        r#"SELECT "begin", "end", name, filename, target, line
           FROM view_instance('thread_spans', '{}')
           WHERE begin <= TIMESTAMP '{}' AND end >= TIMESTAMP '{}'
           ORDER BY begin"#,
        stream_id, time_range.end.to_rfc3339(), time_range.begin.to_rfc3339()
    );
    let df = ctx.sql(&sql).await?;
    let batches = df.collect().await?;  // Collect all batches for this thread
    Ok(batches)
}
```

**Comparison: `buffered()` vs `buffer_unordered()`**
| Approach | Order | Use Case |
|----------|-------|----------|
| `buffered(n)` | Preserved | When results must be processed in order (our case) |
| `buffer_unordered(n)` | Fastest-first | When order doesn't matter |

**Expected improvement**: 5-10x speedup for span queries (limited by object storage concurrency)

### Phase 2: Parallel JIT Partition Materialization (High Impact)

**Problem**: `ThreadSpansView::jit_update()` is called sequentially per thread via `view_instance()`

**Solution**: Pre-materialize all thread partitions in parallel using `buffer_unordered()` before starting span queries.

**Why `buffer_unordered()` instead of `buffered()`?**
- Order doesn't matter - we just need all partitions ready before proceeding
- `buffer_unordered()` yields results as they complete (faster overall)
- Automatic concurrency limiting without manual batching
- Cleaner than `try_join_all` with `chunks()`

```rust
// Add new function to perfetto_trace_execution_plan.rs

use futures::{StreamExt, stream};

async fn prefetch_thread_partitions(
    lakehouse: &Arc<LakehouseContext>,
    view_factory: &Arc<ViewFactory>,
    threads: &[(String, i32, String)],
    query_range: Option<TimeRange>,
) -> anyhow::Result<()> {
    const CONCURRENCY: usize = 10;

    let materialization_futures = stream::iter(threads.iter())
        .map(|(stream_id, _, _)| {
            let lakehouse = lakehouse.clone();
            let view_factory = view_factory.clone();
            let stream_id = stream_id.clone();
            async move {
                let view = view_factory.make_view("thread_spans", &stream_id)?;
                view.jit_update(lakehouse, query_range).await
            }
        })
        .buffer_unordered(CONCURRENCY);  // Unordered - results yielded as they complete

    tokio::pin!(materialization_futures);

    // Consume all results, propagating any errors
    while let Some(result) = materialization_futures.next().await {
        result?;  // Fail fast on first error
    }

    Ok(())
}

// Call before generate_thread_spans_with_writer()
async fn generate_streaming_perfetto_trace(...) {
    // ... emit descriptors ...

    // Pre-materialize partitions in parallel
    prefetch_thread_partitions(&lakehouse, &view_factory, &threads, Some(time_range)).await?;

    // Now span queries will hit existing partitions
    generate_thread_spans_with_writer(...).await?;
}
```

**Comparison: Phase 1 vs Phase 2 patterns**
| Phase | Order Matters? | Pattern | Reason |
|-------|----------------|---------|--------|
| 1: Span queries | Yes | `buffered()` | Must emit to writer in thread order |
| 2: JIT materialization | No | `buffer_unordered()` | Just need all partitions ready |

**Expected improvement**: 3-5x speedup for partition materialization

### Phase 3: Shared Session Context with Pre-registered Views (Medium Impact)

**Problem**: Each `ctx.sql()` call parses SQL and resolves table functions.

**Solution**: Pre-register all thread views at session start.

```rust
// Pre-register thread views as named tables
async fn generate_streaming_perfetto_trace(...) {
    let ctx = make_session_context(...).await?;

    // Pre-register all thread views as named tables
    for (stream_id, _, _) in &threads {
        let view = view_factory.make_view("thread_spans", stream_id)?;
        let table_name = format!("thread_spans_{}", stream_id.replace('-', "_"));
        ctx.register_table(&table_name, Arc::new(MaterializedView::new(...)))?;
    }

    // Now queries use pre-registered tables (no table function resolution)
    let sql = format!("SELECT ... FROM {} WHERE ...", table_name);
}
```

**Expected improvement**: ~20% reduction in query overhead

### Phase 4: Increase File Cache Capacity (Low Effort, Medium Impact)

**Problem**: Default 200MB cache may not hold all thread partition files.

**Solution**: Make cache size configurable and increase default for trace workloads.

```rust
// rust/analytics/src/lakehouse/file_cache.rs

// Increase defaults for trace generation workloads
const DEFAULT_CACHE_SIZE_BYTES: u64 = 500 * 1024 * 1024;  // 500MB (was 200MB)
const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 20 * 1024 * 1024;  // 20MB (was 10MB)

// Add configurable constructor
impl FileCache {
    pub fn for_trace_generation() -> Self {
        Self::new(1024 * 1024 * 1024, 50 * 1024 * 1024)  // 1GB cache, 50MB max file
    }
}
```

**Expected improvement**: Better cache hit rates for repeated trace generation

### Phase 5: Async Spans Query Optimization (Medium Impact)

**Problem**: Self-join on `async_events` view can be slow.

**Solution**: Rewrite as window function or materialized join table.

```sql
-- Current: Self-join (slow)
WITH begin_events AS (SELECT ... WHERE event_type = 'begin'),
     end_events AS (SELECT ... WHERE event_type = 'end')
SELECT ... FROM begin_events b INNER JOIN end_events e ON b.span_id = e.span_id

-- Proposed: Window function (single scan)
SELECT
    span_id,
    MIN(CASE WHEN event_type = 'begin' THEN time END) as begin_time,
    MAX(CASE WHEN event_type = 'end' THEN time END) as end_time,
    FIRST_VALUE(name) FILTER (WHERE event_type = 'begin') as name,
    ...
FROM view_instance('async_events', '{process_id}')
WHERE time >= TIMESTAMP '{begin}' AND time <= TIMESTAMP '{end}'
GROUP BY span_id
HAVING MIN(CASE WHEN event_type = 'begin' THEN time END) IS NOT NULL
   AND MAX(CASE WHEN event_type = 'end' THEN time END) IS NOT NULL
```

**Expected improvement**: 2x speedup for async span queries

### Phase 6: Progress Reporting (UX Improvement)

**Problem**: No visibility into trace generation progress for large traces.

**Solution**: Report progress through the streaming response.

```typescript
// analytics-web-app/src/lib/api.ts
// Already supports progress messages, enhance backend to send them

// Backend sends:
{ "type": "progress", "message": "Processing thread 15/50: main-thread" }
{ "type": "progress", "message": "Fetching async spans..." }
{ "type": "binary_start" }
// binary data follows
```

## Implementation Priority

| Phase | Effort | Impact | Priority |
|-------|--------|--------|----------|
| 1: Parallel span queries | Medium | High | P0 |
| 2: Parallel JIT materialization | Medium | High | P0 |
| 4: Increase cache size | Low | Medium | P1 |
| 6: Progress reporting | Low | UX | P1 |
| 3: Pre-registered views | Medium | Medium | P2 |
| 5: Async spans optimization | Medium | Medium | P2 |

## Metrics to Track

1. **Trace generation time**: End-to-end from request to complete trace
2. **Per-thread query time**: Time spent in span queries
3. **JIT materialization time**: Time spent checking/creating partitions
4. **File cache hit rate**: `file_cache_hit / (file_cache_hit + file_cache_miss)`
5. **Object storage read latency**: Time for cache misses

## Testing Strategy

1. Create test process with 50+ threads and known span counts
2. Benchmark cold cache (first trace after restart)
3. Benchmark warm cache (repeated trace generation)
4. Compare before/after for each optimization phase
5. Monitor memory usage during parallel operations

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Memory pressure from parallel queries | Limit parallelism (PARALLEL_BATCH_SIZE) |
| Object storage rate limiting | Implement backoff, limit concurrent requests |
| Database connection exhaustion | Use connection pooling, limit parallel DB ops |
| Partial failures in parallel ops | Graceful degradation, continue with successful threads |
