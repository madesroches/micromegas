# Plan: Async Trait Tracing Support - ROOT CAUSE CONFIRMED

## ✅ Root Cause Analysis - CONFIRMED

### Debug Evidence from Test Runs:
```bash
# Test output from test_comparison_async_trait_vs_regular:
Async trait result: processed: async_trait_test
🔵 DEBUG: on_begin_async_scope called for function: async_trait_tracing_test::regular_async_function
🔴 DEBUG: on_end_async_scope called for function: async_trait_tracing_test::regular_async_function
Regular async result: regular: regular_test

# Test output from test_simple_service_process_events:
📨 DEBUG: on_thread_event called with type_id: BeginThreadSpanEvent  <- ❌ WRONG for async trait
📨 DEBUG: on_thread_event called with type_id: EndThreadSpanEvent    <- ❌ WRONG for async trait
```

**Confirmed Root Cause**: The `#[span_fn]` macro incorrectly treats async trait methods as synchronous functions because `#[async_trait]` transforms the signature, removing the `async` keyword before `#[span_fn]` processes it.  

## Current Investigation Status

### ✅ Phase 1: Problem Identification - COMPLETE
- **Debug instrumentation** added to `dispatch.rs` to trace async span calls
- **Test suite** created in `rust/analytics/tests/async_trait_tracing_test.rs`
- **Root cause confirmed**: Async trait methods generate thread span events instead of async span events

### 🔧 Phase 2: Implementation - NEXT STEP
- **Task**: Add Future return type detection in proc macro
- **Location**: `rust/tracing/proc-macros/src/lib.rs:47`
- **Solution**: Check if return type is `Pin<Box<dyn Future>>` pattern
- **Code change needed**:
  ```rust
  // Current (line 47):
  if function.sig.asyncness.is_some() {
  
  // Should be:
  if function.sig.asyncness.is_some() || returns_future(&function) {
  ```

### 🔍 Technical Details of the Problem

**File**: `rust/tracing/proc-macros/src/lib.rs:47-65`

**Critical Finding**: When `#[span_fn]` processes async trait methods, it sees no `async` keyword (removed by `#[async_trait]`) and takes the sync path!

**What Actually Happens**:
```rust
// ❌ WRONG: async trait method gets ThreadSpanGuard (sync instrumentation)
impl SimpleService for SimpleServiceImpl {
    fn process(&self, input: &str) -> Pin<Box<dyn Future<Output = String> + Send + '_>> {
        static _METADATA_FUNC: SpanMetadata = /* ... */;
        let guard_named = ThreadSpanGuard::new(&_METADATA_FUNC);  // ❌ THREAD SPAN!
        Box::pin(async move {
            // async implementation...
        })
    }
}

// ✅ CORRECT: regular async function gets InstrumentedFuture (async instrumentation)  
async fn regular_async_function() -> String {
    static _SCOPE_DESC: SpanDesc = /* ... */;
    let fut = async move { /* ... */ };
    InstrumentedFuture::new(fut, &_SCOPE_DESC)  // ✅ ASYNC SPAN!
}
```

**The Problem**:
1. `#[async_trait]` transforms: `async fn method()` → `fn method() -> Pin<Box<dyn Future>>`
2. `#[span_fn]` sees `asyncness = None` (no async keyword)
3. Routes to sync path: creates `ThreadSpanGuard` instead of `InstrumentedFuture`
4. Result: Thread span events instead of async span events

**Detection Strategy**: Analyze return type for patterns like `Pin<Box<dyn Future>>` or `impl Future`.

### ❌ What's NOT Working
- **Async Trait Methods**: Generate `BeginThreadSpanEvent`/`EndThreadSpanEvent` instead of async span events
- **Wrong Instrumentation**: Missing `InstrumentedFuture` wrapper for async trait methods  
- **Async Span Functions**: `on_begin_async_scope`/`on_end_async_scope` never called for async trait methods
- **Async Context**: Lost async span context and proper async instrumentation flow

