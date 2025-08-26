# Perfetto Async Spans Generation Plan

## Overview

Generate Perfetto trace files from a process's async span events by extending the existing Perfetto trace generation functionality to include async span events alongside the current thread-based spans.

## Current State Analysis (Updated 2025-08-21)

### ✅ PHASE 1 COMPLETED: Analytics Web App Development Tool
- **Analytics Web App**: Fully implemented with Next.js frontend and Rust Axum backend
  - Modern React UI with Tailwind CSS and Radix UI components
  - Process discovery and filtering (`/analyticsweb/processes`)
  - Real-time Perfetto trace generation with HTTP streaming (`/analyticsweb/perfetto/{process_id}/generate`)
  - Process statistics and log entries endpoints
  - Health monitoring and CORS configuration
  - Production-ready deployment capability
- **Testing Foundation**: Platform for validating trace generation is operational

### Existing Perfetto Infrastructure
- **Rust Perfetto crate** (`rust/perfetto/`): Complete protobuf-based Perfetto trace writer
- **Writer API**: Supports process descriptors, thread descriptors, and span events
- **Client integration**: `perfetto_trace_client.rs` generates traces from thread spans using `view_instance('thread_spans', '{stream_id}')`

### ✅ Existing Async Events Infrastructure (Completed)
- **AsyncEventRecord structure**: Contains `stream_id`, `block_id`, `time`, `event_type`, `span_id`, `parent_span_id`, `depth`, `name`, `filename`, `target`, `line`
- **Async Events View**: `AsyncEventsView` provides materialized view access to async span events via `view_instance('async_events', '{process_id}')`
- **Event Types**: "begin" and "end" events mark async span boundaries
- **Depth tracking**: Async spans include depth information for hierarchical visualization

## Implementation Plan

### ✅ Phase 1: Analytics Web App Development Tool with Current Client (COMPLETED)

**Status**: ✅ **COMPLETED** - Fully operational analytics web application

**Implemented Features**:
- **Analytics Web Server** (`rust/analytics-web-srv/`): Axum-based REST API server
  - Health check endpoints with FlightSQL connectivity status
  - Process listing and metadata retrieval
  - Real-time Perfetto trace generation with HTTP streaming progress updates
  - Process statistics and log entries retrieval
  - Environment variable-based CORS configuration
- **Frontend Application** (`analytics-web-app/`): Next.js 15 + React 18 + TypeScript
  - Modern UI with Tailwind CSS and Radix UI components
  - Process discovery table with search and filtering
  - Real-time trace generation with progress visualization
  - Responsive design and error handling
- **Development Tooling**: Fully configured development and production environments
  - Hot reloading for frontend development
  - Integrated backend/frontend development workflow
  - Production build pipeline and static file serving

**Testing Capability**:
- ✅ Platform ready for testing async span implementation phases
- ✅ Real-time trace generation and validation interface available
- ✅ HTTP streaming infrastructure operational for progress reporting

### ✅ Phase 2: Perfetto Writer Streaming Support (COMPLETED)

**Status**: ✅ **COMPLETED** - Full streaming writer implementation

**Objective**: Make Perfetto Writer capable of streaming generation (foundation for SQL approach)

**Implemented Features**:
- **StreamingPerfettoWriter<W: Write>**: Streams TracePackets directly to output without memory accumulation
- **Proper protobuf framing**: Uses prost's `encode_key()` and `encode_varint()` for reliable encoding
- **Two-buffer approach**: Separate buffers for packet data and protobuf framing
- **Interning state management**: Maintains string interning (names, categories, source_locations) across streaming
- **Identical API**: `emit_process_descriptor()`, `emit_thread_descriptor()`, `emit_span()` methods match regular Writer
- **Full binary compatibility**: Produces identical output to regular Writer (verified: 470 bytes, 9 packets)

**Implementation Details**:
- **streaming_writer.rs**: Dedicated module for streaming functionality
- **Constant memory usage**: Memory usage independent of trace size (tested with 1000 spans)
- **Error handling**: Proper error propagation for Write failures
- **Clean separation**: No changes to existing Writer implementation

**Testing**:
- **6 comprehensive tests**: Basic usage, compatibility, packet framing, interning, memory usage, error handling
- **External test file**: `tests/streaming_writer_tests.rs` following project conventions
- **Compatibility example**: `streaming_comparison.rs` demonstrates identical output
- **All tests passing**: Both regular and streaming writers produce identical results

