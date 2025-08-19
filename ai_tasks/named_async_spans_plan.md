# Named Async Span Event Tracking Plan

## Problem Statement
When manually instrumenting async code (not using `#[span_fn]`), users currently can only create async spans with static names. We need to support variable/dynamic names for manual async span instrumentation.

## Current Implementation Analysis
- **`#[span_fn]` macro**: Works correctly - generates static spans with function names (no change needed)
- **Manual async instrumentation**: Currently only supports static `SpanMetadata` descriptors
- **EXISTING INFRASTRUCTURE**: Named async span events already exist but aren't exposed!
  - `BeginAsyncNamedSpanEvent` and `EndAsyncNamedSpanEvent` structs exist
  - Use `SpanLocation` (just location info) instead of `SpanMetadata` (location + static name)  
  - Use `StringId` for dynamic names (pointer + length to string data)
  - Functions `on_begin_async_named_scope()` and `on_end_async_named_scope()` exist in dispatch.rs
- Thread named spans already work as a reference implementation

## Proposed Solution

**Key Insight**: The named async span infrastructure already exists! We just need to expose it through a public API for manual instrumentation.

### Phase 1: Create Manual Named Async Span API
1. **Find current manual async instrumentation patterns**
   - Look for existing manual async span usage
   - Understand how manual instrumentation currently works
   
2. **Create named async span API**
   - `span_async_named(name: &str, async_block)` - for manual named spans
   - Use existing `on_begin_async_named_scope()` and `on_end_async_named_scope()` functions
   - Create convenience wrapper similar to thread named spans

### Phase 2: Testing & Documentation
1. **Test coverage**
   - Unit tests for dynamic name creation
   - Integration tests for end-to-end flow
   - Performance benchmarks comparing static vs named async spans

2. **Documentation**
   - Update API documentation
   - Add examples for common use cases

## Implementation Order
1. **Find current manual async instrumentation patterns** - understand existing usage
2. **Create manual named async span API** - expose existing infrastructure
3. **Testing and documentation**

## Key Considerations
- **Performance**: Named spans already exist, so no new overhead
- **Compatibility**: Existing code continues to work unchanged
- **API Design**: Follow thread named span patterns for consistency
- **String Management**: Use `StringId` for efficient string handling

## Success Criteria  
- [ ] Manual API: `span_async_named(name, async_block)` works for manual instrumentation
- [ ] Async spans show unique names per invocation in analytics
- [ ] No performance regression for existing static spans or `#[span_fn]` 
- [ ] Clean, consistent API following existing thread named span patterns

## Example Usage (Target API)
```rust
// Manual named async instrumentation - new API
span_async_named("process_user_123", async {
    // async work with dynamic name
});

// Static function spans continue to work unchanged (no changes needed)
#[span_fn]
async fn process_user(user_id: &str) {
    // span name is "process_user" (static function name)
}

// Use case: dynamic names for different operations
for user_id in user_ids {
    span_async_named(&format!("process_user_{}", user_id), async move {
        // each user gets uniquely named span
    }).await;
}
```

## Existing Infrastructure to Leverage
- `BeginAsyncNamedSpanEvent` / `EndAsyncNamedSpanEvent` structs ✅
- `on_begin_async_named_scope()` / `on_end_async_named_scope()` functions ✅  
- `StringId` for efficient string handling ✅
- Thread named span pattern to follow ✅
- Event queue and serialization already handle named events ✅

## Risks & Mitigations
- **Risk**: String lifetime management in async contexts
  - **Mitigation**: Use `StringId` pattern like thread spans, ensure strings live long enough
  
- **Risk**: Macro complexity increases  
  - **Mitigation**: Start with manual API, add macro features incrementally

## Next Steps
1. Review and refine this plan
2. Find current manual async instrumentation patterns (if any exist)
3. Create basic manual `span_async_named()` API using existing infrastructure
4. Test with simple examples