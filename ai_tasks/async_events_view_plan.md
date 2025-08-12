# Async Events View Plan

## Overview
Plan for implementing a view to visualize and analyze async events in the micromegas telemetry system.

## Goals
- Provide visibility into async operation lifecycles
- Track async call stacks and execution flow
- Identify performance bottlenecks in async operations
- Correlate async events with their initiating context

## View Instance Keying Options

Two approaches are being considered for keying the async events view:

### Option 1: Use `process_id` (Like LogView/MetricsView)

**Pattern**: Follow `LogView` and `MetricsView` approach
- Accept `process_id` as view_instance_id (or "global" for all processes)
- Aggregate async events from **ALL streams** within the process
- Use `list_process_streams_tagged()` to find all relevant streams

**Advantages**:
- **Cross-thread visibility**: Async operations often span multiple threads within a process (task spawning, thread pool migration)
- **Complete async picture**: Shows the full async execution flow across the entire process
- **Consistency**: Matches pattern used by LogView and MetricsView
- **Global view support**: Can support "global" view across all processes
- **Simpler UX**: Users don't need to know specific stream IDs
- **Task migration**: Captures async tasks that move between threads/streams

**Disadvantages**:
- **More data**: Higher volume of events to process and display
- **Less granular**: Can't focus on a specific thread's async behavior
- **Performance**: Potentially slower queries due to larger data sets

### Option 2: Use `stream_id` (Like ThreadSpansView)

**Pattern**: Follow `ThreadSpansView` approach
- Accept `stream_id` as view_instance_id
- Show async events from **ONE specific stream** only
- Parse as UUID to identify the specific stream

**Advantages**:
- **Granular control**: Precise filtering for debugging specific threads
- **Performance**: Faster queries with less data to process
- **Consistency**: Matches `ThreadSpansView` (async events relate to spans)
- **Thread-local context**: Aligns with per-thread async call stack tracking
- **Focused debugging**: Better for isolating issues in specific execution contexts

**Disadvantages**:
- **Limited visibility**: Misses async operations that span multiple threads
- **Complex UX**: Users need to know and specify stream IDs
- **Fragmented view**: May need multiple queries to understand full async flow
- **Task boundaries**: Can't see async tasks that migrate between streams

### Key Consideration: Async Runtime Behavior

Modern async runtimes (tokio, async-std) commonly:
- Spawn tasks that execute on different threads than where they were created
- Move futures between thread pools for load balancing
- Use work-stealing schedulers that migrate tasks between threads
- Coordinate across multiple streams within the same process

This suggests **process_id** might provide more valuable insights for async debugging and analysis.

## Current Direction: Exploring Option 1 (process_id)

We will start by exploring Option 1 in detail - using `process_id` for view instance keying. This approach aligns with the cross-process nature of async operations and provides a comprehensive view of async execution flow within a process boundary.

### Next Steps for Option 1 Implementation:
1. Design async events view following LogView/MetricsView pattern (process-scoped keying)
2. Implement process-scoped async event aggregation across all thread streams
3. Evaluate performance and usability
4. Compare results with Option 2 if needed

**Note**: The implementation will also be similar to ThreadSpansView since async span events are collected in the same thread streams. The key difference is aggregating data from **all thread streams** within a process (like LogView/MetricsView) rather than filtering to a single stream (like ThreadSpansView).

## Implementation Details

### Required Rust Structs

#### 1. AsyncEventsViewMaker
```rust
/// A `ViewMaker` for creating `AsyncEventsView` instances.
#[derive(Debug)]
pub struct AsyncEventsViewMaker {}

impl ViewMaker for AsyncEventsViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(AsyncEventsView::new(view_instance_id)?))
    }
}
```

#### 2. AsyncEventsView
```rust
/// A view of async span events across all streams in a process.
#[derive(Debug)]
pub struct AsyncEventsView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    process_id: sqlx::types::Uuid,
}

impl AsyncEventsView {
    pub fn new(view_instance_id: &str) -> Result<Self> {
        // Parse process_id (no global view support)
        let process_id = Uuid::parse_str(view_instance_id)
            .with_context(|| "Uuid::parse_str")?;
        
        Ok(Self {
            view_set_name: Arc::new(String::from("async_events")),
            view_instance_id: Arc::new(view_instance_id.into()),
            process_id,
        })
    }
}
```

