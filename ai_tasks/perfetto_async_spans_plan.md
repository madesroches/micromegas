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

### Phase 4: CPU Tracing Control

**Status**: 🔄 **PENDING** - Not yet implemented

**Objective**: Add configurable CPU tracing control with environment variable

**Current State**: CPU tracing is always enabled, which may not be desired for all use cases

**Design Principles**:
- **Process-lifetime constant**: Environment variable is read once at startup and never changes
- **Zero overhead when disabled**: No tokio runtime callbacks registered if CPU tracing is disabled
- **No thread stream initialization**: Thread streams should not be created when CPU tracing is disabled
- **Minimal overhead by default**: Default value is disabled (false) for optimal production performance

**Implementation Tasks**:

#### 4.1 Environment Variable Infrastructure
1. **Add CPU tracing flag to Dispatch struct** in `rust/tracing/src/dispatch.rs`:
   - Add `cpu_tracing_enabled: bool` field to `struct Dispatch`
   - Add `cpu_tracing_enabled: bool` parameter to `Dispatch::new()` constructor
   - Store the passed value as an instance field for efficient access
   - Do NOT read environment variables in tracing library

2. **Update sink libraries** to read environment variable:
   - **`rust/telemetry-sink/`**: Read `MICROMEGAS_ENABLE_CPU_TRACING` environment variable (default: false)
   - **`rust/telemetry-ingestion-srv/`**: Read environment variable and pass to dispatch (default: false)
   - **`rust/flight-sql-srv/`**: Read environment variable and pass to dispatch (default: false)

3. **Update `init_event_dispatch()` function**:
   - Add `cpu_tracing_enabled: bool` parameter to function signature
   - Pass CPU tracing setting when creating new Dispatch instance
   - Callers (sink libraries) responsible for reading environment variable
   - Log clearly when CPU tracing is enabled/disabled at dispatch creation

#### 4.2 Thread Stream Conditional Initialization
1. **Modify `init_thread_stream()` function** in `rust/tracing/src/dispatch.rs`:
   - Check `G_DISPATCH.get().map_or(false, |d| d.cpu_tracing_enabled)` before proceeding
   - Early return if CPU tracing is disabled
   - Do not allocate ThreadStream when CPU tracing is disabled
   - Do not call sink.on_init_thread_stream() when disabled

2. **Update `Dispatch::init_thread_stream()` method**:
   - Check `self.cpu_tracing_enabled` flag before creating ThreadStream
   - When disabled, do not add "cpu" tag to stream tags array
   - When disabled, do not register thread stream in `self.thread_streams` vector

#### 4.3 Tokio Runtime Integration Changes
**⚠️ Critical Initialization Order Issue**: Tokio callbacks are registered before dispatch exists, but need to know CPU tracing setting.

1. **Add environment variable reading to runtime extension** in `rust/tracing/src/runtime.rs`:
   - Read `MICROMEGAS_ENABLE_CPU_TRACING` directly in `with_tracing_callbacks()`
   - This happens at runtime creation time (before dispatch initialization)
   - Store the setting for callback decision making
   - Default to `false` (disabled by default for minimal overhead)

2. **Conditional callback registration**:
   ```rust
   fn with_tracing_callbacks(&mut self) -> &mut Self {
       let cpu_tracing_enabled = std::env::var("MICROMEGAS_ENABLE_CPU_TRACING")
           .map(|v| v == "true")
           .unwrap_or(false); // Default to disabled for minimal overhead

       if !cpu_tracing_enabled {
           // No callbacks registered when CPU tracing disabled
           return self;
       }

       self.on_thread_start(|| {
           init_thread_stream();
       })
       .on_thread_stop(|| {
           unregister_thread_stream();
       })
   }
   ```

3. **Update proc macro integration** in `rust/micromegas-proc-macros/src/lib.rs`:
   - `#[micromegas_main]` macro must check CPU tracing before enabling runtime callbacks
   - Read environment variable before creating tokio runtime
   - Only call `.with_tracing_callbacks()` if CPU tracing is enabled
   - Ensure consistent behavior between manual runtime setup and proc macro

#### 4.4 Event Recording Path Changes
1. **Update all `on_thread_event<T>()` calls**:
   - **No additional checks needed** - if CPU tracing is disabled, thread streams are never initialized
   - `LOCAL_THREAD_STREAM` remains `None` when CPU tracing is disabled
   - Existing event recording functions naturally become no-ops when stream is `None`
   - Current implementation already handles the case where no thread stream exists

2. **Update thread-local stream access**:
   - `on_begin_scope()`, `on_end_scope()`, `on_begin_named_scope()`, `on_end_named_scope()`
   - **No additional checks needed** - these functions already handle missing thread streams gracefully
   - When CPU tracing is disabled, these become natural no-ops due to uninitialized streams

#### 4.5 Proc Macro Integration Updates
**Critical**: The `#[micromegas_main]` macro must handle CPU tracing initialization order correctly.

1. **Update `#[micromegas_main]` macro** in `rust/micromegas-proc-macros/src/lib.rs`:
   - Read `MICROMEGAS_ENABLE_CPU_TRACING` environment variable before creating runtime
   - Only enable tokio callbacks if CPU tracing is enabled
   - Ensure telemetry guard initialization happens with correct CPU tracing setting