### ✅ What's Working  
- **Event Recording**: Events ARE recorded (tests pass) but with wrong type
- **Regular Async Functions**: Correctly use `on_begin_async_scope`/`on_end_async_scope`
- **Thread Spans**: Sync function instrumentation works perfectly
- **Compilation**: No build errors, silent incorrect behavior

### 🔍 Current Gap (Not Technical Limitation)
- **Outdated Documentation**: `rust/tracing/proc-macros/src/lib.rs:5` incorrectly states "async trait functions not supported"
- **Missing Integration**: Existing async trait implementations don't use `#[span_fn]` (likely due to the incorrect documentation)
- **No Test Coverage**: No existing tests for async trait tracing (until our new test)

## Test Results Summary

Our comprehensive test (`rust/analytics/tests/async_trait_tracing_test.rs`) proves:

```rust
#[async_trait]
impl SimpleService for SimpleServiceImpl {
    #[span_fn]  // ✅ WORKS!
    async fn process(&self, input: &str) -> String {
        format!("processed: {}", input)
    }
}

#[async_trait]
impl GenericService<String> for GenericServiceImpl {
    #[span_fn]  // ✅ WORKS!
    async fn handle(&self, item: String) -> String {
        format!("handled: {}", item)
    }
}

#[async_trait]
impl ComplexService for ComplexServiceImpl {
    #[span_fn]  // ✅ WORKS!
    async fn complex_method(&self, data: &[u8], options: HashMap<String, String>) -> Result<Vec<u8>, String> {
        Ok(data.to_vec())
    }
}
```

**All generate correct async span events!**

## Problem Analysis

### Why Async Traits Are Complex
The `#[async_trait]` macro from the `async-trait` crate transforms:

```rust
#[async_trait]
trait MyTrait {
    async fn method(&self) -> Result<()>;
}
```

Into:
```rust
trait MyTrait {
    fn method(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}
```

### Challenges for `#[span_fn]`
1. **Signature Transformation**: The async signature is completely rewritten
2. **Return Type Boxing**: The return type becomes a boxed future
3. **Lifetime Management**: Complex lifetime parameters are introduced
4. **Ordering Dependencies**: `#[span_fn]` must work correctly with `#[async_trait]` transformations

## Implementation Strategy - REFINED

### Phase 1: ✅ Detection and Analysis - COMPLETE

Test suite already created and confirms the issue:
- `rust/analytics/tests/async_trait_tracing_test.rs` 
- Tests show async trait methods generate thread span events (wrong!)
- Regular async functions correctly generate async span events (correct!)

### Phase 2: 🚀 Core Implementation - IN PROGRESS

#### 2.1 ✅ Add Future Return Type Detection - COMPLETE
Added helper functions to detect Future return types in proc macro.

#### 2.2 🔄 New Approach: Separate `span_async_trait` Macro

**Current Implementation**:

```rust
/// New macro specifically for async trait methods
#[proc_macro_attribute]
pub fn span_async_trait(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as TraceArgs);
    let mut function = parse_macro_input!(input as ItemFn);

    let function_name = args
        .alternative_name
        .map_or(function.sig.ident.to_string(), |n| n.to_string());

    // For async trait methods, we keep the async keyword and wrap the body
    // async-trait will handle the transformation to Pin<Box<dyn Future>>
    if function.sig.asyncness.is_some() {
        let original_block = &function.block;
        
        // Keep the function async, just wrap the body with instrumentation
        function.block = parse_quote! {
            {
                static_span_desc!(_SCOPE_DESC, concat!(module_path!(), "::", #function_name));
                let fut = async move #original_block;
                InstrumentedFuture::new(fut, &_SCOPE_DESC).await
            }
        };
    }

    TokenStream::from(quote! { #function })
}
```

**Current Implementation Status**:
```rust
/// span_async_trait: trace the execution of an async trait method  
/// This macro is applied BEFORE #[async_trait] transforms the method
#[proc_macro_attribute]
pub fn span_async_trait(args: TokenStream, input: TokenStream) -> TokenStream {
    // ... 
    if function.sig.asyncness.is_some() {
        // This branch never executes because async-trait transforms first
    } else {
        // This is where we actually end up - handling transformed methods
        if returns_future(&function) {
            // Insert instrumentation at the beginning of the function body
            function.block.stmts.insert(0, parse_quote! {
                static_span_desc!(_SCOPE_DESC, concat!(module_path!(), "::", #function_name));
                let _span_guard = micromegas_tracing::spans::guards::AsyncSpanGuard::new(&_SCOPE_DESC);
            });
        }
    }
}
```

