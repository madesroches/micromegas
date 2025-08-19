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

### 🔍 What's Missing

- **Depth Field**: Async span events don't include depth information
- **Depth Calculation**: No mechanism to calculate nesting depth for async operations
- **Hierarchical Analysis**: Difficult to analyze async call trees without depth information

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

### Phase 1: Extend Async Span Event Structures

#### 1.1 Update Event Definitions
**Location**: `rust/tracing/src/spans/events.rs`

Add `depth` field to async span events:
```rust
#[derive(Debug, TransitReflect)]
pub struct BeginAsyncSpanEvent {
    pub span_desc: &'static SpanMetadata,
    pub span_id: u64,
    pub parent_span_id: u64,
    pub depth: u32,  // NEW: Nesting depth in async call stack
    pub time: i64,
}

#[derive(Debug, TransitReflect)]
pub struct EndAsyncSpanEvent {
    pub span_desc: &'static SpanMetadata,
    pub span_id: u64,
    pub parent_span_id: u64,
    pub depth: u32,  // NEW: Nesting depth in async call stack
    pub time: i64,
}

#[derive(Debug, TransitReflect)]
pub struct BeginAsyncNamedSpanEvent {
    pub span_location: &'static SpanLocation,
    pub name: StringId,
    pub span_id: u64,
    pub parent_span_id: u64,
    pub depth: u32,  // NEW: Nesting depth in async call stack
    pub time: i64,
}

#[derive(Debug, TransitReflect)]
pub struct EndAsyncNamedSpanEvent {
    pub span_location: &'static SpanLocation,
    pub name: StringId,
    pub span_id: u64,
    pub parent_span_id: u64,
    pub depth: u32,  // NEW: Nesting depth in async call stack
    pub time: i64,
}
```

#### 1.2 Update Dispatch Functions
**Location**: `rust/tracing/src/dispatch.rs`

Modify async scope functions to accept depth as a parameter:
```rust
#[inline(always)]
pub fn on_begin_async_scope(scope: &'static SpanMetadata, parent_span_id: u64, depth: u32) -> u64 {
    let id = G_ASYNC_SPAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    on_thread_event(BeginAsyncSpanEvent {
        span_desc: scope,
        span_id: id as u64,
        parent_span_id,
        depth,  // NEW: Passed as argument
        time: now(),
    });
    id as u64
}

#[inline(always)]
pub fn on_end_async_scope(span_id: u64, parent_span_id: u64, scope: &'static SpanMetadata, depth: u32) {
    on_thread_event(EndAsyncSpanEvent {
        span_desc: scope,
        span_id,
        parent_span_id,
        depth,  // NEW: Passed as argument
        time: now(),
    });
}
```

#### 1.3 Update InstrumentedFuture
**Location**: `rust/tracing/src/spans/instrumented_future.rs`

Calculate depth from the async call stack and pass it to dispatch functions:
```rust
fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.project();
    ASYNC_CALL_STACK.with(|stack_cell| {
        let stack = unsafe { &mut *stack_cell.get() };
        assert!(!stack.is_empty());
        let parent = stack[stack.len() - 1];
        // Calculate depth: stack length - 1 (since stack[0] is root with depth 0)
        let depth = (stack.len().saturating_sub(1)) as u32;
        // ... use depth in on_begin_async_scope and on_end_async_scope calls
    })
}
```

#### 1.4 Remove Deprecated Guards
**Location**: `rust/tracing/src/guards.rs`

The simple `AsyncSpanGuard` and `AsyncNamedSpanGuard` pass `depth: 0` and don't have access to proper depth context. These should be deprecated and eventually removed in favor of:

1. **`InstrumentedFuture`**: For proper async span instrumentation with accurate depth tracking
2. **Proc macros**: `#[span_fn]` and `.instrument()` extension methods that use `InstrumentedFuture` internally

**Deprecation Strategy**:
```rust
#[deprecated(note = "Use InstrumentedFuture or proc macros for proper async depth tracking")]
pub struct AsyncSpanGuard { ... }

#[deprecated(note = "Use InstrumentedFuture or proc macros for proper async depth tracking")]
pub struct AsyncNamedSpanGuard { ... }
```

**Migration Path**:
- Replace `AsyncSpanGuard::new(span_desc)` with `future.instrument(span_desc)`
- Replace manual guard usage with `#[span_fn]` proc macro for async functions
- Update documentation to recommend proper async instrumentation patterns

### Phase 2: Update Async Events View

#### 2.1 Update Async Events Schema
**Location**: `rust/analytics/src/lakehouse/async_events_table.rs`

