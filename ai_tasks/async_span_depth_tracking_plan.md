# Plan: Add Depth to Async Span Events

## Overview

Add depth tracking to async span events to enable call hierarchy analysis for async code, similar to how the `thread_spans` view provides depth information for synchronous call trees. This will enable better async operation visualization, debugging, and performance analysis.

## Current State Analysis

### ✅ What's Working
- **Async Span Events**: Complete implementation with `BeginAsyncSpanEvent`/`EndAsyncSpanEvent` capturing span lifecycle
- **Parent-Child Relationships**: `parent_span_id` field correctly tracks async span hierarchies
- **Thread-Local Call Stack**: `ASYNC_CALL_STACK` in `InstrumentedFuture` maintains current async context
- **Async Events View**: `view_instance('async_events', process_id)` provides raw async event data
- **Thread Spans Depth**: Existing `depth` field in `thread_spans` view shows how depth is implemented
- **✅ COMPLETED - Depth Field Implementation**: All async span events now include depth information
- **✅ COMPLETED - Depth Calculation**: `InstrumentedFuture` calculates depth from async call stack
- **✅ COMPLETED - Schema Extension**: Async events table schema includes depth field
- **✅ COMPLETED - End-to-End Integration**: Depth tracking works from event generation to storage

### 🔍 What's Missing (Remaining Tasks)

- **Basic Testing**: Unit tests specifically for depth tracking functionality
- **Integration Testing**: End-to-end validation of depth tracking in real scenarios
- **Documentation Updates**: Schema documentation and query examples with depth usage
- **Performance Validation**: Ensure minimal performance impact of depth tracking

## Target: Enhanced Async Span Events with Depth

### Expected Use Cases

```sql
-- Find top-level async operations (most common use case)

SELECT name, AVG(duration_ms) as avg_duration, COUNT(*) as count
FROM (
  SELECT
    begin_events.name,
    begin_events.depth,
    CAST((end_events.time - begin_events.time) AS BIGINT) / 1000000 as duration_ms
  FROM
    (SELECT * FROM view_instance('async_events', 'process_123') WHERE event_type = 'begin') begin_events
  LEFT JOIN
    (SELECT * FROM view_instance('async_events', 'process_123') WHERE event_type = 'end') end_events
    ON begin_events.span_id = end_events.span_id
)
WHERE depth < 3  -- Only shallow operations (top-level and immediate children)
GROUP BY name
ORDER BY avg_duration DESC;

-- Compare performance by call depth
SELECT depth, COUNT(*) as span_count, AVG(duration_ms) as avg_duration
FROM async_spans_with_duration
GROUP BY depth
ORDER BY depth;

-- Find operations that spawn many nested async calls
SELECT name, depth, COUNT(*) as nested_count
FROM async_spans_with_duration
WHERE depth > 0
GROUP BY name, depth
HAVING COUNT(*) > 10  -- Functions that create many nested async operations
ORDER BY nested_count DESC;
```

## Implementation Strategy

### ✅ Phase 1: Extend Async Span Event Structures (COMPLETED)

#### ✅ 1.1 Update Event Definitions (COMPLETED)
**Location**: `rust/tracing/src/spans/events.rs`

**Status**: ✅ COMPLETED - Added `depth: u32` field to all async span events:
- `BeginAsyncSpanEvent`
- `EndAsyncSpanEvent`
- `BeginAsyncNamedSpanEvent`
- `EndAsyncNamedSpanEvent`

#### ✅ 1.2 Update Dispatch Functions (COMPLETED)
**Location**: `rust/tracing/src/dispatch.rs`

**Status**: ✅ COMPLETED - All async scope functions now accept depth as a parameter:
- `on_begin_async_scope(scope, parent_span_id, depth)`
- `on_end_async_scope(span_id, parent_span_id, scope, depth)`
- `on_begin_async_named_scope(span_location, name, parent_span_id, depth)`
- `on_end_async_named_scope(span_id, parent_span_id, span_location, name, depth)`

#### ✅ 1.3 Update InstrumentedFuture (COMPLETED)
**Location**: `rust/tracing/src/spans/instrumented_future.rs`

**Status**: ✅ COMPLETED - Proper depth calculation from async call stack:
```rust
// Calculate depth: stack.len() - 1 gives us the depth of the new span
// (stack[0] is root, so first real span at stack.len()=1 has depth=0)
let depth = (stack.len().saturating_sub(1)) as u32;
```

#### ✅ 1.4 Remove Deprecated Guards (COMPLETED)
**Location**: `rust/tracing/src/guards.rs`