**Final Implementation**:
```rust
/// span_async_trait: trace the execution of an async trait method
#[proc_macro_attribute]
pub fn span_async_trait(args: TokenStream, input: TokenStream) -> TokenStream {
    // ... parse args and function ...
    
    if returns_future(&function) {
        let stmts = &function.block.stmts;
        
        // Extract async block from Box::pin(async move { ... })
        if stmts.len() == 1 {
            if let syn::Stmt::Expr(syn::Expr::Call(call_expr)) = &stmts[0] {
                if call_expr.args.len() == 1 {
                    let async_block = &call_expr.args[0];
                    
                    // Replace with instrumented version
                    function.block = parse_quote! {
                        {
                            static_span_desc!(_SCOPE_DESC, concat!(module_path!(), "::", #function_name));
                            Box::pin(InstrumentedFuture::new(
                                #async_block,
                                &_SCOPE_DESC
                            ))
                        }
                    };
                }
            }
        }
    }
    TokenStream::from(quote! { #function })
}
```

**✅ SUCCESS**: Generates proper async span events identical to regular async functions!

### ✅ Phase 3: Testing and Validation - COMPLETE

#### 3.1 ✅ Test Results - SUCCESS
Running the test suite confirms the implementation works:

```bash
cargo test --package micromegas-analytics test_comparison_async_trait_vs_regular -- --nocapture
```

**Actual Output**:
```
🔵 DEBUG: on_begin_async_scope called for function: async_trait_tracing_test::process
🔵 DEBUG: Generated span_id: 1, parent_span_id: 0
🔵 DEBUG: BeginAsyncSpanEvent sent to thread event queue
🔴 DEBUG: on_end_async_scope called for function: async_trait_tracing_test::process, span_id: 1
🔴 DEBUG: EndAsyncSpanEvent sent to thread event queue
🔵 DEBUG: on_begin_async_scope called for function: async_trait_tracing_test::regular_async_function
🔵 DEBUG: Generated span_id: 2, parent_span_id: 0
🔵 DEBUG: BeginAsyncSpanEvent sent to thread event queue
🔴 DEBUG: on_end_async_scope called for function: async_trait_tracing_test::regular_async_function, span_id: 2
🔴 DEBUG: EndAsyncSpanEvent sent to thread event queue
Total events from both calls: 4
✓ PERFECT: Both async trait method and regular async function generated identical event counts!
```

**All Test Cases Pass**:
- ✅ `test_comparison_async_trait_vs_regular`: 4 events (2 per async function)
- ✅ `test_simple_service_process_events`: 2 events from async trait method
- ✅ `test_async_trait_span_fn_comprehensive`: 10 events across all async trait variations

### ✅ Phase 4: Integration and Usage - READY

Async trait methods throughout the codebase can now use `#[span_async_trait]`:

```rust
// Example: rust/analytics/src/record_batch_transformer.rs
#[async_trait]
impl RecordBatchTransformer for TrivialRecordBatchTransformer {
    #[span_async_trait]  // ✅ Now generates proper async span events!
    async fn transform(&self, src: RecordBatch) -> Result<RecordBatch> {
        Ok(src)
    }
}

// All async trait implementations can now be instrumented:
#[async_trait]
impl QueryPartitionProvider for LivePartitionProvider {
    #[span_async_trait]
    async fn fetch(&self, ...) -> Result<Vec<Partition>> {
        // Automatically traced with async span events
    }
}
```

**Usage Pattern**:
1. Apply `#[async_trait]` to impl block (as before)
2. Add `#[span_async_trait]` to individual async methods 
3. Async span events are automatically generated

### Phase 4: Documentation and Guidelines

