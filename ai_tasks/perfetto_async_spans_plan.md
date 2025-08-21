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

### Phase 1: Production-Ready Analytics Web App with Current Client

**Objective**: Create a modern, production-ready analytics web application using existing `perfetto_trace_client.rs` for immediate testing capability and future scalability

**Detailed Implementation**: See [analytics_web_app_plan.md](./analytics_web_app_plan.md) for complete Phase 1 specifications including:
- Architecture decisions (Next.js + React + Axum)
- Technology stack and dependencies
- Detailed API design with HTTP streaming
- React component architecture
- Production-ready features (observability, security, deployment)
- Development experience and testing strategy
- WebAssembly decision analysis
- HTTP streaming implementation patterns

**Key Features**:
- **HTTP Streaming**: Real-time progress updates with single-request trace delivery
- **Modern UI**: React components with TypeScript, Tailwind CSS, Radix UI
- **Production Ready**: Observability, security, scalability considerations
- **Testing Foundation**: Platform for validating all subsequent async span phases

### Phase 2: Perfetto Writer Streaming Support

**Objective**: Make Perfetto Writer capable of streaming generation (foundation for SQL approach)

**Key Insight**: Perfetto binary format uses varint length-prefixed TracePackets that can be written incrementally without holding the complete Trace in memory.

**Tasks**:
1. **Create StreamingPerfettoWriter**:
   - New `StreamingPerfettoWriter<W: Write>` struct for direct file writing
   - Implement `write_packet()` method that writes individual TracePackets with proper protobuf framing
   - Handle varint length encoding for each packet (field tag 0x0A + length + packet bytes)
   - Maintain interning state (names, categories, source_locations) for efficient string handling

2. **Two-phase streaming approach**:
   - **Phase 1**: Lightweight pre-pass to collect all unique strings and assign stable IDs
   - **Phase 2**: Stream packets directly to file as they're generated
   - Emit setup packets (process descriptor, interned data) before span events
   - Stream span events incrementally as DataFusion produces them

3. **Client-side streaming assembly** (alternative to file writing):
   - Server streams individual TracePackets via FlightSQL chunks
   - Client uses `StreamingTraceBuilder` to collect packets into final Trace
   - Enables real-time progress updates and backpressure

4. **Unit tests for streaming Writer**:
   - Test direct file writing produces identical binary output to `Trace.encode_to_vec()`
   - Test varint encoding correctness for various packet sizes
   - Test interning state management across streaming operations
   - Compare streaming vs non-streaming output for identical traces
   - Test memory usage remains constant regardless of trace size

### Phase 3: Async Event Support in Perfetto Writer

**Objective**: Add async track support to the Perfetto writer (independent of streaming)

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

4. **Unit tests for async events** (following existing Writer test patterns):
   - Test async track creation with various span hierarchies
   - Test async span event generation (begin/end pairs)
   - Test UUID generation and track parenting
   - Verify Perfetto protobuf structure and interning

5. **Test via web app**:
   - Use Phase 1 web app to validate async track generation
   - Compare traces with/without async events
   - Verify Perfetto UI displays async tracks correctly

### Phase 4: FlightSQL Streaming Table Function

**Objective**: Implement FlightSQL chunked binary streaming infrastructure

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

### Phase 5: Server-Side Perfetto Generation

**Objective**: Move trace generation logic from client to server via SQL table function

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

### Phase 6: Refactor Client to Use SQL Generation

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

### Phase 7: Data Processing Optimization

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

### Phase 8: Python Client Refactoring

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

### Phase 9: Integration Testing and Validation

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

## Migration Strategy

- **Backward compatible**: Existing thread-only trace generation continues to work
- **Flexible span selection**: Users can choose thread spans only, async spans only, or both
- **Gradual rollout**: Test with small processes before enabling for large-scale traces
- **Python client migration**: Phase 4 eliminates code duplication by having Python CLI call Rust implementation

## Dependencies

- Existing `rust/perfetto` crate and writer infrastructure
- Existing `AsyncEventsView` and async events data pipeline
- Existing `perfetto_trace_client.rs` query infrastructure
- No new external dependencies required

**Note**: The Python CLI script (`python/micromegas/cli/write_perfetto.py`) completely duplicates functionality that already exists in the Rust Perfetto client (`rust/public/src/client/perfetto_trace_client.rs`). Both query the analytics service and generate Perfetto traces with identical logic. The Python CLI should be refactored to call the existing Rust client instead of maintaining a duplicate implementation. This would eliminate the need to implement async spans in multiple places and ensure consistent behavior.

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