**Status**: ✅ COMPLETED - Updated simple guards to use `depth: 0` as temporary measure. Guards marked for future deprecation in favor of `InstrumentedFuture` and proc macros.

### ✅ Phase 2: Update Async Events View (COMPLETED)

#### ✅ 2.1 Update Async Events Schema (COMPLETED)
**Location**: `rust/analytics/src/async_events_table.rs`

**Status**: ✅ COMPLETED - Schema now includes depth field:
- Added `depth: u32` field to `AsyncEventRecord` struct
- Updated `async_events_table_schema()` with `Field::new("depth", DataType::UInt32, false)`
- Schema now has 11 columns total (was 10)

#### ✅ 2.2 Update Record Builder (COMPLETED)
**Location**: `rust/analytics/src/async_events_table.rs`

**Status**: ✅ COMPLETED - `AsyncEventRecordBuilder` handles depth:
- Added `depths: PrimitiveBuilder<UInt32Type>` field
- Updated `append()` method to store depth values
- Updated `finish()` method to include depth column in output

#### ✅ 2.3 Update Block Parser (COMPLETED)
**Location**: `rust/analytics/src/async_block_processing.rs`

**Status**: ✅ COMPLETED - Event parsing extracts depth field:
- Extended `AsyncBlockProcessor` trait with depth parameter
- Updated helper functions to extract depth from serialized events
- Updated `AsyncEventCollector` to store depth in lakehouse records
- All tests updated to handle new schema and depth values### 🔄 Phase 3: Testing and Validation (IN PROGRESS)

#### ✅ 3.1 Basic Instrumentation Tests (COMPLETED)
**Location**: `rust/tracing/tests/async_depth_tracking_tests.rs`

**Status**: ✅ COMPLETED - Validates basic async instrumentation works:
- ✅ `test_basic_async_instrumentation` - Basic async operations with depth tracking
- ✅ `test_nested_async_instrumentation` - Nested async operations
- ✅ `test_parallel_async_operations` - Parallel async tasks
- ✅ `test_deeply_nested_async` - Multi-level nesting validation
- ✅ `test_error_handling_with_instrumentation` - Error handling doesn't break depth tracking

#### 📋 3.2 Python Integration Tests (TODO)
**Location**: `python/micromegas/tests/test_async_events_depth.py`

**Status**: 📋 TODO - End-to-end validation via Python client:
- 📋 Generate nested async operations with micromegas-tracing
- 📋 Query async_events view via FlightSQL
- 📋 Validate depth values in query results match expected hierarchy
- 📋 Test depth-based filtering and aggregation queries
- 📋 Verify performance with realistic async workloads

#### 🔄 3.3 End-to-End Validation (IN PROGRESS)
**Current Status**: Basic tests implemented and passing, need comprehensive validation:
- ✅ Event generation with depth field
- ✅ Event storage in analytics layer
- ✅ Schema consistency (11 columns)
- ⏳ query async event depth using sql in python

### Phase 4: Documentation Updates

#### 4.1 Schema Documentation
**Location**: `mkdocs/docs/query-guide/schema-reference.md`

Update async_events view documentation:
```markdown
### `async_events`

| Field | Type | Description |
|-------|------|-------------|
| `span_id` | `Int64` | Unique async span identifier |
| `parent_span_id` | `Int64` | Parent span identifier |
| `depth` | `UInt32` | Nesting depth in async call hierarchy |
| `event_type` | `Dictionary(Int16, Utf8)` | "begin" or "end" |
| ... | ... | ... |

**Example Queries:**
```sql
-- Find top-level async operations
SELECT name, depth, AVG(duration_ms) as avg_duration
FROM async_spans_with_duration
WHERE depth <= 2  -- Focus on top-level and shallow operations
GROUP BY name, depth
ORDER BY avg_duration DESC;
```
```

#### 4.2 Query Pattern Examples
Add examples showing how to use depth for:
- Performance analysis by call depth
- Identifying problematic deep async nesting
- Visualizing async operation hierarchies

## Implementation Priority

### ✅ High Priority (Core Functionality) - COMPLETED
1. **✅ Event Structure Updates**: Add depth field to async span events
2. **✅ Dispatch Function Updates**: Accept depth as parameter in dispatch functions
3. **✅ Schema Updates**: Add depth to async events view
4. **✅ Basic Integration**: Ensure depth tracking works end-to-end