**Code Organization**:
- StreamingPerfettoWriter in dedicated `src/streaming_writer.rs` module
- Updated `lib.rs` exports: separate imports for `Writer` and `StreamingPerfettoWriter`
- Well-documented protobuf field number constant with schema reference

### ✅ Phase 3: Async Event Support in Perfetto Writer (COMPLETED)

**Status**: ✅ **COMPLETED** - Full async span support implemented with timestamp reliability issues resolved

**Objective**: Add async track support to the Perfetto writer (independent of streaming)

**✅ Implemented Features**:
- **Single async track approach**: All async spans appear on unified "Async Operations" track
- **Regular Writer support** (`rust/perfetto/src/writer.rs`):
  - `append_async_track_descriptor()` - creates single async track parented to process
  - `append_async_span_begin()` / `append_async_span_end()` - emit async span events
  - Safety assertions prevent misuse (track must exist before span events)
- **Streaming Writer support** (`rust/perfetto/src/streaming_writer.rs`):
  - `emit_async_track_descriptor()` - streaming async track creation
  - `emit_async_span_begin()` / `emit_async_span_end()` - streaming async span events
  - API parity with regular writer for consistent behavior

**✅ Testing & Validation**:
- **Comprehensive unit tests** (13 tests total, all passing):
  - Async track creation for both regular and streaming writers
  - Async span event generation with proper track UUID assignment
  - Error handling for missing track descriptors (panic tests)
  - Idempotent track creation verification
  - Compatibility with existing streaming writer functionality
- **End-to-end testing** via trace generation utility:
  - Real telemetry data from `telemetry-generator` (process: `34d7c06d-3163-4111-863e-c5fc09d22d51`)
  - Generated valid 8,356-byte Perfetto trace with 166 packets
  - 1 process descriptor, 10 thread descriptors, 1 async track, 154 track events
  - Trace validated and ready for Perfetto UI visualization

**✅ Implementation Details**:
- **Track hierarchy**: Process → Thread tracks + Single async track
- **UUID generation**: `xxh64("async_track".as_bytes(), process_uuid)` for consistent async track IDs
- **Event processing**: Handles both "begin" and "end" async events with proper timestamps
- **Memory efficiency**: Streaming writer maintains constant memory usage
- **API consistency**: Both writers use identical method signatures and behavior

**✅ Key Design Decisions**:
- **Single async track per process** instead of per-span tracks for better visualization
- **Process-level parenting** for async track (not thread-level) for cleaner hierarchy
- **Safety-first design** with assertions ensuring proper usage patterns
- **Full streaming support** for memory-efficient large trace generation

**✅ Resolved Issues**:
- **✅ Timestamp reliability fixed**: Replaced `NullPartitionProvider` with `LivePartitionProvider` in `find_process_with_latest_timing`
- **✅ Stream filtering implemented**: Trace generation now only processes CPU streams using `array_has(streams.tags, 'cpu')`
- **✅ DataFusion Query Issue resolved**: AsyncEventsView now includes proper view_factory with processes view access
- **✅ UUID parsing fixed**: Added `parse_optional_uuid()` function to handle empty UUID strings gracefully
- **✅ Schema compatibility**: Fixed `tsc_frequency` casting from UInt64 to Int64 in DataFusion queries
- **✅ Test compilation**: Updated async_events_tests.rs to work with new AsyncEventsView constructor
- **✅ Query range optimization**: Pass query_range to limit partition search and reduce database load

### ✅ Phase 4: CPU Tracing Control (COMPLETED)

**Status**: ✅ **COMPLETED** - Full CPU tracing control implementation

**Objective**: Add configurable CPU tracing control with environment variable

**✅ Implemented Features**:
- **Environment variable control**: `MICROMEGAS_ENABLE_CPU_TRACING` environment variable (defaults to `false`)
- **Process-lifetime constant**: Environment variable is read once at startup and never changes
- **Zero overhead when disabled**: No tokio runtime callbacks registered if CPU tracing is disabled
- **Conditional thread stream initialization**: Thread streams are not created when CPU tracing is disabled
- **Minimal overhead by default**: Default value is disabled (false) for optimal production performance

**✅ Implementation Complete**:

#### ✅ 4.1 Environment Variable Infrastructure
- **✅ Dispatch struct updated**: Added `cpu_tracing_enabled: bool` field to `struct Dispatch`
- **✅ Constructor updated**: Added `cpu_tracing_enabled: bool` parameter to `Dispatch::new()` constructor
- **✅ Function signature updated**: `init_event_dispatch()` now requires `cpu_tracing_enabled: bool` parameter
- **✅ Sink libraries updated**: All sink libraries read `MICROMEGAS_ENABLE_CPU_TRACING` environment variable
- **✅ Service integration**: Services pass CPU tracing setting when creating dispatch

#### ✅ 4.2 Thread Stream Conditional Initialization
- **✅ Conditional initialization**: `init_thread_stream()` checks CPU tracing flag before proceeding
- **✅ Early return**: Function returns early if CPU tracing is disabled
- **✅ No allocation when disabled**: ThreadStream not allocated when CPU tracing is disabled
- **✅ Stream tags conditional**: "cpu" tag not added to stream tags when disabled

#### ✅ 4.3 Tokio Runtime Integration
- **✅ Runtime extension updated**: `TracingRuntimeExt::with_tracing_callbacks()` reads environment variable
- **✅ Conditional callback registration**: No callbacks registered when CPU tracing disabled
- **✅ Default to disabled**: Defaults to `false` for minimal overhead
- **✅ Both variants implemented**: Both `with_tracing_callbacks()` and `with_tracing_callbacks_and_custom_start()` support conditional behavior

#### ✅ 4.4 Event Recording Path
- **✅ Natural no-ops**: Event recording functions become no-ops when thread streams are uninitialized
- **✅ Graceful handling**: All span functions handle missing thread streams gracefully
- **✅ Zero overhead**: No additional runtime checks needed in hot paths

#### ✅ 4.5 Test Infrastructure Updates
- **✅ Test utilities updated**: `InMemoryTracingGuard` enables CPU tracing for tests by default
- **✅ Environment variable support**: Tests can override CPU tracing with `MICROMEGAS_ENABLE_CPU_TRACING=true`
- **✅ Proper test isolation**: Tests use proper test utilities with automatic cleanup

#### ✅ 4.6 Service and Development Configuration
- **✅ Development environment**: Tests explicitly enable CPU tracing via environment variable
- **✅ Production ready**: Default disabled behavior suitable for production deployments
- **✅ Configurable services**: Services can enable/disable CPU tracing via environment variable

**✅ Testing and Validation**:
- **✅ Unit tests**: All existing tests updated to work with CPU tracing control
- **✅ Integration tests**: Tests verify conditional behavior works correctly
- **✅ Async span compatibility**: Async spans work regardless of CPU tracing setting
- **✅ Zero regression**: All existing functionality preserved when CPU tracing is enabled

**✅ Key Implementation Details**:
```rust
// Runtime extension conditionally registers callbacks
impl TracingRuntimeExt for tokio::runtime::Builder {
    fn with_tracing_callbacks(&mut self) -> &mut Self {
        let cpu_tracing_enabled = std::env::var("MICROMEGAS_ENABLE_CPU_TRACING")
            .map(|v| v == "true")
            .unwrap_or(false); // Default to disabled

        if !cpu_tracing_enabled {
            return self; // No callbacks when disabled
        }

        self.on_thread_start(|| init_thread_stream())
            .on_thread_stop(|| unregister_thread_stream())
    }
}

// Tests enable CPU tracing explicitly
#[test]
#[serial]
fn test_async_spans() {
    unsafe { std::env::set_var("MICROMEGAS_ENABLE_CPU_TRACING", "true"); }
    let guard = init_in_memory_tracing();
    // ... test logic
}
```

**✅ Expected Outcomes Achieved**:
- **✅ Zero runtime overhead** when CPU tracing is disabled
- **✅ No thread stream allocation** when CPU tracing is disabled
- **✅ No tokio callback registration** when CPU tracing is disabled
- **✅ Configurable via environment variable** at process startup
- **✅ Minimal overhead by default** for optimal production performance
- **✅ All tests working** with proper CPU tracing configuration

**✅ Test Infrastructure Updates (August 26, 2025)**:
During the completion of Phase 4, several test files required updates to work with the new CPU tracing control:

