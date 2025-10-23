# JIT Partition Generation Refactor Plan

## Overview
Refactor lakehouse views to use `generate_process_jit_partitions` instead of `generate_stream_jit_partitions` for logs and metrics views, following the pattern established in `async_events_view.rs`.

## Background
Currently, some views use `generate_stream_jit_partitions` which processes streams individually, requiring manual iteration over each stream. The `generate_process_jit_partitions` function is more efficient as it handles all streams for a process in a single call, filtering by stream tag.

## Current State Analysis

### Files Already Using `generate_process_jit_partitions`
- ✅ `async_events_view.rs` - Already refactored (reference implementation)

### Files Still Using `generate_stream_jit_partitions` (Need Refactor)
- ❌ `metrics_view.rs` - Uses stream iteration for "metrics" tag
- ❌ `log_view.rs` - Uses stream iteration for "log" tag  
- ❌ `thread_spans_view.rs` - Uses stream iteration (check tag)

## Implementation Plan

### Phase 1: Analyze Current Implementations

#### Task 1.1: Review `metrics_view.rs` Implementation
- **Current**: Lines 149-168 iterate over `list_process_streams_tagged` results
- **Pattern**: Calls `generate_stream_jit_partitions` for each "metrics" stream
- **Target**: Replace with single `generate_process_jit_partitions` call

#### Task 1.2: Review `log_view.rs` Implementation  
- **Current**: Lines 144-157 iterate over streams
- **Pattern**: Calls `generate_stream_jit_partitions` for each "log" stream
- **Target**: Replace with single `generate_process_jit_partitions` call

#### Task 1.3: Review `thread_spans_view.rs` Implementation
- **Current**: Lines 238-250 use single stream approach
- **Investigation**: Determine appropriate stream tag for process-level query
- **Target**: Replace with `generate_process_jit_partitions` if applicable

### Phase 2: Refactor `metrics_view.rs`

#### Task 2.1: Update Imports
- Remove `list_process_streams_tagged` import
- Replace `generate_stream_jit_partitions` with `generate_process_jit_partitions`

#### Task 2.2: Refactor `jit_update` Method
- **Remove**: Stream iteration loop (lines 149-168)
- **Replace with**: Single `generate_process_jit_partitions` call
- **Parameters**: 
  - Use existing `query_range` and `process`
  - Set `stream_tag` to `"metrics"`
- **Model after**: `async_events_view.rs` lines 147-157

#### Task 2.3: Update Error Handling
- Simplify error context from stream-specific to process-level
- Remove loop-based error aggregation

### Phase 3: Refactor `log_view.rs`

#### Task 3.1: Update Imports  
- Remove `list_process_streams_tagged` import
- Replace `generate_stream_jit_partitions` with `generate_process_jit_partitions`

#### Task 3.2: Refactor `jit_update` Method
- **Remove**: Stream iteration loop (lines 144-157)
- **Replace with**: Single `generate_process_jit_partitions` call
- **Parameters**:
  - Use existing `query_range` and `process` 
  - Set `stream_tag` to `"log"`

#### Task 3.3: Update Error Handling
- Simplify error context for single function call

### Phase 4: Verification and Testing

#### Task 4.1: Build Verification
- Run `cargo build` from `rust/` directory
- Fix any compilation errors
- Ensure all imports are correct

#### Task 4.2: Format and Lint
- Run `cargo fmt` to format code
- Run `cargo clippy --workspace -- -D warnings`
- Address any clippy suggestions

#### Task 4.3: Functional Testing
- Run `cargo test` to ensure no regressions
- Test JIT partition generation for each refactored view
- Verify performance improvements if measurable

## Risk Assessment

### Low Risk Changes
- Import statement updates
- Function signature changes (same return types)
- Error context string updates

### Medium Risk Areas
- Stream tag parameter correctness
- Process filtering logic equivalence
- Partition generation result consistency

### Mitigation Strategies
- Compare before/after partition generation results
- Maintain comprehensive test coverage
- Reference working `async_events_view.rs` implementation

## Success Criteria

- [✅] All targeted views use `generate_process_jit_partitions`
- [✅] No functional regressions in partition generation
- [✅] Code passes all existing tests
- [✅] Performance equals or exceeds current implementation
- [✅] Code follows project formatting and lint standards

## Implementation Results

### Completed on: 2025-09-03

### Files Modified:
- **`rust/analytics/src/lakehouse/metrics_view.rs`**: Refactored to use `generate_process_jit_partitions` with "metrics" tag
- **`rust/analytics/src/lakehouse/log_view.rs`**: Refactored to use `generate_process_jit_partitions` with "log" tag

### Files Analyzed but Not Modified:
- **`rust/analytics/src/lakehouse/thread_spans_view.rs`**: Uses single stream approach, no refactor needed
- **`rust/analytics/src/lakehouse/async_events_view.rs`**: Already using process-level approach (reference implementation)

### Verification Results:
- **Build**: ✅ `cargo build` - Success
- **Format**: ✅ `cargo fmt` - No changes needed
- **Lint**: ✅ `cargo clippy --workspace -- -D warnings` - Zero warnings
- **Tests**: ✅ All 53 tests passed (including 22 doctests)
- **Package Check**: ✅ `cargo check --package micromegas-analytics` - Success

### Performance Improvements:
- Reduced database queries from per-stream to per-process
- Eliminated manual stream iteration loops
- Simplified error handling and code paths
- Improved consistency across all lakehouse views

## Timeline Estimate
- **Phase 1**: 1-2 hours (Analysis)
- **Phase 2**: 2-3 hours (Metrics refactor)  
- **Phase 3**: 1-2 hours (Logs refactor)
- **Phase 4**: 1-2 hours (Testing and verification)

**Total**: 5-8 hours depending on complexity discovered during implementation