2. **Example macro expansion pattern**:
   ```rust
   // Generated code should look like:
   fn main() {
       let cpu_tracing_enabled = std::env::var("MICROMEGAS_ENABLE_CPU_TRACING")
           .map(|v| v == "true")
           .unwrap_or(false); // Default to disabled for minimal overhead

       let _guard = micromegas_telemetry_sink::TelemetryGuard::new();

       let mut runtime_builder = tokio::runtime::Builder::new_multi_thread()
           .enable_all();

       if cpu_tracing_enabled {
           runtime_builder = runtime_builder.with_tracing_callbacks();
       }

       let runtime = runtime_builder.build().expect("Failed to create Tokio runtime");
       // ... rest of generated code
   }
   ```

3. **Consistency with manual runtime setup**:
   - Both proc macro and manual setup should read environment variable at same point
   - Both should make same decision about registering tokio callbacks
   - Document the correct manual setup pattern for applications not using proc macro

#### 4.6 Service Configuration
1. **Update service startup scripts**:
   - `local_test_env/ai_scripts/start_services.py` should set environment variable to `true` for development
   - `build/docker_command.py` should pass through environment variable
   - Explicitly enable for development/testing environments

2. **Update sink library initialization** in services:
   - **`rust/telemetry-ingestion-srv/src/main.rs`**: Read `MICROMEGAS_ENABLE_CPU_TRACING` and pass to dispatch
   - **`rust/flight-sql-srv/src/main.rs`**: Read environment variable and pass to dispatch
   - **Test utilities**: Update to pass CPU tracing parameter when creating dispatch (default to `true` for tests)
   - **Example pattern**: `init_event_dispatch(logs_size, metrics_size, threads_size, sink, properties, cpu_tracing_enabled)`

3. **Add logging and observability**:
   - Log CPU tracing state at service startup (in sink libraries)
   - Include CPU tracing status in health checks
   - Document performance implications of enabling/disabling

#### 4.7 Testing Strategy
1. **Unit tests for conditional behavior**:
   - Test that thread streams are not created when disabled
   - Test that tokio callbacks are not registered when disabled
   - Test that span events become no-ops when disabled
   - Test proc macro behavior with CPU tracing disabled/enabled
   - Test manual runtime setup with CPU tracing disabled/enabled

2. **Integration tests**:
   - Test trace generation with CPU tracing disabled (should have no thread tracks)
   - Test trace generation with CPU tracing enabled (current behavior)
   - Test that async spans still work regardless of CPU tracing setting
   - Test services startup with different CPU tracing environment variable values

3. **Initialization order tests**:
   - Verify that environment variable is read before tokio runtime creation
   - Verify that dispatch receives correct CPU tracing setting
   - Test that proc macro and manual setup produce identical behavior

#### 4.8 Documentation Updates
1. **Configuration documentation**:
   - Update relevant README files with environment variable
   - Document performance implications and use cases
   - Add examples of when to disable CPU tracing
   - Document proc macro behavior changes

2. **Migration guide**:
   - Explain default behavior change (if changing default to disabled)
   - Instructions for services that need CPU tracing
   - Performance benchmarks showing overhead reduction
   - Update examples showing both proc macro and manual runtime setup

**Expected Outcomes**:
- **Zero runtime overhead** when CPU tracing is disabled
- **No thread stream allocation** when CPU tracing is disabled
- **No tokio callback registration** when CPU tracing is disabled
- **Configurable via environment variable** at process startup
- **Minimal overhead by default** for optimal production performance
- **Clear logging** of CPU tracing state for debugging

### Phase 5: FlightSQL Streaming Table Function

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
- **Async Events Infrastructure**: Complete async span data collection and view system
- **Trace Generation Utility**: End-to-end testing tool for validating async span implementation with proper stream filtering

### ✅ Phase 3 Achievement Highlights
- **Full API Implementation**: Both regular and streaming writers support async spans
- **Production Testing**: Successfully generated 8,356-byte trace with real telemetry data
- **Comprehensive Validation**: 13 unit tests + end-to-end trace validation
- **Performance Verified**: Streaming writer maintains constant memory usage
- **UI Compatible**: Generated traces ready for Perfetto UI visualization
- **Reliability Resolved**: Fixed timestamp issues, stream filtering, and DataFusion query problems

### 🔄 Pending Implementation (In Priority Order)
1. **Phase 4**: CPU Tracing Control (Next Priority)
   - Add configurable CPU tracing with environment variable control
   - Disabled by default to reduce trace size and processing overhead
   - Essential for production deployments where CPU tracing may not be needed

2. **Phase 5-7**: Server-Side Generation (Medium Priority)
   - Eliminates code duplication between client implementations
   - Enables advanced features like real-time streaming via FlightSQL
   - Can leverage completed Phase 2 & 3 infrastructure
   - Focus on FlightSQL table function for `perfetto_trace_chunks`

3. **Python Client Refactoring** (Lower Priority)
   - Remove duplicate Perfetto generation logic
   - Use server-side generation via FlightSQL queries
   - Maintain CLI compatibility with async span support

### Next Recommended Steps
1. **Immediate**: Phase 3 provides complete async span visualization capability
2. **Short-term**: Implement CPU tracing control (Phase 4) - essential for production usage
3. **Medium-term**: Consider implementing server-side generation for code consolidation
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