1. **Fixed Test Files**:
   - `rust/analytics/tests/async_span_tests.rs` - All 4 async span tests now passing
   - `rust/analytics/tests/async_trait_tracing_test.rs` - All 2 async trait tests now passing
   - `rust/tracing/tests/thread_park_test.rs` - Thread park test now passing

2. **Root Cause Identified**: Tests were failing because CPU tracing was disabled by default, causing tokio runtime callbacks to not be registered and async spans to not be recorded (0 events instead of expected counts).

3. **Solution Applied**:
   - Added `unsafe { std::env::set_var("MICROMEGAS_ENABLE_CPU_TRACING", "true"); }` to enable CPU tracing in tests
   - Updated imports to use proper test utilities (`micromegas_tracing::test_utils::init_in_memory_tracing`)
   - Added missing `TracingBlock` trait import for `nb_objects()` method
   - Replaced deprecated manual initialization with `InMemoryTracingGuard` for automatic cleanup
   - Updated function signatures to match new `init_event_dispatch(cpu_tracing_enabled: bool)` parameter

4. **All Tests Now Passing**: Full CI pipeline passes with formatting, clippy, and all unit/integration tests successful

### Phase 5: FlightSQL Streaming Table Function

**Status**: 🔄 **IN PROGRESS**

**Objective**: Implement FlightSQL chunked binary streaming infrastructure for server-side Perfetto trace generation

**Current Limitations**:
- No server-side Perfetto trace generation capability
- All trace generation happens client-side in `perfetto_trace_client.rs`
- No SQL interface for generating traces with different span types
- Client must fetch all data and generate traces locally

**Implementation Design**:

1. **Table Function: `perfetto_trace_chunks`**
   - **Location**: `rust/analytics/src/lakehouse/perfetto_trace_table_function.rs`
   - **SQL Interface**: 
     ```sql
     SELECT chunk_id, chunk_data 
     FROM perfetto_trace_chunks(
       'process_id',                              -- Process UUID (required)
       'span_types',                              -- 'thread', 'async', or 'both' (required)
       TIMESTAMP '2024-01-01T00:00:00Z',          -- Start time as UTC timestamp (required)
       TIMESTAMP '2024-01-01T01:00:00Z'           -- End time as UTC timestamp (required)
     ) ORDER BY chunk_id
     ```
   - **Implementation**: `PerfettoTraceTableFunction` implementing `TableFunctionImpl`
   - **Registration**: Add to `register_table_functions()` in `query.rs`
   - **Dependencies**: Needs access to ViewFactory, DataLakeConnection, ObjectStore
   - **All arguments mandatory**: Simplifies implementation, defaults can be set by callers

2. **Custom Execution Plan**:
   - **Location**: `rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs`
   - **Key Components**:
     ```rust
     pub struct PerfettoTraceExecutionPlan {
         schema: SchemaRef,           // chunk_id: Int32, chunk_data: Binary
         process_id: String,
         span_types: SpanTypes,       // enum: Thread, Async, Both
         time_range: TimeRange,       // Required time range
         view_factory: Arc<ViewFactory>,
         // ... other dependencies
     }
     ```
   - **Execution Flow**:
     1. Query process metadata from `processes` table filtered by process_id
     2. Query thread metadata from `streams` table filtered by process_id
     3. Query thread spans if needed via `view_instance('thread_spans', stream_id)` for each stream
     4. Query async events if needed via `view_instance('async_events', process_id)`
     5. Stream chunks as data becomes available using `SendableRecordBatchStream`
   - **Streaming Strategy**:
     - Stream each TracePacket as a separate chunk immediately when ready
     - No buffering or aggregation - simpler implementation
     - Each row in output = one TracePacket (natural boundaries)

3. **Perfetto Writer Integration**:
   - **Writer Setup**:
     ```rust
     let (packet_sender, packet_receiver) = mpsc::channel(16);
     let packet_writer = PacketWriter::new(packet_sender);
     let mut writer = StreamingPerfettoWriter::new(packet_writer);
     ```
   - **Packet-Based Chunk Generation**:
     - Process descriptor → Packet 0 → Chunk 0
     - Each thread descriptor → Own packet → Own chunk
     - Async track descriptor → Own packet → Own chunk  
     - Each span event → Own packet → Own chunk
     - Natural 1:1 mapping between TracePackets and output rows
   - **Memory Management**: 
     - Never accumulate full trace in memory
     - Stream directly from DataFusion query results to output
     - Each TracePacket immediately becomes a chunk (no buffering)