#### 4.1 Update AI Guidelines
**Location**: `AI_GUIDELINES.md`

Add async trait tracing guidelines:

```markdown
### Async Trait Tracing
- Use `#[span_fn]` on async trait method implementations for automatic tracing
- Apply `#[async_trait]` first, then `#[span_fn]` on individual methods
- Async trait methods generate the same span events as regular async functions
```

#### 4.2 Developer Documentation
**Location**: `rust/tracing/README.md`

Document async trait support:

```markdown
## Async Trait Support

The `#[span_fn]` macro now supports async trait methods:

\`\`\`rust
use async_trait::async_trait;
use micromegas_tracing::prelude::*;

#[async_trait]
trait MyService {
    async fn process(&self, data: String) -> Result<String>;
}

#[async_trait]
impl MyService for MyServiceImpl {
    #[span_fn]
    async fn process(&self, data: String) -> Result<String> {
        // Automatically traced async trait method
        Ok(data.to_uppercase())
    }
}
\`\`\`
```

## Implementation Steps

### ✅ Implementation Complete
1. ✅ **DONE**: Identify and confirm root cause
2. ✅ **DONE**: Create test suite demonstrating the issue
3. ✅ **DONE**: Implement Future return type detection in proc macro
4. ✅ **DONE**: Create separate `span_async_trait` macro for cleaner implementation
5. ✅ **DONE**: Test the new macro with async trait methods
6. ✅ **DONE**: Successfully implement async trait tracing support
7. ✅ **DONE**: Merge functionality back into unified `span_fn` macro
8. ✅ **DONE**: Update documentation and remove outdated comments
9. ✅ **DONE**: Re-enable async trait instrumentation in lakehouse processors
10. ✅ **DONE**: Enhance test suite with `HeterogeneousQueue` event type validation
11. ✅ **DONE**: Simplify and consolidate redundant tests for maintainability

### ✅ Investigation Complete: Macro Ordering Issue Identified

**Key Discovery**: `#[async_trait]` on impl blocks processes ALL methods inside, including those with our attributes, BEFORE our macro gets to run.

**What Actually Happens**:
1. `#[async_trait]` is applied to the impl block
2. It transforms ALL methods inside from `async fn` to `fn -> Pin<Box<dyn Future>>`
3. THEN our `#[span_async_trait]` macro runs on the already-transformed methods
4. By this time, `function.sig.asyncness.is_none()` and the method returns `Pin<Box<dyn Future>>`

**Evidence from Debug Output**:
```
🔄 SPAN_ASYNC_TRAIT: Handling transformed method process
🔄 SPAN_ASYNC_TRAIT: Handling transformed method transform  
🔄 SPAN_ASYNC_TRAIT: Handling transformed method handle
🔄 SPAN_ASYNC_TRAIT: Handling transformed method complex_method
```

**Challenges Encountered**:
1. **Double-boxing**: When we try to wrap the transformed body with `InstrumentedFuture`, we create nested `Box::pin()` calls
2. **Context mismatch**: `AsyncSpanGuard` needs to run inside async context, but we're in sync context that returns Future
3. **Body complexity**: The transformed method body contains `Box::pin(async move { ... })` that we need to intercept

## ✅ Success Criteria - ALL ACHIEVED

**Test Results Confirm Success**:
- ✅ Async trait methods generate `BeginAsyncSpanEvent`/`EndAsyncSpanEvent` 
- ✅ Debug output shows `on_begin_async_scope`/`on_end_async_scope` calls
- ✅ Event counts match between async trait methods and regular async functions
- ✅ No more thread span events for async methods
- ✅ All async trait variations supported (simple, generic, complex signatures)

## ✅ Key Insights - Lessons Learned

1. **Macro ordering solved**: `#[async_trait]` transforms methods before our macro runs, but we can work with the transformed result
2. **AST parsing successful**: Extract `async move` blocks from `Box::pin(async move { ... })` calls  
3. **InstrumentedFuture integration**: Wrapping extracted async blocks with `InstrumentedFuture` works perfectly
4. **Pattern matching approach**: Use syn AST pattern matching to safely extract async blocks
5. **Test-driven development**: Comprehensive test suite enabled rapid iteration and validation

