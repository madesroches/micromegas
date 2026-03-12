# Capture Async Span Depth at Creation Time Plan

## Overview

The `depth` field in async span events is computed at poll time from the thread-local `ASYNC_CALL_STACK`, but it should be captured once at future creation time. This causes begin/end events for the same span to report different depths depending on which thread/context polls them, making the depth field unreliable for flame graph rendering.

GitHub issue: #927

## Current State

In `rust/tracing/src/spans/instrumented_future.rs`, both `InstrumentedFuture` and `InstrumentedNamedFuture` already capture `parent` (parent span ID) at creation time in their `new()` constructors (lines 136-140, 205-209). However, `depth` is computed fresh on every `poll()` call:

```rust
// Line 162 (InstrumentedFuture::poll)
let depth = (stack.len().saturating_sub(1)) as u32;

// Line 232 (InstrumentedNamedFuture::poll)
let depth = (stack.len().saturating_sub(1)) as u32;
```

Since `ASYNC_CALL_STACK` is thread-local and async futures can be polled on different threads or at different stack depths, the `depth` value varies between the begin event (first poll) and end event (final poll). The `parent_span_id` is already correctly captured at creation time, so depth should follow the same pattern for consistency.

## Design

Add a `depth: u32` field to both `InstrumentedFuture` and `InstrumentedNamedFuture` structs, computed once in `new()` alongside `parent`, then used in `poll()` instead of recomputing from the stack.

## Implementation Steps

All changes are in `rust/tracing/src/spans/instrumented_future.rs`:

### 1. Add `depth` field to `InstrumentedFuture` struct (line 124-131)

Add `depth: u32` to the struct definition.

### 2. Capture depth in `InstrumentedFuture::new()` (lines 135-147)

Compute `depth` from `(stack.len().saturating_sub(1)) as u32` in the same closure that captures `parent`, and store it in the struct. Use `saturating_sub` to match the defensive style of the existing poll code.

### 3. Use stored depth in `InstrumentedFuture::poll()` (lines 156-187)

Replace `let depth = (stack.len().saturating_sub(1)) as u32;` on line 162 with `let depth = *this.depth;`.

### 4. Add `depth` field to `InstrumentedNamedFuture` struct (lines 192-200)

Add `depth: u32` to the struct definition.

### 5. Capture depth in `InstrumentedNamedFuture::new()` (lines 202-217)

Compute `depth` from `(stack.len().saturating_sub(1)) as u32` in the same closure that captures `parent`, and store it in the struct. Use `saturating_sub` to match the defensive style of the existing poll code.

### 6. Use stored depth in `InstrumentedNamedFuture::poll()` (lines 226-264)

Replace `let depth = (stack.len().saturating_sub(1)) as u32;` on line 232 with `let depth = *this.depth;`.

## Files to Modify

- `rust/tracing/src/spans/instrumented_future.rs` — the only file that needs changes

### 7. Add depth consistency test to `rust/tracing/tests/async_depth_tracking_tests.rs`

The existing tests are smoke tests that only verify instrumentation runs without panicking — they don't assert depth values. Add a test that uses an in-memory event sink to capture async span events and verifies that begin/end events for the same `span_id` report the same `depth` value, and that nested spans have increasing depth.

## Testing Strategy

- Run `cargo test` from `rust/` to verify existing tests pass
- Run `cargo clippy --workspace -- -D warnings` to verify no warnings
- The new depth consistency test (step 7) validates that begin/end events for the same span report identical depth values after the fix

## Open Questions

None — the fix is straightforward and mirrors how `parent` is already handled.
