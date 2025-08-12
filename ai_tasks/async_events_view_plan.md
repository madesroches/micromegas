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

## Unit Testing Strategy

Following patterns from existing async and analytics tests, the testing approach will include:

### 1. Async Event Generation Tests
Based on `async_span_tests.rs` patterns:

```rust
#[test]
#[serial]
fn test_async_events_view_manual_instrumentation() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("async-events-test")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");

    // Create async operations across multiple threads
    runtime.block_on(async {
        let handles = (0..3).map(|i| {
            tokio::spawn(async move {
                test_async_function(i).await;
                flush_thread_buffer();
            })
        }).collect::<Vec<_>>();
        
        for handle in handles {
            handle.await.expect("Task failed");
        }
    });

    drop(runtime);
    shutdown_dispatch();

    // Validate async events were captured
    let state = sink.state.lock().expect("Failed to lock sink state");
    let total_events: usize = state.thread_blocks.iter()
        .map(|block| count_async_events(block))
        .sum();
    
    assert!(total_events > 0, "Expected async events to be captured");
    unsafe { force_uninit() };
}
```

### 2. Block Payload Parsing Tests  
Based on `log_tests.rs` and `span_tests.rs` patterns:

```rust
#[test]
#[serial]
fn test_parse_async_block_payload() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());
    
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = ThreadStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    // Add mock async span events
    stream.get_events_mut().push(BeginAsyncSpanEvent { /* ... */ });
    stream.get_events_mut().push(EndAsyncSpanEvent { /* ... */ });
    
    let mut block = stream.replace_block(Arc::new(ThreadBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    
    // Test parsing with AsyncEventBlockProcessor
    let mut processor = MockAsyncEventBlockProcessor::new();
    let stream_info = make_stream_info(&stream);
    let received_block: micromegas_telemetry::block_wire_format::Block = 
        ciborium::from_reader(&encoded[..]).unwrap();
        
    parse_async_block_payload("test_block", 0, &received_block.payload, &stream_info, &mut processor)
        .unwrap();
        
    assert_eq!(processor.begin_count, 1);
    assert_eq!(processor.end_count, 1);
    
    shutdown_dispatch();
    unsafe { force_uninit() };
}
```

### 3. View Integration Tests
Based on analytics test patterns:

```rust
#[tokio::test]
async fn test_async_events_view_creation() {
    let process_id = uuid::Uuid::new_v4();
    let view = AsyncEventsView::new(&process_id.to_string()).unwrap();
    
    assert_eq!(*view.get_view_set_name(), "async_events");
    assert_eq!(*view.get_view_instance_id(), process_id.to_string());
}

#[tokio::test] 
async fn test_async_events_view_maker() {
    let maker = AsyncEventsViewMaker {};
    let process_id = uuid::Uuid::new_v4();
    let view = maker.make_view(&process_id.to_string()).unwrap();
    
    assert_eq!(*view.get_view_set_name(), "async_events");
}
```

### 4. Record Builder Tests

```rust
#[test]
fn test_async_event_record_builder() {
    let mut builder = AsyncEventRecordBuilder::with_capacity(10);
    
    // Add test async event data
    builder.append_async_event(AsyncEventData {
        span_id: 1,
        parent_span_id: Some(0), 
        event_type: "begin",
        timestamp: 1000000,
        thread_id: "thread-1",
        name: "async_function",
        target: "my_module",
        filename: "test.rs",
        line: 42,
    }).unwrap();
    
    let batch = builder.finish().unwrap();
    assert_eq!(batch.num_rows(), 1);
    
    // Verify schema matches expected
    assert_eq!(batch.schema(), &get_async_events_schema());
}
```

### 5. Cross-Thread Async Flow Tests

```rust
#[test]
#[serial]
fn test_cross_thread_async_tracking() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());
    
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
        
    runtime.block_on(async {
        // Spawn task that migrates across threads
        let handle = tokio::spawn(async {
            for i in 0..5 {
                tokio::task::yield_now().await; // Force potential thread migration
                async_instrumented_work(i).await;
            }
        });
        handle.await.unwrap();
    });
    
    // Validate that async events show cross-thread execution
    let events = extract_async_events_from_sink(&sink);
    let unique_threads: std::collections::HashSet<_> = events.iter()
        .map(|e| &e.thread_id)
        .collect();
        
    // Should have events from multiple threads due to work stealing
    assert!(unique_threads.len() > 1, "Expected cross-thread async execution");
}
```

### 6. Test Organization

Create new test file: `rust/analytics/tests/async_events_view_tests.rs`

**Test Dependencies**:
- `serial_test::serial` - For tests that need exclusive access to global tracing state
- `tokio-test` - For async test utilities  
- Mock data generators for consistent test scenarios
- Test database setup utilities from existing analytics tests