Add depth field to the async events schema:
```rust
pub fn get_async_events_schema() -> Schema {
    Schema::new(vec![
        Field::new("stream_id", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("block_id", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("time", DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())), false),
        Field::new("event_type", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        Field::new("span_id", DataType::Int64, false),
        Field::new("parent_span_id", DataType::Int64, false),
        Field::new("depth", DataType::UInt32, false),  // NEW: Depth field
        Field::new("name", DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)), false),
        // ... other fields
    ])
}
```

#### 2.2 Update Record Builder
**Location**: `rust/analytics/src/lakehouse/async_events_table.rs`

Add depth to the record builder:
```rust
pub struct AsyncEventsRecordBuilder {
    // ... existing fields
    depths: UInt32Builder,  // NEW
    // ... other fields
}

impl AsyncEventsRecordBuilder {
    pub fn append_begin_event(&mut self, /* ... */, depth: u32) -> Result<()> {
        // ... existing fields
        self.depths.append_value(depth);  // NEW
        // ... rest
    }

    pub fn append_end_event(&mut self, /* ... */, depth: u32) -> Result<()> {
        // ... existing fields
        self.depths.append_value(depth);  // NEW
        // ... rest
    }
}
```

#### 2.3 Update Block Parser
**Location**: `rust/analytics/src/thread_block_processor.rs`

Update async event parsing to extract depth field:
```rust
// Update on_begin_async_scope and on_end_async_scope to extract depth
fn on_begin_async_scope(&mut self, block_id: &str, scope: ScopeDesc, ts: i64, span_id: i64, parent_span_id: i64, depth: u32) -> Result<bool>;
fn on_end_async_scope(&mut self, block_id: &str, scope: ScopeDesc, ts: i64, span_id: i64, parent_span_id: i64, depth: u32) -> Result<bool>;
```

### Phase 3: Testing and Validation

#### 3.1 Unit Tests
**Location**: `rust/analytics/tests/`

Create comprehensive tests for depth tracking:
```rust
#[test]
fn test_async_depth_tracking() {
    // Test nested async operations generate correct depths
    // Test parallel async operations have correct depths
    // Test depth consistency between begin/end events
}

#[test]
fn test_async_events_view_with_depth() {
    // Test async events view includes depth field
    // Test SQL queries filtering by depth work correctly
    // Test depth values match expected hierarchy
}
```

#### 3.2 Integration Tests
**Location**: `python/micromegas/tests/test_async_events.py`

Update existing async events tests to validate depth:
```python
def test_async_events_depth_hierarchy():
    """Test depth tracking in nested async operations"""
    # Generate nested async operations
    # Query async events with depth information
    # Validate depth values match expected hierarchy

def test_async_events_depth_queries():
    """Test SQL queries using depth field"""
    # Test filtering by depth level
    # Test aggregating by depth
    # Test depth-based performance analysis
```

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

### High Priority (Core Functionality)
1. **Event Structure Updates**: Add depth field to async span events
2. **Dispatch Function Updates**: Calculate depth from async call stack
3. **Schema Updates**: Add depth to async events view
4. **Basic Testing**: Ensure depth tracking works correctly

### Medium Priority (Enhanced Features)
1. **Advanced Query Examples**: Documentation with depth-based queries
2. **Performance Optimization**: Ensure depth calculation doesn't impact performance
3. **Integration Testing**: Comprehensive test suite validation
4. **Deprecate Legacy Guards**: Mark `AsyncSpanGuard` and `AsyncNamedSpanGuard` as deprecated

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

### Functional Requirements
- ✅ Async span events include accurate depth information
- ✅ Depth values correctly represent async call hierarchy nesting
- ✅ SQL queries can filter and aggregate by depth
- ✅ Existing async events functionality remains unaffected

### Performance Requirements
- ✅ Depth calculation adds <1ns overhead per async event
- ✅ Memory usage increases <5% for async events storage
- ✅ Query performance on depth field is efficient

### Testing Requirements
- ✅ 100% test coverage for depth tracking functionality
- ✅ Integration tests validate end-to-end depth tracking
- ✅ Performance tests confirm overhead requirements

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

### Implementation Steps
1. **Update Event Structures**: Add depth field to async span events
2. **Implement Depth Calculation**: Modify dispatch functions to include depth
3. **Update Schema and Parsing**: Extend async events view with depth field
4. **Add Tests**: Comprehensive testing of depth tracking functionality
5. **Update Documentation**: Schema and query examples with depth usage
6. **Performance Validation**: Ensure minimal performance impact

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
