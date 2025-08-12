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