4. **Testing Strategy**:

   **Why Integration Tests Over Unit Tests**:
   - Unit testing challenges:
     - Table functions require full DataFusion SessionContext
     - Need to mock ViewFactory, DataLakeConnection, ObjectStore
     - Complex mock setup for view_instance queries
     - Limited value in testing with mocked data
   
   **Integration Test Approach**:
   - **Location**: `python/micromegas/tests/test_perfetto_trace_chunks.py`
   - **Test Infrastructure**:
     ```python
     from .test_utils import *
     import micromegas
     
     def test_perfetto_trace_chunks_basic():
         """Test basic perfetto trace chunk generation"""
         # 1. Find recent telemetry-generator process
         sql = """
         SELECT process_id, start_time
         FROM processes  
         WHERE exe LIKE '%generator%'
         ORDER BY start_time DESC
         LIMIT 1;
         """
         processes = client.query(sql)
         
         # 2. Query chunks using new table function (all args mandatory)
         sql = """
         SELECT chunk_id, chunk_data
         FROM perfetto_trace_chunks(
             '{process_id}', 
             'both',
             TIMESTAMP '{begin_ts}',
             TIMESTAMP '{end_ts}'
         )
         ORDER BY chunk_id;
         """.format(
             process_id=process_id,
             begin_ts=process_begin.isoformat(),
             end_ts=process_end.isoformat()
         )
         chunks = client.query(sql)
         
         # 3. Reassemble chunks into complete trace
         trace_bytes = b''.join(chunks['chunk_data'])
         
         # 4. Validate trace structure
         assert len(trace_bytes) > 0
         # Could also write to file and validate with protobuf library
     ```
   - **Test Cases**:
     1. Thread-only trace generation (`'thread'` parameter)
     2. Async-only trace generation (`'async'` parameter)
     3. Combined thread + async traces (`'both'` parameter)
     4. Verify packet streaming (each chunk is valid protobuf)
     5. Time range filtering with optional parameters
   
   **End-to-End Validation**:
   - Compare with existing `perfetto_trace_client` output for thread-only traces
   - Write trace to file and manually validate in Perfetto UI
   - Verify packet-based streaming (each chunk is a complete TracePacket)

5. **Implementation Order**:
   1. Create basic table function that returns empty chunks
   2. Implement custom execution plan with schema
   3. Add process metadata query and packet generation
   4. Add thread span querying and streaming (one packet per span)
   5. Add async event querying and streaming (one packet per event pair)
   6. Integration testing with real data

**Key Design Decisions**:
- **Packet-Based Chunking**: Each TracePacket becomes its own chunk for simplicity
- **No Buffering**: Packets stream immediately without aggregation
- **No Caching**: Each query generates fresh trace (stateless)
- **Streaming First**: Never accumulate full trace in memory
- **SQL-Native**: Leverages DataFusion's query optimization
- **Reusable**: Can be called from any FlightSQL client

### Phase 6: Server-Side Perfetto Generation

**Status**: 🔄 **PENDING** - Depends on Phase 5

**Objective**: Implement the actual trace generation logic within the execution plan from Phase 5

**Note**: Phase 5 creates the infrastructure (table function, execution plan, chunking), while Phase 6 implements the actual Perfetto generation logic inside that infrastructure.

**Implementation Details**:

1. **PacketWriter Implementation**:
   ```rust
   pub struct PacketWriter {
       sender: mpsc::Sender<RecordBatch>,
       chunk_id: i32,
   }
   
   impl PacketWriter {
       // Called by StreamingPerfettoWriter after each packet
       pub fn send_packet(&mut self, packet_bytes: Vec<u8>) -> Result<()> {
           let batch = create_chunk_batch(self.chunk_id, packet_bytes);
           self.sender.send(batch).await?;
           self.chunk_id += 1;
           Ok(())
       }
   }
   ```

2. **PerfettoTraceStream Implementation**:
   ```rust
   impl Stream for PerfettoTraceStream {
       type Item = Result<RecordBatch>;
       
       fn poll_next(...) -> Poll<Option<Self::Item>> {
           // 1. Poll queries for new data
           // 2. Feed data to StreamingPerfettoWriter
           // 3. Return chunks as RecordBatches
       }
   }
   ```