## ✅ Solution Achieved

**Successful Implementation**: The `span_async_trait` macro successfully:
1. **Detects transformed methods**: Uses `returns_future()` to identify async trait methods
2. **Extracts async blocks**: Parses `Box::pin(async move { ... })` and extracts the inner async block
3. **Instruments correctly**: Wraps with `InstrumentedFuture` for proper async span tracing
4. **Maintains compatibility**: Works seamlessly with existing async-trait usage patterns

## ✅ Summary - COMPLETE SUCCESS

**Problem Solved**: Async trait methods now generate proper async span events identical to regular async functions.

**Root Cause Identified**: `#[async_trait]` processes impl blocks before our macros run, transforming `async fn` to `fn -> Pin<Box<dyn Future>>`.

**Solution Implemented**: The `span_async_trait` macro successfully:
- Works with transformed async trait methods
- Extracts async blocks from `Box::pin(async move { ... })` calls  
- Wraps them with `InstrumentedFuture` for proper async span instrumentation
- Generates identical async span events to regular async functions

**Status**: ✅ **COMPLETE** - Async trait tracing support is fully implemented, tested, and merged into the unified `span_fn` macro.

**Final Implementation**: The unified `span_fn` macro now automatically handles all function types:

### ✅ Unified span_fn Macro - FINAL SOLUTION

**Detection Logic** (applied in order):
1. **`returns_future(&function)`** → Async trait method (after `#[async_trait]` transformation)
   - Extracts async blocks from `Box::pin(async move { ... })`
   - Wraps with `InstrumentedFuture` for proper async span events
2. **`function.sig.asyncness.is_some()`** → Regular async function
   - Removes `async` keyword, changes return type to `impl Future`
   - Wraps with `InstrumentedFuture`
3. **Neither** → Sync function
   - Adds `span_scope!` instrumentation

**Usage**: Simply apply `#[span_fn]` to any function type:

```rust
// Regular async function
#[span_fn]
async fn regular_function() -> String { ... }

// Async trait methods  
#[async_trait]
impl MyTrait for MyImpl {
    #[span_fn]  // ✅ Now works perfectly!
    async fn trait_method(&self) -> String { ... }
}

// Sync function
#[span_fn]
fn sync_function() -> String { ... }
```

**Key Benefits**:
- **Single macro**: Users only need `#[span_fn]` for all function types
- **Automatic detection**: No need to choose between different macros
- **Complete coverage**: Sync functions, async functions, and async trait methods all generate proper span events
- **Unambiguous logic**: Macro ordering makes detection reliable and deterministic

**Test Results**: All function types generate identical, correct span events with robust event type validation.

### ✅ Final Test Suite - SIMPLIFIED AND ENHANCED

**Removed Redundancy**: Consolidated 8 overlapping tests into 2 focused, robust tests that provide superior validation:

#### Test 1: `test_async_trait_comprehensive` 
- **Coverage**: All async trait variations (simple, generic, complex) + sync/async controls
- **Validation**: 12 total events (2 sync thread events + 10 async span events) 
- **Technology**: Uses `HeterogeneousQueue::iter()` + pattern matching on `ThreadEventQueueAny` variants
- **Purpose**: Comprehensive functionality + event type validation in one test

#### Test 2: `test_async_trait_equivalence`
- **Coverage**: Direct comparison between async trait methods and regular async functions
- **Validation**: 4 async span events, 0 sync thread events
- **Purpose**: Proves async trait methods behave identically to regular async functions

**Key Enhancement**: Event type inspection using the `HeterogeneousQueue` interface to definitively validate that:
- Sync functions generate `BeginThreadSpanEvent`/`EndThreadSpanEvent` 
- Async trait methods generate `BeginAsyncSpanEvent`/`EndAsyncSpanEvent` (not sync events)
- Regular async functions generate `BeginAsyncSpanEvent`/`EndAsyncSpanEvent`

This provides **proof** that the unified `span_fn` macro correctly distinguishes between function types and generates appropriate event types.
