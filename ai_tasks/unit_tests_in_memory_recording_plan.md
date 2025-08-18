# Unit Tests In-Memory Recording Plan

## Current State Analysis

### Tests Already Using In-Memory Recording ✅
These tests properly use `InMemorySink` and `init_in_mem_tracing`:
- `analytics/tests/async_span_tests.rs` - All 3 tests use `InMemorySink`
- `analytics/tests/async_trait_tracing_test.rs` - Both tests use `InMemorySink`
- `tracing/tests/flush_monitor_safety.rs` - Uses `InMemorySink`
- `tracing/tests/thread_park_test.rs` - Uses `InMemorySink`
- `tracing/tests/test_macros.rs` - Uses `DebugEventSink` (custom in-memory sink)

### Tests Using Mixed Approaches ⚠️
These tests use `TelemetryGuardBuilder` to set up the full telemetry infrastructure, then bypass it with manual stream management:
- `analytics/tests/log_tests.rs` - 4 tests
- `analytics/tests/span_tests.rs` - 1 test  
- `analytics/tests/metrics_test.rs` - 3 tests

### Tests Using Pure In-Memory Data Structures ✅
These tests create data structures directly without any telemetry infrastructure:
- `analytics/tests/async_events_tests.rs` - All 13 tests
- Most tests in `transit/tests/` - Pure data structure tests

### Tests Using External Dependencies (Ignored) 🚫
These tests are marked with `#[ignore]` and can be excluded:
- `analytics/tests/sql_view_test.rs` - Uses `connect_to_data_lake`
- `analytics/tests/histo_view_test.rs` - Uses `connect_to_data_lake`

## Problems to Solve

### 1. Inconsistent Test Infrastructure
- Some tests use `TelemetryGuardBuilder` (full telemetry stack)
- Others use `init_event_dispatch` with `InMemorySink` (pure in-memory)
- Different approaches for the same goal (isolated unit testing)

### 2. Manual Stream Management + Full Telemetry Stack
Tests in `analytics/tests/log_tests.rs`, `span_tests.rs`, and `metrics_test.rs`:
- Set up full `TelemetryGuard` (including potential HTTP sinks and external dependencies)
- Then manually create streams (`LogStream::new`, `MetricsStream::new`, `ThreadStream::new`)
- Encode blocks with `block.encode_bin(&process_info)`  
- Parse blocks with `parse_block`
- Convert to data structures with `log_entry_from_value`, `measure_from_value`

This creates the worst of both worlds: full ingestion stack dependency + manual stream management that bypasses the telemetry infrastructure.

### 3. Test Concurrency Issues
`init_event_dispatch` uses global state that causes concurrency problems:
- Global singleton `G_DISPATCH` prevents parallel test execution
- Tests using `init_event_dispatch` must run with `#[serial]` 
- Requires explicit cleanup with `shutdown_dispatch()` and `unsafe { force_uninit() }`
- Mixed tests using `TelemetryGuardBuilder` may conflict with `init_event_dispatch`

### 4. Missing Helper Functions
No standardized way to:
- Initialize in-memory tracing across all test files
- Convert between telemetry events and in-memory data structures
- Verify in-memory sink contents
- Manage test concurrency and cleanup

## Implementation Plan

### Phase 1: Standardize In-Memory Test Infrastructure

#### 1.1 Create Common Test Utilities (`rust/tracing/src/test_utils.rs`)
```rust
use std::sync::Arc;
use std::collections::HashMap;
use crate::dispatch::{init_event_dispatch, shutdown_dispatch, force_uninit};
use crate::event::in_memory_sink::InMemorySink;

/// RAII guard for in-memory tracing that handles cleanup
pub struct InMemoryTracingGuard {
    pub sink: Arc<InMemorySink>,
}

impl InMemoryTracingGuard {
    pub fn new() -> Self {
        let sink = Arc::new(InMemorySink::new());
        init_event_dispatch(1024, 1024, 1024, sink.clone(), HashMap::new()).unwrap();
        Self { sink }
    }
}

impl Drop for InMemoryTracingGuard {
    fn drop(&mut self) {
        shutdown_dispatch();
        unsafe { force_uninit() };
    }
}

/// All tests using this MUST be marked with #[serial]
pub fn init_in_memory_tracing() -> InMemoryTracingGuard {
    InMemoryTracingGuard::new()
}

pub fn init_in_memory_tracing_with_tokio() -> (tokio::runtime::Runtime, InMemoryTracingGuard) {
    let guard = InMemoryTracingGuard::new();
    
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("test-runtime")
        .with_tracing_callbacks()
        .build()
        .unwrap();
    
    (runtime, guard)
}
```

#### 1.2 Enhance `InMemorySink` in `tracing/src/event/in_memory_sink.rs`
Current implementation has several `todo!()` methods. Complete implementation:
- `on_log()` - Store log events
- `on_process_log_block()` - Store log blocks  
- `on_process_metrics_block()` - Store metrics blocks
- `is_busy()` - Return false for tests
- Add helper methods to extract test data

### Phase 2: Convert Mixed Approach Tests

#### 2.1 Update `analytics/tests/log_tests.rs`
**Current pattern:**
```rust
let _telemetry_guard = TelemetryGuardBuilder::default().build();
// Manual stream creation and encoding - bypasses telemetry infrastructure
let mut stream = LogStream::new(1024, process_id, &[], HashMap::new());
// ... manual encoding/parsing
```