3. **Query Integration in ExecutionPlan**:
   - **Process Metadata** (using processes table):
     ```sql
     SELECT process_id, exe, username, computer, tsc_frequency, start_time
     FROM processes
     WHERE process_id = '{process_id}'
     LIMIT 1
     ```
   - **Thread Metadata** (using streams table):
     ```sql
     -- Get thread stream metadata
     SELECT stream_id, 
            property_get(properties, 'thread-name') as thread_name,
            property_get(properties, 'thread-id') as thread_id
     FROM streams
     WHERE process_id = '{process_id}'
       AND array_has(tags, 'cpu')
     ```
   - **Thread Spans** (if span_types includes threads):
     ```sql
     -- For each stream_id from above, get spans
     SELECT id, parent, depth, hash, begin, end, duration, name, target, filename, line
     FROM view_instance('thread_spans', '{stream_id}')
     WHERE begin >= ? AND end <= ?
     ```
   - **Async Events** (if span_types includes async):
     ```sql
     SELECT stream_id, time, event_type, span_id, parent_span_id, 
            depth, name, filename, target, line
     FROM view_instance('async_events', '{process_id}')
     WHERE time >= ? AND time <= ?
     ORDER BY time
     ```

4. **Async Event Processing**:
   ```rust
   struct AsyncSpanTracker {
       pending_spans: HashMap<i64, AsyncSpanInfo>,
       async_track_uuid: u64,  // Single track for all async spans
   }
   
   impl AsyncSpanTracker {
       fn process_event(&mut self, event: AsyncEvent, writer: &mut StreamingPerfettoWriter) {
           match event.event_type.as_str() {
               "begin" => {
                   writer.emit_async_span_begin(...);
                   self.pending_spans.insert(event.span_id, ...);
               }
               "end" => {
                   if let Some(begin_info) = self.pending_spans.remove(&event.span_id) {
                       writer.emit_async_span_end(...);
                   }
               }
           }
       }
   }
   ```

5. **Memory-Efficient Streaming**:
   - Process events in batches from DataFusion queries
   - Never load all events into memory at once
   - Use DataFusion's natural backpressure to control memory usage
   - Stream chunks immediately as they're generated

6. **Error Handling**:
   - Handle incomplete async spans (begin without end)
   - Log warnings for orphaned events
   - Continue processing on non-fatal errors
   - Return partial traces rather than failing completely

### Phase 7: Refactor Client to Use SQL Generation

**Objective**: Convert `perfetto_trace_client.rs` to use `perfetto_trace_chunks` SQL function

**Tasks**:
1. **Simplify client implementation**:
   - Replace `format_perfetto_trace()` logic with SQL query to `perfetto_trace_chunks`
   - Remove duplicate process/thread/span querying from client
   - Remove Perfetto Writer usage from client code

2. **Implement chunk reconstruction**:
   - Query `SELECT chunk_id, chunk_data FROM perfetto_trace_chunks(...) ORDER BY chunk_id`
   - Collect all chunks from FlightSQL stream
   - Reassemble binary data in chunk_id order
   - Return complete trace as Vec<u8>

3. **Maintain API compatibility**:
   - Keep existing `format_perfetto_trace()` and `write_perfetto_trace()` signatures
   - Span type selection via SQL function parameters
   - Preserve error handling and edge case behavior

4. **Validation against baseline**:
   - Use Phase 5 web app to compare before/after trace generation
   - Ensure identical binary output for thread-only traces
   - Verify async spans work correctly in refactored version
   - Performance comparison between approaches

### Phase 8: Data Processing Optimization

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


### Phase 9: Python Client Refactoring

**Objective**: Eliminate duplicate Perfetto generation logic

**Tasks**:
1. **Refactor Python CLI**:
   - Modify `python/micromegas/cli/write_perfetto.py` to use `perfetto_trace_chunks` table function
   - Remove duplicate Perfetto generation logic from `python/micromegas/micromegas/perfetto.py`
   - Implement chunked binary reconstruction in Python client

2. **Ensure feature parity**:
   - Python CLI automatically gets async spans support through Rust implementation
   - Maintain same command-line interface for backward compatibility
   - Add span type selection flags: `--spans=[thread|async|both]` (default: both)

3. **Integration testing**:
   - Verify Python CLI produces identical output to direct Rust calls
   - Test error handling and edge cases
   - Performance comparison between old and new approaches

