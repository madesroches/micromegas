# Plan for Issue #466: Track Query Latency Using Metrics & Async Traces

## Overview
Enhance the FlightSQL service with comprehensive query latency tracking using both metrics and async span tracing to provide detailed observability into query performance.

## 1. Analysis Phase
- **Current State Review**: Examine existing FlightSQL service instrumentation in `rust/public/src/servers/flight_sql_service_impl.rs`
  - Current basic timing: `imetric!("request_duration", "ticks", duration as u64)` at lines 189, 237, 295, 323, 427
  - Existing metrics infrastructure: `imetric!`, `fmetric!` macros from tracing crate
- **Infrastructure Assessment**: Review async span tracing capabilities
  - Available: `async_span_scope!` macro for distributed tracing
  - Async events infrastructure already in place for correlation
- **Key Execution Phases**: Identify query processing stages to instrument
  - Query parsing and validation
  - Session context creation  
  - SQL planning (DataFusion logical plan creation)
  - Query execution (physical plan execution)

## 2. Async Span Integration  
- **Primary Span**: Add `async_span_scope!("flight_sql_query_execution")` around entire `execute_query` method
- **Nested Spans**: Create child spans for major phases:
  - `async_span_scope!("query_parsing")` for SQL validation
  - `async_span_scope!("session_context_creation")` for DataFusion context setup
  - `async_span_scope!("query_planning")` for logical/physical plan creation
  - `async_span_scope!("query_execution")` for plan execution
- **Correlation**: Ensure spans correlate with metrics through consistent timing and metadata

## 3. Enhanced Metrics Implementation
- **Granular Timing Metrics**: Break down `request_duration` into phases:
  - `query_latency_parse` - Query parsing/validation time
  - `query_latency_session` - Session context creation time  
  - `query_latency_planning` - SQL planning time (DataFusion logical plan)
  - `query_latency_execution` - Query execution time (physical plan execution)
  - `query_latency_total` - Total end-to-end time
- **Query Characteristics**: Add dimensional metrics with properties:
  - Query size (bytes)
  - Has time range filter (boolean)
  - Has LIMIT clause (boolean)
  - Result column count
  - Query complexity category (small/medium/large based on size)
- **Success Metrics**: Track query completion rates
  - `query_success` - Count of successful queries
  - `query_result_columns` - Number of columns returned

## 4. Error Tracking Enhancement
- **Error Classification**: Add specific error metrics by failure type:
  - `query_errors_by_type` with properties: `session_context_creation`, `sql_planning`, `execution`
  - `query_errors` - Total error count
- **Error Context**: Include error details in async spans for debugging
- **Backwards Compatibility**: Maintain existing error handling while adding observability

## 5. Implementation Details
- **Timing Precision**: Use microseconds for all latency metrics for consistency
- **Property Arrays**: Use slice syntax for tagged metrics: `[("key", "value")].as_slice()`
- **Legacy Support**: Keep existing `request_duration` metric for backwards compatibility
- **Units**: Standardize on "microseconds" for latency, "bytes" for size, "count" for quantities

## 6. Testing & Validation
- **Sample Queries**: Test with various query types:
  - Simple SELECT queries
  - Complex JOINs with time ranges
  - Queries with/without LIMIT clauses
  - Queries of different sizes
- **Metrics Verification**: Ensure all metrics are properly recorded in telemetry
- **Trace Validation**: Confirm async spans appear correctly in distributed traces
- **Performance Impact**: Verify minimal overhead from added instrumentation

## 7. Critical Bug Fix
- **Stream Completion Issue**: Fix incorrect `request_duration` metric timing
  - **Problem**: Current `imetric!("request_duration", "ticks", duration as u64)` at line 198 measures only query setup time, not actual data streaming completion
  - **Impact**: Metric reports query as "complete" when stream is just created, not when data transfer finishes
  - **Implementation Completed**:
    - Created `CompletionTrackedStream` wrapper that tracks stream consumption
    - Renamed current metric to `query_setup_duration` for accuracy
    - Added new metrics:
      - `query_duration_total` - Actual end-to-end time including data transfer
      - `query_completed_successfully` - Success completion counter
      - `query_duration_with_error` - Duration when errors occur
    - Maintained `request_duration` for backwards compatibility
    - Code compiles successfully
  - **Solution Options**:
    1. Rename current metric to `query_setup_duration` to reflect actual measurement
    2. Implement stream wrapper to track actual completion time
    3. Add separate `query_stream_duration` metric for end-to-end measurement
  - **Recommendation**: Keep current timing for query preparation phases, add new instrumentation for stream completion

## 8. Code Quality & Deployment
- **Formatting**: Run `cargo fmt` from `rust/` directory before commit
- **Linting**: Run `cargo clippy --workspace -- -D warnings` to catch issues
- **Testing**: Execute `cargo test` to ensure no regressions
- **API Compatibility**: Ensure no breaking changes to FlightSQL service interface
- **Documentation**: Add inline comments explaining new metrics and spans

## Expected Outcomes
- Detailed query performance visibility across all execution phases
- Correlation between metrics and distributed traces for end-to-end observability
- Ability to identify performance bottlenecks in specific query phases
- Enhanced error tracking with categorization for debugging
- Foundation for query performance optimization and alerting