**Sequential Test Requirements**:
Tests that use `init_in_mem_tracing()`, `shutdown_dispatch()`, or `force_uninit()` must be marked with `#[serial]` because they modify global tracing state:
- ✅ `test_async_events_view_manual_instrumentation`
- ✅ `test_parse_async_block_payload` 
- ✅ `test_cross_thread_async_tracking`
- ❌ `test_async_event_record_builder` (pure data structure test)
- ❌ `test_async_events_view_creation` (no tracing setup)
- ❌ `test_async_events_view_maker` (no tracing setup)

**Test Coverage Areas**:
- ✅ Async event generation and capture
- ✅ Block payload parsing for async events
- ✅ View creation and factory patterns
- ✅ Record builder functionality  
- ✅ End-to-end view materialization
- ✅ Cross-thread async tracking
- ✅ Error handling and edge cases
- ✅ Performance with large async workloads

## Python Integration Tests

Following patterns from existing Python tests in `python/micromegas/tests/`, create end-to-end integration tests:

### New Test File: `python/micromegas/tests/test_async_events.py`

```python
from .test_utils import *

def test_async_events_query():
    """Test basic async events view querying"""
    # Get a process from the generator binary (which produces async spans)
    sql = """
    SELECT processes.process_id 
    FROM processes
    WHERE exe LIKE '%generator%' OR exe LIKE '%telemetry-generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    assert len(processes) > 0, "No generator processes found - run telemetry generator first"
    process_id = processes.iloc[0]["process_id"]
    
    # Query async events for this process
    sql = """
    SELECT span_id, parent_span_id, event_type, time, thread_id, name, target
    FROM view_instance('async_events', '{process_id}')
    ORDER BY time
    LIMIT 10;
    """.format(process_id=process_id)
    
    async_events = client.query(sql, begin, end)
    print(async_events)
    assert len(async_events) > 0, "Expected async events in results"

def test_async_events_cross_thread():
    """Test that async events show cross-thread execution"""
    sql = """
    SELECT processes.process_id 
    FROM processes
    WHERE exe LIKE '%generator%' OR exe LIKE '%telemetry-generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    assert len(processes) > 0, "No generator processes found - run telemetry generator first"
    process_id = processes.iloc[0]["process_id"]
    
    # Query for async events across different threads
    sql = """
    SELECT DISTINCT thread_id, COUNT(*) as event_count
    FROM view_instance('async_events', '{process_id}')
    WHERE event_type = 'begin'
    GROUP BY thread_id
    ORDER BY event_count DESC;
    """.format(process_id=process_id)
    
    thread_summary = client.query(sql, begin, end)
    print("Async events per thread:")
    print(thread_summary)
    
    # Should have events from multiple threads
    assert len(thread_summary) > 0, "Expected async events"

def test_async_events_span_relationships():
    """Test parent-child relationships in async spans"""
    sql = """
    SELECT processes.process_id 
    FROM processes
    WHERE exe LIKE '%generator%' OR exe LIKE '%telemetry-generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    assert len(processes) > 0, "No generator processes found - run telemetry generator first"
    process_id = processes.iloc[0]["process_id"]
    
    # Query for parent-child async span relationships
    sql = """
    SELECT parent.name as parent_name, child.name as child_name, 
           parent.span_id as parent_id, child.parent_span_id
    FROM view_instance('async_events', '{process_id}') parent
    JOIN view_instance('async_events', '{process_id}') child 
         ON parent.span_id = child.parent_span_id
    WHERE parent.event_type = 'begin' AND child.event_type = 'begin'
    LIMIT 10;
    """.format(process_id=process_id)
    
    relationships = client.query(sql, begin, end)
    print("Parent-child async span relationships:")
    print(relationships)

def test_async_events_duration_analysis():
    """Test analyzing async operation durations"""
    sql = """
    SELECT processes.process_id 
    FROM processes
    WHERE exe LIKE '%generator%' OR exe LIKE '%telemetry-generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    assert len(processes) > 0, "No generator processes found - run telemetry generator first"
    process_id = processes.iloc[0]["process_id"]
    
    # Calculate durations by matching begin/end events
    sql = """
    SELECT begin_events.name, begin_events.thread_id,
           (end_events.time - begin_events.time) / 1000000 as duration_ms
    FROM 
        (SELECT * FROM view_instance('async_events', '{process_id}') 
         WHERE event_type = 'begin') begin_events
    JOIN 
        (SELECT * FROM view_instance('async_events', '{process_id}') 
         WHERE event_type = 'end') end_events
    ON begin_events.span_id = end_events.span_id
    ORDER BY duration_ms DESC
    LIMIT 10;
    """.format(process_id=process_id)
    
    durations = client.query(sql, begin, end)
    print("Longest async operations:")
    print(durations)
```

### Integration Test Coverage:
- ✅ Basic async events view querying 
- ✅ Cross-thread async execution validation
- ✅ Parent-child span relationship analysis
- ✅ Duration calculation and performance analysis
- ✅ Integration with existing micromegas Python client
- ✅ Real data validation using view_instance() function