### Phase 10: Integration Testing and Validation

**Objective**: Ensure generated traces are valid and useful

**Tasks**:
1. **Unit tests** (following `async_events_tests.rs` pattern):
   - Mock FlightSQL client responses with known async events data
   - Test async track creation with various span hierarchies
   - Test event matching ("begin"/"end" pairs)
   - Test handling of incomplete spans
   - Verify Perfetto protobuf structure and interning

2. **Integration tests** (using existing `telemetry-generator`):
   - Use `rust/telemetry-ingestion-srv/test/generator.rs` which already generates async spans
   - Generate real telemetry data and call `format_perfetto_trace()`
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

## Current Implementation Status Summary

### ✅ Completed
- **Phase 1**: Analytics Web App - Fully operational testing and development platform
- **Phase 2**: Perfetto Writer Streaming Support - Complete streaming infrastructure with identical output compatibility
- **Phase 3**: Async Event Support in Perfetto Writer - Complete async span implementation with all reliability issues resolved
- **Phase 4**: CPU Tracing Control - Complete configurable CPU tracing with environment variable control
- **Async Events Infrastructure**: Complete async span data collection and view system
- **Trace Generation Utility**: End-to-end testing tool for validating async span implementation with proper stream filtering
- **Test Infrastructure**: All tests updated and working with CPU tracing control

### ✅ Phase 3 Achievement Highlights
- **Full API Implementation**: Both regular and streaming writers support async spans
- **Production Testing**: Successfully generated 8,356-byte trace with real telemetry data
- **Comprehensive Validation**: 13 unit tests + end-to-end trace validation
- **Performance Verified**: Streaming writer maintains constant memory usage
- **UI Compatible**: Generated traces ready for Perfetto UI visualization
- **Reliability Resolved**: Fixed timestamp issues, stream filtering, and DataFusion query problems

### ✅ Phase 4 Achievement Highlights
- **Environment Variable Control**: `MICROMEGAS_ENABLE_CPU_TRACING` with default disabled for minimal overhead
- **Zero Overhead When Disabled**: No tokio callbacks or thread streams when CPU tracing is disabled
- **Full Test Suite Working**: All async span tests updated and passing with proper CPU tracing configuration
- **Production Ready**: Default disabled behavior suitable for production deployments
- **Development Friendly**: Tests explicitly enable CPU tracing for validation

### 🔄 Pending Implementation (In Priority Order)
1. **Phase 5-7**: Server-Side Generation (Medium Priority)
   - Eliminates code duplication between client implementations
   - Enables advanced features like real-time streaming via FlightSQL
   - Can leverage completed Phase 2 & 3 infrastructure
   - Focus on FlightSQL table function for `perfetto_trace_chunks`

2. **Python Client Refactoring** (Lower Priority)
   - Remove duplicate Perfetto generation logic
   - Use server-side generation via FlightSQL queries
   - Maintain CLI compatibility with async span support

### Next Recommended Steps
1. **✅ Complete**: Phases 1-4 provide complete async span visualization capability with configurable CPU tracing
2. **Medium-term**: Consider implementing server-side generation for code consolidation
3. **Long-term**: Advanced features like real-time trace streaming
4. **Long-term**: Advanced features like real-time trace streaming

## Phase 5-6 Implementation Strategy

### Why Split Phase 5 and 6?

**Phase 5** focuses on the **infrastructure**:
- Table function registration and SQL interface
- ExecutionPlan skeleton with proper schema
- Chunking mechanism and streaming framework
- Basic integration with DataFusion query engine

**Phase 6** focuses on the **trace generation logic**:
- Actual queries to fetch process/thread/async data
- StreamingPerfettoWriter integration
- Event processing and async span matching
- Memory-efficient streaming implementation

This split allows us to:
1. Test the infrastructure independently (Phase 5 can return dummy chunks)
2. Iterate on the chunking mechanism without complex trace logic
3. Validate the SQL interface before implementing generation
4. Parallelize development if needed

### Testing Philosophy

**Integration Tests Over Unit Tests**:
- Table functions are deeply integrated with DataFusion's execution engine
- Mocking the entire context (ViewFactory, DataLakeConnection, ObjectStore) provides minimal value
- Real data validation is more important than mock validation
- Integration tests can verify end-to-end correctness including:
  - SQL parsing and planning
  - Query execution and data fetching
  - Chunk generation and streaming
  - Binary trace validity