**Target pattern:**
```rust
use micromegas_tracing::test_utils::init_in_memory_tracing;
use serial_test::serial;

#[test]
#[serial]  // REQUIRED - tests share global state
fn test_name() {
    let guard = init_in_memory_tracing();  // RAII cleanup
    // Use actual tracing macros: info!(), debug!(), etc.
    // Verify results in guard.sink.state
    // Automatic cleanup via Drop
}
```

#### 2.2 Update `analytics/tests/metrics_test.rs`
All tests must be marked `#[serial]`:
```rust
use micromegas_tracing::test_utils::init_in_memory_tracing;
use serial_test::serial;

#[test]
#[serial]
fn test_static_metrics() {
    let guard = init_in_memory_tracing();
    // Use actual metric macros: imetric!(), fmetric!()
    // Verify results in guard.sink.state
}
```

#### 2.3 Update `analytics/tests/span_tests.rs`  
Must be marked `#[serial]`:
```rust
use micromegas_tracing::test_utils::init_in_memory_tracing;
use serial_test::serial;

#[test]
#[serial]
fn test_parse_span_interops() {
    let guard = init_in_memory_tracing();
    // Use actual span macros: span_scope!()
    // Verify results in guard.sink.state
}
```

### Phase 3: Standardize Existing In-Memory Tests

#### 3.1 Consolidate Helper Functions
Remove duplicate `init_in_mem_tracing` functions from:
- `analytics/tests/async_span_tests.rs`
- `analytics/tests/async_trait_tracing_test.rs`  
- `tracing/tests/thread_park_test.rs`

Replace with common utility.

#### 3.2 Update Import Patterns
Standardize imports across all test files to use common utilities.

### Phase 4: Verification and Cleanup

#### 4.1 Test Categories After Conversion
- **Serial In-Memory Tests**: Use `InMemorySink` + `#[serial]` + real tracing macros
- **Pure Data Structure Tests**: Direct data structure manipulation (fully parallel)
- **Integration Tests**: Marked `#[ignore]` (no change needed)

#### 4.2 Benefits Achieved
- **Consistent**: All non-ignored tests use same in-memory approach
- **Fast**: No external dependencies or HTTP calls
- **Reliable**: No race conditions with external services (except global state serialization)
- **Maintainable**: Single pattern for test infrastructure setup
- **Correct**: Proper cleanup prevents test interference

#### 4.3 Concurrency Trade-offs
- **Serial tests**: Tests using `init_event_dispatch` must run sequentially
- **Parallel tests**: Pure data structure tests can still run in parallel
- **Net improvement**: Fewer dependencies + reliable cleanup > concurrency loss

### Phase 5: Documentation

#### 5.1 Update Test Documentation
- Document when to use in-memory vs integration testing
- Provide examples of common test patterns
- Document helper functions

#### 5.2 Add CLAUDE.md Guidelines
```markdown
## Testing Guidelines
- Unit tests: use `micromegas_tracing::test_utils::init_in_memory_tracing()` + `#[serial]`
- Never use `TelemetryGuardBuilder` in unit tests
```

## Implementation Order

1. **Create common test utilities** - Foundation for all other changes
2. **Complete `InMemorySink` implementation** - Required for utilities to work
3. **Convert `log_tests.rs`** - Largest set of mixed tests
4. **Convert `metrics_test.rs`** - Similar pattern to log tests
5. **Convert `span_tests.rs`** - Complete the analytics test conversion
6. **Consolidate existing in-memory tests** - Remove duplication
7. **Verify all tests pass** - Ensure no regressions
8. **Update documentation** - Capture new patterns

## Success Criteria

- [ ] All non-ignored unit tests use `InMemorySink` or pure data structures
- [ ] No unit tests depend on external HTTP services or databases
- [ ] Tests using global state are properly serialized with `#[serial]`
- [ ] Automatic cleanup prevents test interference
- [ ] Test execution time improved (no network I/O)
- [ ] Single standardized pattern for test infrastructure setup
- [ ] Zero `TelemetryGuardBuilder` usage in unit tests (replaced with `InMemorySink`)

## Concurrency Impact

**Before**: Mixed test patterns with potential conflicts and no cleanup
**After**: 
- Serial in-memory tests: ~10 tests (reliable, no external deps)
- Parallel data structure tests: ~15 tests (unchanged)
- Net result: Better reliability, fewer dependencies, acceptable concurrency trade-off

## Files to Modify

### New Files
- `rust/tracing/src/test_utils.rs` - Test utilities

### Modified Files  
- `rust/tracing/src/lib.rs` - Export `test_utils` module
- `rust/tracing/src/event/in_memory_sink.rs` - Complete implementation
- `rust/analytics/tests/log_tests.rs` - Convert to in-memory
- `rust/analytics/tests/metrics_test.rs` - Convert to in-memory  
- `rust/analytics/tests/span_tests.rs` - Convert to in-memory
- `rust/analytics/tests/async_span_tests.rs` - Use common utilities
- `rust/analytics/tests/async_trait_tracing_test.rs` - Use common utilities
- `rust/tracing/tests/thread_park_test.rs` - Use common utilities
- `rust/tracing/tests/flush_monitor_safety.rs` - Use common utilities

This plan ensures all unit tests record data in memory while maintaining test coverage and improving reliability.