#### 3. AsyncEventBlockProcessor
```rust
/// Processes async span events from thread event blocks.
pub struct AsyncEventBlockProcessor {
    record_builder: AsyncEventRecordBuilder,
}

impl AsyncBlockProcessor for AsyncEventBlockProcessor {
    fn on_begin_async_scope(&mut self, block_id: &str, event_id: i64, scope: ScopeDesc, ts: i64, parent_span_id: i64) -> Result<bool>;
    fn on_end_async_scope(&mut self, block_id: &str, event_id: i64, scope: ScopeDesc, ts: i64, span_id: i64) -> Result<bool>;
}
```

#### 4. AsyncEventRecordBuilder
```rust
/// A builder for creating a `RecordBatch` of async events.
pub struct AsyncEventRecordBuilder {
    pub span_ids: PrimitiveBuilder<Int64Type>,
    pub parent_span_ids: PrimitiveBuilder<Int64Type>,
    pub event_types: StringDictionaryBuilder<Int16Type>, // "begin" | "end"
    pub timestamps: PrimitiveBuilder<TimestampNanosecondType>,
    pub thread_ids: StringDictionaryBuilder<Int16Type>, // stream_id as string
    pub names: StringDictionaryBuilder<Int16Type>,
    pub targets: StringDictionaryBuilder<Int16Type>,
    pub filenames: StringDictionaryBuilder<Int16Type>,
    pub lines: PrimitiveBuilder<UInt32Type>,
}
```

#### 5. Schema Function
```rust
/// Returns the schema for the async_events table.
pub fn get_async_events_schema() -> Schema {
    Schema::new(vec![
        Field::new("span_id", DataType::Int64, false),
        Field::new("parent_span_id", DataType::Int64, true), // nullable for root spans
        Field::new("event_type", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("time", DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())), false),
        Field::new("thread_id", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("name", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("target", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("filename", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("line", DataType::UInt32, false),
    ])
}
```

### Integration with Existing Code

#### ViewFactory Registration
Add to `default_view_factory()` in `view_factory.rs`:
```rust
let async_events_view_maker = Arc::new(AsyncEventsViewMaker {});
factory.add_view_set(String::from("async_events"), async_events_view_maker);
```

#### New Async Block Parser Function
Create a new function `parse_async_block_payload()` for async events:
- Handle `BeginAsyncSpanEvent`, `EndAsyncSpanEvent`, `BeginAsyncNamedSpanEvent`, `EndAsyncNamedSpanEvent`
- Keep `parse_thread_block_payload()` focused on sync thread events only
- Extract async span data (span_id, parent_span_id, thread context)
- Use similar pattern but with async-specific processor trait

```rust
/// Parses async span events from a thread event block payload.
#[span_fn]
pub fn parse_async_block_payload<Proc: AsyncBlockProcessor>(
    block_id: &str,
    object_offset: i64,
    payload: &micromegas_telemetry::block_wire_format::BlockPayload,
    stream: &micromegas_telemetry::stream_info::StreamInfo,
    processor: &mut Proc,
) -> Result<bool> {
    // Process only async span events, ignore sync thread events
}

/// Trait for processing async event blocks.
pub trait AsyncBlockProcessor {
    fn on_begin_async_scope(&mut self, block_id: &str, event_id: i64, scope: ScopeDesc, ts: i64, parent_span_id: i64) -> Result<bool>;
    fn on_end_async_scope(&mut self, block_id: &str, event_id: i64, scope: ScopeDesc, ts: i64, span_id: i64) -> Result<bool>;
}
```

### Data Flow

1. **Event Collection**: Async span events from all thread streams in a process
2. **Processing**: Use `list_process_streams_tagged()` to find all relevant streams
3. **Aggregation**: Collect events across streams using JIT partitions approach
4. **Schema**: Raw async events with thread context, not aggregated spans like ThreadSpansView

### Key Differences from ThreadSpansView

- **Scope**: Process-wide instead of single stream
- **Data**: Raw async events instead of constructed call tree spans  
- **Thread Context**: Include thread_id/stream_id to show cross-thread async flow
- **Event Types**: Separate begin/end events to show async lifecycle

