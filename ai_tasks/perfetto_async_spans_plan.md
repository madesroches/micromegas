# Perfetto Async Spans Generation Plan

## Overview

Generate Perfetto trace files from a process's async span events by extending the existing Perfetto trace generation functionality to include async span events alongside the current thread-based spans.

## Current State Analysis

### Existing Perfetto Infrastructure
- **Rust Perfetto crate** (`rust/perfetto/`): Complete protobuf-based Perfetto trace writer
- **Writer API**: Supports process descriptors, thread descriptors, and span events
- **Client integration**: `perfetto_trace_client.rs` generates traces from thread spans using `view_instance('thread_spans', '{stream_id}')`

### Existing Async Events Infrastructure  
- **AsyncEventRecord structure**: Contains `stream_id`, `block_id`, `time`, `event_type`, `span_id`, `parent_span_id`, `depth`, `name`, `filename`, `target`, `line`
- **Async Events View**: `AsyncEventsView` provides materialized view access to async span events
- **Event Types**: "begin" and "end" events mark async span boundaries

## Implementation Plan

### Phase 1: Extend Perfetto Writer for Async Events

**Objective**: Add async track support to the existing Perfetto writer

**Tasks**:
1. **Add async track creation** in `rust/perfetto/src/writer.rs`:
   - Create `append_async_track_descriptor()` method for async event tracks
   - Async tracks use different UUIDs from thread tracks (hash span_id + stream_id)
   - Parent async tracks to their originating thread track

2. **Add async span event methods**:
   - `append_async_span_begin()` - for "begin" events
   - `append_async_span_end()` - for "end" events
   - Use `track_event::Type::SliceBegin` and `track_event::Type::SliceEnd`

3. **Handle nested async spans**:
   - Each unique `span_id` gets its own track
   - Use `parent_span_id` to establish track hierarchy in Perfetto
   - Leverage existing `depth` field for visual organization

### Phase 2: Extend Client Integration

**Objective**: Update `perfetto_trace_client.rs` to include async events

**Tasks**:
1. **Add async events query** to `format_perfetto_trace()`:
   - Query `view_instance('async_events', '{process_id}')` for async span events
   - Filter events within query time range
   - Group by `stream_id` to match with threads

2. **Create async tracks per span**:
   - For each unique `span_id`, create an async track descriptor
   - Parent the async track to the appropriate thread track via `stream_id`

3. **Generate async span events**:
   - Match "begin" and "end" events by `span_id`
   - Create SliceBegin/SliceEnd events on the appropriate async track
   - Handle orphaned events (begin without end, end without begin)

### Phase 3: Data Processing Optimization

**Objective**: Ensure efficient async event processing for large traces

**Tasks**:
1. **Optimize async events query**:
   - Add time-range filtering to async events view query
   - Sort by `time` for efficient processing
   - Consider stream-by-stream processing for memory efficiency

2. **Handle async span completion**:
   - Create synthetic end events for incomplete async spans
   - Use process termination time as fallback end time
   - Log warnings for incomplete spans

3. **Memory optimization**:
   - Process async events in batches to avoid loading all events in memory
   - Use streaming approach for large processes with many async events

### Phase 4: Testing and Validation

**Objective**: Ensure generated traces are valid and useful

**Tasks**:
1. **Unit tests**:
   - Test async track creation with various span hierarchies
   - Test event matching (enter/exit pairs)
   - Test handling of incomplete spans

2. **Integration tests**:
   - Generate traces from test processes with async spans
   - Validate traces open correctly in Perfetto UI (ui.perfetto.dev)
   - Verify async spans appear as separate tracks under threads

3. **Performance testing**:
   - Test with processes containing thousands of async spans
   - Measure memory usage and processing time
   - Compare with thread-only trace generation

## Technical Design Details

### Async Track UUID Generation
```rust
fn async_track_uuid(stream_id: &str, span_id: i64) -> u64 {
    xxh64(format!("{}:{}", stream_id, span_id).as_bytes(), 0)
}
```

**Note**: No ID namespacing is needed between thread spans and async spans. While thread span IDs (from call tree analysis) and async span IDs (from `G_ASYNC_SPAN_COUNTER`) could theoretically overlap, this is not a problem because:
- Thread spans and async spans exist on completely different tracks in Perfetto
- Each track has its own unique UUID (thread tracks from `stream_id`, async tracks from `stream_id:span_id`)
- Perfetto tracks are independent - having the same span ID on different tracks is perfectly valid
- The current Perfetto writer doesn't embed span IDs in trace events, only track UUIDs

### Perfetto Track Hierarchy
```
Process Track (process_uuid)
├── Thread Track 1 (thread_uuid_1)
│   ├── Async Track A (span_1)
│   ├── Async Track B (span_2)
│   └── Async Track C (span_3)
└── Thread Track 2 (thread_uuid_2)
    ├── Async Track D (span_4)
    └── Async Track E (span_5)
```

### Event Matching Strategy
1. Collect all async events for a process within time range
2. Group by `span_id` to create event pairs
3. For each span_id:
   - Find "begin" event → SliceBegin
   - Find "end" event → SliceEnd  
   - Create async track if not exists
   - Generate begin/end events on async track

**Note**: Since async spans can have different `parent_span_id` values between their "begin" and "end" events (async operations can be spawned in one context and completed in another), always use the `parent_span_id` from the "begin" event for establishing track hierarchy and parent relationships. In the future, the "end" event's `parent_span_id` could be used for critical path analysis to understand which context was blocking on the async operation's completion.

### SQL Query for Async Events
```sql
SELECT stream_id, time, event_type, span_id, parent_span_id, depth, 
       name, filename, target, line
FROM view_instance('async_events', '{process_id}')
WHERE time >= {begin_time} AND time <= {end_time}
ORDER BY time ASC
```

## Expected Outcomes

1. **Enhanced Perfetto traces** showing both thread execution and async operations
2. **Async span visualization** as separate tracks in Perfetto UI  
3. **Hierarchical async spans** with proper parent-child relationships
4. **Temporal correlation** between thread activity and async operations
5. **Improved debugging** of async performance issues and concurrency patterns

## Migration Strategy

- **Backward compatible**: Existing thread-only trace generation continues to work
- **Opt-in async events**: Add flag/parameter to include async events in traces
- **Gradual rollout**: Test with small processes before enabling for large-scale traces

## Dependencies

- Existing `rust/perfetto` crate and writer infrastructure
- Existing `AsyncEventsView` and async events data pipeline
- Existing `perfetto_trace_client.rs` query infrastructure
- No new external dependencies required