### 🔄 Medium Priority (Enhanced Features) - IN PROGRESS
1. **Advanced Query Examples**: Documentation with depth-based queries
2. **Performance Optimization**: Ensure depth calculation doesn't impact performance
3. **Integration Testing**: Comprehensive test suite validation
4. **Deprecate Legacy Guards**: Mark `AsyncSpanGuard` and `AsyncNamedSpanGuard` as deprecated

### 📋 Low Priority (Future Enhancements)
1. **Visualization Support**: Tools for rendering async call hierarchies
2. **Alerting Integration**: Depth-based performance alerts
3. **Advanced Analytics**: Statistical analysis of async nesting patterns

### Low Priority (Future Enhancements)
1. **Visualization Support**: Tools for rendering async call hierarchies
2. **Alerting Integration**: Depth-based performance alerts
3. **Advanced Analytics**: Statistical analysis of async nesting patterns

## Technical Considerations

### Performance Impact
- **Minimal Overhead**: Depth calculation is a simple stack length operation
- **Thread-Local Access**: Uses existing ASYNC_CALL_STACK without additional locking
- **Storage Efficiency**: UInt32 depth field adds minimal storage overhead

### Backward Compatibility
- **Schema Evolution**: New depth field can be added without breaking existing queries
- **Default Values**: Existing data without depth can default to 0 or be calculated retroactively
- **Gradual Migration**: Existing async events remain functional during migration

### Edge Cases
- **Stack Overflow Protection**: Depth calculation should handle very deep nesting gracefully
- **Cross-Thread Async**: Ensure depth tracking works correctly for async operations spanning threads
- **Error Handling**: Robust handling of async call stack inconsistencies

## Success Criteria

### ✅ Functional Requirements - COMPLETED
- ✅ Async span events include accurate depth information
- ✅ Depth values correctly represent async call hierarchy nesting
- ✅ SQL queries can filter and aggregate by depth
- ✅ Existing async events functionality remains unaffected

### 🔄 Performance Requirements - NEEDS VALIDATION
- ⏳ Depth calculation adds <1ns overhead per async event
- ⏳ Memory usage increases <5% for async events storage
- ⏳ Query performance on depth field is efficient

### 🔄 Testing Requirements - IN PROGRESS
- ✅ Updated existing tests to handle depth field
- ✅ Basic Rust-level instrumentation tests completed
- 📋 Python integration tests to validate end-to-end depth tracking via FlightSQL
- ⏳ Performance tests confirm overhead requirements

## Future Enhancements

### Async Call Tree View
Consider implementing an `async_spans` view similar to `thread_spans` that provides:
- Calculated duration for each async operation
- Complete call tree structure with parent-child relationships
- Depth-based aggregation and analysis capabilities

### Cross-Process Async Tracking
Future enhancement could extend depth tracking across process boundaries for distributed async operations, enabling full distributed tracing capabilities.

### Visual Analytics
Integration with visualization tools to render async operation flame graphs and call trees based on the depth information.

## Development Workflow

### ✅ Implementation Steps - COMPLETED
1. **✅ Update Event Structures**: Add depth field to async span events
2. **✅ Implement Depth Calculation**: Modify dispatch functions to accept depth
3. **✅ Update Schema and Parsing**: Extend async events view with depth field
4. **✅ Update Tests**: Fix all tests to handle new schema
5. **⏳ Add Tests**: Comprehensive testing of depth tracking functionality
6. **⏳ Update Documentation**: Schema and query examples with depth usage
7. **⏳ Performance Validation**: Ensure minimal performance impact

### 🔄 Current Status
- **Phases 1 & 2**: ✅ COMPLETED
- **Phase 3**: 🔄 Testing and Validation (basic Rust tests completed, Python integration tests needed)
- **Phase 4**: 📋 Documentation Updates (ready to start)

### ✅ Commits Made
1. **Phase 1 Commit**: `7e72a483` - Add depth field to async span events
   - Event structures, dispatch functions, InstrumentedFuture updates
   - Guards updated with temporary depth=0
2. **Phase 2 Commit**: `[latest]` - Update async events schema with depth field
   - Schema extension, record builder, block processing updates
   - All tests updated and passing

### Testing Strategy
- **Unit Tests**: Depth calculation logic and edge cases
- **Integration Tests**: End-to-end async events with depth
- **Performance Tests**: Overhead measurement and optimization
- **Manual Testing**: Real-world async operation validation

### Validation Approach
- **Depth Consistency**: Begin and end events have matching depth values
- **Hierarchy Correctness**: Depth values accurately reflect nesting structure
- **Query Functionality**: SQL operations on depth field work as expected
- **Performance Impact**: Minimal overhead in high-frequency scenarios
