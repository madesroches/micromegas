# Perfetto Trace Optimization: Parallel Thread Processing

## Problem

Generating Perfetto traces for processes with ~50 threads is slow due to sequential per-thread processing.

## Solution: Parallel JIT, Sequential Writing

### Key Discovery

The JIT partition locking uses **per-partition advisory locks** based on `(view_set_name, view_instance_id, insert_time_range)`. Since each thread has a unique `stream_id` (used as `view_instance_id`), different threads get different lock keys. **Parallel JIT is safe.**

See `rust/analytics/src/lakehouse/write_partition.rs:227-240`.

### Implementation

Build streams in parallel (triggering JIT), then consume sequentially:

```rust
let max_concurrent = std::thread::available_parallelism()
    .map(|n| n.get())
    .unwrap_or(4);

// Build streams in parallel - JIT happens during execute_stream()
let streams: Vec<(String, SendableRecordBatchStream)> = stream::iter(queries)
    .map(|(stream_id, sql)| {
        let ctx = ctx.clone();
        async move {
            let df = ctx.sql(&sql).await?;
            let stream = df.execute_stream().await?;
            Ok::<_, anyhow::Error>((stream_id, stream))
        }
    })
    .buffered(max_concurrent)
    .try_collect()
    .await?;

// Consume streams sequentially - each thread's spans written together
for (stream_id, mut data_stream) in streams {
    writer.set_current_thread(&stream_id);
    while let Some(batch_result) = data_stream.next().await {
        // ... emit spans ...
    }
}
```

### Why This Works

- **Parallel JIT**: Up to N threads' JIT runs concurrently (the slow part)
- **Sequential writing**: Each thread's spans written together (required)
- **Bounded memory**: Streams buffer ~1 RecordBatch each internally
- **Simple**: No channels, no manual chunking

## Status: ✅ Implemented

**File**: `rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs:388-468`

## Completed Optimizations

- **Parallel JIT**: `buffered(available_parallelism())` on stream creation
- **Removed `arrow_cast` calls**: Preserves dictionary encoding
- **Extended dictionary key types**: Supports Int8/Int16/Int32/Int64 keys

## Future Improvements

- **JIT metadata caching**: Cache partition state to reduce DB queries
- **Batch partition state query**: Single query for all threads' partition states
- **Async spans optimization**: Rewrite self-join as window function
