# Perfetto Writer Channel Refactor Plan

## Overview
Refactor AsyncStreamingPerfettoWriter to use channels for data flow, preventing deadlocks and maintaining reasonable memory usage without loading the full trace into memory.

## Current Issues
- Potential deadlocks in current async implementation
- Memory usage concerns with large traces
- Need for better streaming architecture

## Proposed Solution

### Simpler Approach: Direct Streaming (No Channels Needed)
The real issue isn't AsyncStreamingPerfettoWriter - it's the execution plan architecture:

**Current Problem:**
1. `generate_perfetto_trace_stream()` calls `generate_complete_perfetto_trace()` 
2. `generate_complete_perfetto_trace()` builds entire trace in `Vec<u8>` buffer
3. Then splits complete trace into 64KB chunks for streaming
4. This defeats streaming and causes memory issues

**Simple Fix:**
Replace `generate_complete_perfetto_trace()` with direct chunk yielding:
- Remove the `Vec<u8>` buffer accumulation 
- Yield RecordBatch chunks directly as packets are generated
- Use existing `AsyncStreamingPerfettoWriter` but with streaming output
- No channels, no background tasks, no complex synchronization needed

## Implementation Steps

### Phase 1: Create Streaming Chunk Sender
1. **Create ChunkSender that implements AsyncWrite**
   ```rust
   struct ChunkSender {
       chunk_sender: mpsc::Sender<Result<RecordBatch>>,
       chunk_id: i32,
       current_chunk: Vec<u8>,
   }
   
   impl AsyncWrite for ChunkSender {
       async fn write(&mut self, buf: &[u8]) -> Result<usize> {
           self.current_chunk.extend_from_slice(buf);
           
           // Yield chunk when it reaches reasonable size (e.g., 8KB)
           if self.current_chunk.len() >= 8192 {
               self.flush_chunk().await?;
           }
           Ok(buf.len())
       }
       
       async fn flush(&mut self) -> Result<()> {
           if !self.current_chunk.is_empty() {
               self.flush_chunk().await?;
           }
           Ok(())
       }
   }
   ```

2. **Replace generate_complete_perfetto_trace architecture**
   - Remove the function entirely
   - Move trace generation logic directly into the stream generator
   - Use `ChunkSender` as the AsyncWrite destination for `AsyncStreamingPerfettoWriter`

### Phase 2: Stream-First Architecture
1. **Modify generate_perfetto_trace_stream()**
   ```rust
   fn generate_perfetto_trace_stream(...) -> impl Stream<Item = DFResult<RecordBatch>> {
       stream! {
           let (chunk_sender, mut chunk_receiver) = mpsc::channel(16);
           let chunk_sender = ChunkSender::new(chunk_sender);
           let mut writer = AsyncStreamingPerfettoWriter::new(chunk_sender, &process_id);
           
           // Spawn trace generation in background
           let generation_task = tokio::spawn(async move {
               // Process descriptor
               writer.emit_process_descriptor(&process_exe).await?;
               writer.flush().await?; // Forces chunk emission
               
               // Thread descriptors + spans
               for thread in threads {
                   writer.emit_thread_descriptor(...).await?;
                   // Generate spans for this thread
                   // writer.flush() periodically to create chunks
               }
               
               // Async spans if needed
               if async_spans_requested {
                   writer.emit_async_track_descriptor().await?;
                   // Generate async spans
               }
               
               writer.flush().await?; // Final chunk
               Result::<(), anyhow::Error>::Ok(())
           });
           
           // Stream chunks as they become available
           while let Some(chunk_result) = chunk_receiver.recv().await {
               yield chunk_result;
           }
           
           // Wait for generation to complete
           generation_task.await??;
       }
   }
   ```

### Phase 3: Memory-Efficient Span Processing  
1. **Stream spans directly from DataFusion**
   - Process spans in batches from DataFusion query streams
   - Emit spans to writer as they're processed (don't accumulate)
   - Use periodic `writer.flush()` to control chunk boundaries

2. **Remove all trace buffering**
   - No `Vec<u8>` buffers anywhere
   - No "generate complete trace then chunk" pattern
   - Memory usage bounded by chunk size (8KB) + channel buffer (16 chunks = ~128KB max)

### Phase 4: Simple Deadlock Prevention
1. **Natural backpressure through channels**
   - `ChunkSender` blocks when channel is full (bounded channel)
   - This naturally applies backpressure to span generation
   - No explicit timeout handling needed

2. **Error propagation**
   - Channel closes on errors, terminating stream
   - Background task errors propagated through join handle
   - Clean failure modes without hanging

### Phase 5: Testing & Validation
1. **Verify streaming behavior**
   - Chunks should appear as soon as flush() is called
   - Memory usage should be constant regardless of trace size
   - No "generate complete trace" step

## Technical Implementation Details

### Current Data Flow Analysis
Based on code analysis of `/rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs`:

**Current Flow (Problematic):**
1. `generate_complete_perfetto_trace()` creates `trace_buffer = Vec::new()` (line 259)
2. `AsyncStreamingPerfettoWriter::new(&mut trace_buffer, ...)` (line 260)
3. All packets written to memory buffer via AsyncWrite trait
4. Buffer grows to contain entire trace before returning
5. Comment notes "it's insane to keep this in memory"

**Root Cause of Previous Deadlock:**
- The execution plan needs to stream chunks as they're generated
- But current writer accumulates everything in memory first
- This creates memory pressure and potential blocking behavior

### Channel-Based Flow (Solution)
**New Flow:**
1. `ChannelPerfettoWriter::new(chunk_sink, process_id, 1000)` 
2. Background task receives TracePackets and immediately writes chunks
3. Database queries stream directly to packets via channel
4. Memory usage bounded by channel capacity (1000 packets ~= 100KB max)
5. No trace ever fully materialized in memory

### Specific Code Changes Required

**In `/rust/perfetto/src/streaming_writer.rs`:**
- Add new `ChannelPerfettoWriter` struct alongside existing `AsyncStreamingPerfettoWriter`
- Keep existing writer for backward compatibility
- Implement packet-based channel communication

**In `/rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs`:**
- Replace line 259: `let mut trace_buffer = Vec::new();` 
- Replace line 260: `let mut writer = AsyncStreamingPerfettoWriter::new(&mut trace_buffer, &process_id);`
- Add channel-based chunk streaming in `PerfettoTraceStream::poll_next()`
- Remove line 291: `Ok(trace_buffer)` return

**Channel Configuration:**
- **Capacity**: 1000-5000 packets (tunable based on memory constraints)
- **Packet Size**: ~50-200 bytes per TracePacket (names, spans, descriptors)
- **Total Buffer**: ~50KB-1MB maximum memory usage
- **Timeout**: 100ms timeout for `try_send()` operations to prevent indefinite blocking

## Success Criteria
- No deadlocks under any conditions
- Memory usage remains bounded regardless of trace size
- Maintains or improves current performance
- All existing functionality preserved
- Clean error handling and resource cleanup