**Test Data Strategy**:
- Use `telemetry-generator` which already produces async spans
- Compare output with existing `perfetto_trace_client.rs` for validation
- Ensure backward compatibility for thread-only traces

## Migration Strategy

- **Backward compatible**: Existing thread-only trace generation continues to work
- **Flexible span selection**: Users can choose thread spans only, async spans only, or both
- **Gradual rollout**: Test with small processes before enabling for large-scale traces
- **Python client migration**: Future phases eliminate code duplication by having Python CLI call Rust implementation

## Dependencies

- Existing `rust/perfetto` crate and writer infrastructure
- Existing `AsyncEventsView` and async events data pipeline
- Existing `perfetto_trace_client.rs` query infrastructure
- No new external dependencies required

**Note**: The Python CLI script (`python/micromegas/cli/write_perfetto.py`) completely duplicates functionality that already exists in the Rust Perfetto client (`rust/public/src/client/perfetto_trace_client.rs`). Both query the analytics service and generate Perfetto traces with identical logic. The Python CLI should be refactored to call the existing Rust client instead of maintaining a duplicate implementation. This would eliminate the need to implement async spans in multiple places and ensure consistent behavior.

## Trace Generation Utility

### ✅ Implementation Complete (`rust/trace-gen-util/`)

A standalone command-line utility for generating Perfetto traces directly from the analytics service:

**Key Features**:
- **FlightSQL Integration**: Connects to analytics service and queries trace data
- **Thread + Async Spans**: Generates traces with both thread spans and async operations
- **CLI Interface**: Flexible command-line options for process ID, output file, time ranges
- **Real-time Validation**: Built-in trace validation using Perfetto protobuf library
- **Memory Efficient**: Uses streaming Perfetto writer for large traces

**Usage**:
```bash
cargo run --bin trace-gen -- --process-id "<process-id>" --output "trace.perfetto"
```

**Validation Results** (with `telemetry-generator` data):
- **Process ID**: `34d7c06d-3163-4111-863e-c5fc09d22d51`
- **Generated trace**: 8,356 bytes, 166 packets
- **Track structure**: 1 process, 10 threads, 1 async track, 154 track events
- **Status**: ✅ Valid Perfetto trace ready for UI visualization

This utility demonstrates the end-to-end async span implementation and provides a practical tool for generating Perfetto traces from any process in the analytics system.

## Annex: StreamingPerfettoWriter Implementation Details

### API-Based Protobuf Encoding

Instead of hardcoding protobuf field tags and varint encoding, use prost's internal encoding functions for reliability:

```rust
use std::io::Write;
use prost::{Message, encoding::{encode_key, encode_varint, WireType}};

pub struct StreamingPerfettoWriter<W: Write> {
    writer: W,
    // Interning state (same as regular Writer)
    names: HashMap<String, u64>,
    categories: HashMap<String, u64>,
    source_locations: HashMap<(String, u32), u64>,
}

impl<W: Write> StreamingPerfettoWriter<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            names: HashMap::new(),
            categories: HashMap::new(),
            source_locations: HashMap::new(),
        }
    }

    pub fn write_packet(&mut self, packet: TracePacket) -> anyhow::Result<()> {
        let mut buf = Vec::new();

        // Encode the packet to get its bytes
        packet.encode(&mut buf)?;

        // Write the field key for repeated TracePacket (field 1, wire type 2)
        encode_key(1, WireType::LengthDelimited, &mut self.writer)?;

        // Write the varint length
        encode_varint(buf.len() as u64, &mut self.writer)?;

        // Write the packet data
        self.writer.write_all(&buf)?;

        Ok(())
    }

    // Same pattern for other methods: emit_setup(), emit_span(), etc.
}
```

### Key Benefits of API-Based Approach

1. **No hardcoded constants**: Uses prost's `encode_key()` and `encode_varint()` functions
2. **Forward compatibility**: Protobuf changes handled by prost library updates
3. **Type safety**: `WireType::LengthDelimited` instead of magic numbers
4. **Consistency**: Same encoding logic as `Trace.encode_to_vec()`
5. **Maintainability**: Leverages existing prost dependency

### Dependencies Required

- `prost` crate (already used)
- `prost::encoding` module for low-level encoding functions
- No additional external dependencies needed
