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

4. **All Tests Now Passing**: Full CI pipeline passes with formatting, clippy, and all unit/integration tests successful### Phase 5: FlightSQL Streaming Table Function

**Status**: 🔄 **PENDING** - Not yet implemented

**Objective**: Implement FlightSQL chunked binary streaming infrastructure

**Current Limitations**:
- No server-side Perfetto trace generation capability
- All trace generation happens client-side in `perfetto_trace_client.rs`
- No SQL interface for generating traces with different span types

**Tasks**:
1. **Implement `perfetto_trace_chunks` table function**:
   - Add `PerfettoTraceTableFunction` implementing `TableFunctionImpl`
   - Register in `default_view_factory()` alongside existing `view_instance` function
   - SQL interface: `SELECT chunk_id, chunk_data FROM perfetto_trace_chunks('process_id', 'begin_time', 'end_time', 'both') ORDER BY chunk_id`

2. **Streaming execution plan**:
   - `PerfettoTraceProvider` implementing `TableProvider`
   - `PerfettoTraceExecutionPlan` implementing `ExecutionPlan`
   - Stream Perfetto trace data as it's generated, not after completion
   - Use streaming Writer from Phase 2 for incremental chunk emission
   - Yield record batches with schema: `chunk_id: Int32, chunk_data: Binary` as data becomes available

3. **Unit tests for table function**:
   - Mock FlightSQL client with known process data
   - Test chunk generation and ordering
   - Test binary reconstruction from chunks
   - Test error handling for invalid parameters

### Phase 6: Server-Side Perfetto Generation

**Status**: 🔄 **PENDING** - Not yet implemented

**Objective**: Move trace generation logic from client to server via SQL table function

**Current Approach**: All trace generation happens in client-side `perfetto_trace_client.rs`

**Tasks**:
1. **Implement server-side trace generation** in `PerfettoTraceExecutionPlan`:
   - Integrate Phase 2 streaming Writer with Phase 3 async event support
   - Query `view_instance('async_events', '{process_id}')` for async span events
   - Query thread spans and process metadata within FlightSQL server context
   - Generate complete Perfetto trace server-side using streaming emission

2. **Handle async events in server generation**:
   - Match "begin" and "end" events by `span_id`
   - Create async tracks per span using Phase 2 methods
   - Parent async tracks to appropriate thread tracks via `stream_id`
   - Handle orphaned events (begin without end, end without begin)

3. **Streaming chunk emission**:
   - Stream process descriptors, thread descriptors, then span events as data becomes available
   - Each query result batch triggers chunk emission via Phase 2 streaming Writer
   - Natural backpressure from DataFusion prevents memory bloat

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
