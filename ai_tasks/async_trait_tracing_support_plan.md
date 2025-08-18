# Plan: Async Trait Tracing Support - CRITICAL DISCOVERY

## 🔍 CRITICAL DISCOVERY: Async Trait Methods Use Thread Spans Instead of Async Spans

**Debug instrumentation revealed the REAL issue: Async trait methods are instrumented as THREAD spans, not ASYNC spans!**

### Key Findings from Debug Output:
- ✅ **Events ARE recorded** - tests pass because events exist
- ❌ **Wrong event type** - async trait methods generate `BeginThreadSpanEvent`/`EndThreadSpanEvent` 
- ❌ **Missing async span calls** - no `on_begin_async_scope`/`on_end_async_scope` calls
- ✅ **Regular async functions** - correctly call async span functions

### Debug Evidence:
```
🔵 on_begin_async_scope called: regular_async_function  <- ✅ Correct for regular async
🔴 on_end_async_scope called: regular_async_function    <- ✅ Correct for regular async
Processing thread event: BeginThreadSpanEvent          <- ❌ Wrong for async trait methods!
Processing thread event: EndThreadSpanEvent            <- ❌ Wrong for async trait methods!
```

**Root Cause**: The `#[span_fn]` macro incorrectly treats async trait methods as synchronous functions due to `#[async_trait]` transformation.  

## Current Investigation Status

### ✅ Phase 1: Problem Identification - COMPLETE
- **Debug instrumentation** added to `dispatch.rs` 
- **Root cause identified**: Async trait methods treated as sync functions
- **Issue confirmed**: Wrong span type used (thread vs async)

### 🔄 Phase 2: Proc Macro Analysis - COMPLETE
- ✅ **Root cause identified**: `function.sig.asyncness.is_some()` check fails for async trait methods
- ✅ **Mechanism understood**: `#[async_trait]` removes `async` keyword, returns `Pin<Box<Future>>`
- ✅ **Detection strategy**: Need to analyze return type for Future patterns
- ✅ **Code location**: `rust/tracing/proc-macros/src/lib.rs:45-60`

### ⏳ Phase 3: Implementation Design - COMPLETE
- ✅ **Pattern identified**: Need to detect `Pin<Box<dyn Future>>` return types
- ✅ **Strategy confirmed**: Parse return type syntax tree for Future patterns
- ✅ **Target behavior**: Route Future-returning functions to async instrumentation
- ✅ **Implementation plan**: Modify conditional logic in `span_fn` macro

### 🔄 Phase 4: Implementation - IN PROGRESS
- **Task**: Implement Future return type detection in proc macro
- **Location**: `rust/tracing/proc-macros/src/lib.rs:45`
- **Logic**: `if is_async_function(&function) || returns_future(&function)`

### ⏳ Phase 5: Testing - PENDING
- **Task**: Modify proc macro to detect async trait signatures
- **Requirement**: Generate async span events for async trait methods
- **Constraint**: Maintain performance and compatibility

### 🔍 Root Cause Confirmed by Macro Expansion

**File**: `rust/tracing/proc-macros/src/lib.rs:45-60`

**Critical Finding**: Macro expansion reveals that `#[span_fn]` on async trait methods generates **ThreadSpanGuard** instead of **InstrumentedFuture**!

**Expanded Code Shows the Bug**:
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

## Implementation Strategy

### Phase 1: Detection and Compatibility Analysis

#### 1.0 Problem Demonstration Test
**Location**: `rust/tracing/tests/async_trait_tracing_test.rs` (new file)

Create the simplest possible test that demonstrates the current limitation:

```rust
// This test should either:
// 1. Fail to compile with a clear error message, OR
// 2. Compile but show that no span events are generated

use async_trait::async_trait;
use micromegas_tracing::prelude::*;

#[async_trait]
trait SimpleService {
    async fn process(&self, input: &str) -> String;
}

struct SimpleServiceImpl;

#[async_trait]
impl SimpleService for SimpleServiceImpl {
    #[span_fn]  // This should currently fail or not work properly
    async fn process(&self, input: &str) -> String {
        format!("processed: {}", input)
    }
}

#[tokio::test]
async fn test_async_trait_span_fn_limitation() {
    let service = SimpleServiceImpl;
    let result = service.process("test").await;
    assert_eq!(result, "processed: test");
    
    // TODO: Add assertion that span events were/were not generated
    // This will help us measure success when the feature is implemented
}
```

**Success Criteria for this test**:
- Documents the exact current behavior (compilation error or missing spans)
- Provides baseline for measuring implementation success
- Uses minimal dependencies and simplest possible async trait

#### 1.1 Async Trait Detection
**Note**: Like the other async tracing unit tests, we'll record events in memory and validate their presence.

**Location**: `rust/tracing/proc-macros/src/lib.rs`

Add detection logic to identify when `#[span_fn]` is applied to a function that will be processed by `#[async_trait]`:

```rust
fn is_async_trait_method(function: &ItemFn) -> bool {
    // Check for async trait context indicators:
    // 1. Function is async
    // 2. Has `self` parameter (method)
    // 3. No function body generics that would indicate standalone function
    function.sig.asyncness.is_some() 
        && has_self_parameter(&function.sig)
        && !has_standalone_async_indicators(function)
}

fn has_self_parameter(sig: &Signature) -> bool {
    sig.inputs.iter().any(|input| {
        matches!(input, FnArg::Receiver(_))
    })
}
```

#### 1.2 Macro Ordering Strategy
Research and implement proper macro expansion ordering:

- **Option A**: Make `#[span_fn]` async-trait-aware and handle the transformation
- **Option B**: Require specific ordering: `#[async_trait]` first, then `#[span_fn]`
- **Option C**: Create a new combined macro `#[async_trait_span]`

### Phase 2: Core Implementation

#### 2.1 Enhanced Proc Macro Logic
**Location**: `rust/tracing/proc-macros/src/lib.rs`

Extend the `span_fn` macro to handle async trait methods:

```rust
pub fn span_fn(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as TraceArgs);
    let mut function = parse_macro_input!(input as ItemFn);

    let function_name = get_function_name(&args, &function);

    if function.sig.asyncness.is_some() {
        if is_async_trait_method(&function) {
            // New: Handle async trait methods
            handle_async_trait_method(function, function_name)
        } else {
            // Existing: Handle regular async functions
            handle_regular_async_function(function, function_name)
        }
    } else {
        // Existing: Handle sync functions
        handle_sync_function(function, function_name)
    }
}

fn handle_async_trait_method(mut function: ItemFn, function_name: String) -> TokenStream {
    let original_block = &function.block;
    
    // Keep the async signature intact for async_trait compatibility
    function.block = parse_quote! {
        {
            static_span_desc!(_SCOPE_DESC, concat!(module_path!(), "::", #function_name));
            let fut = async move #original_block;
            InstrumentedFuture::new(fut, &_SCOPE_DESC)
        }
    };

    TokenStream::from(quote! { #function })
}
```

#### 2.2 Future Instrumentation Enhancement
**Location**: `rust/tracing/src/async_instrumentation.rs` (new file)

Ensure `InstrumentedFuture` works correctly with boxed futures from async traits:

```rust
use std::pin::Pin;
use std::future::Future;
use std::task::{Context, Poll};

pub struct InstrumentedFuture<F> {
    future: F,
    span_desc: &'static SpanMetadata,
    span_guard: Option<AsyncSpanGuard>,
}

impl<F> InstrumentedFuture<F> {
    pub fn new(future: F, span_desc: &'static SpanMetadata) -> Self {
        Self {
            future,
            span_desc,
            span_guard: None,
        }
    }
}

impl<F> Future for InstrumentedFuture<F>
where
    F: Future,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        
        // Create span guard on first poll
        if this.span_guard.is_none() {
            this.span_guard = Some(AsyncSpanGuard::new(this.span_desc));
        }

        // Poll the underlying future
        let future = unsafe { Pin::new_unchecked(&mut this.future) };
        future.poll(cx)
    }
}
```

### Phase 3: Integration and Testing

#### 3.1 Update Existing Async Traits
**Locations**: Multiple files using `#[async_trait]`

Add `#[span_fn]` to key async trait implementations:

```rust
// rust/analytics/src/record_batch_transformer.rs
#[async_trait]
impl RecordBatchTransformer for TrivialRecordBatchTransformer {
    #[span_fn]
    async fn transform(&self, src: RecordBatch) -> Result<RecordBatch> {
        Ok(src)
    }
}

// rust/analytics/src/lakehouse/view.rs  
#[async_trait]
impl PartitionSpec for SomePartition {
    #[span_fn]
    async fn write(&self, lake: Arc<DataLakeConnection>, logger: Arc<dyn Logger>) -> Result<()> {
        // Implementation with automatic tracing
    }
}
```

#### 3.2 Comprehensive Testing
**Location**: `rust/tracing/proc-macros/tests/` (new directory)

Create test suite for async trait tracing:

```rust
// tests/async_trait_tests.rs
use async_trait::async_trait;
use micromegas_tracing::prelude::*;

#[async_trait]
trait TestTrait {
    async fn test_method(&self) -> String;
}

struct TestImpl;

#[async_trait]
impl TestTrait for TestImpl {
    #[span_fn]
    async fn test_method(&self) -> String {
        "test".to_string()
    }
}

#[tokio::test]
async fn test_async_trait_span_fn() {
    let test_impl = TestImpl;
    let result = test_impl.test_method().await;
    assert_eq!(result, "test");
    
    // Verify span events were generated
    // (requires integration with test telemetry sink)
}
```

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

## Implementation Timeline

### Week 1: Analysis and Foundation
- [ ] **First Task**: Write a minimal unit test that demonstrates the current limitation
  - Create simple async trait with one method
  - Attempt to apply `#[span_fn]` to the trait method implementation
  - Document the compilation error or behavior that proves the problem
  - Use this as the baseline test case for measuring success
- [ ] Research async trait macro expansion behavior
- [ ] Design detection logic for async trait methods
- [ ] Create proof-of-concept implementation

### Week 2: Core Implementation  
- [ ] Implement async trait method detection
- [ ] Extend `span_fn` macro with async trait support
- [ ] Ensure `InstrumentedFuture` compatibility

### Week 3: Integration and Testing
- [ ] Add comprehensive test suite
- [ ] Update existing async trait implementations
- [ ] Verify span event generation and collection

### Week 4: Documentation and Cleanup
- [ ] Update AI Guidelines and documentation
- [ ] Code review and optimization
- [ ] Validate with real-world usage patterns

## Success Criteria

### Functional Requirements
- [ ] `#[span_fn]` works on async trait method implementations
- [ ] Generates correct `BeginAsyncSpanEvent`/`EndAsyncSpanEvent` pairs
- [ ] Compatible with existing `#[async_trait]` usage patterns
- [ ] No performance degradation compared to regular async functions

### Quality Requirements
- [ ] Comprehensive test coverage for async trait scenarios
- [ ] Clear error messages for unsupported usage patterns
- [ ] Documentation includes examples and best practices
- [ ] Follows existing code style and conventions

### Integration Requirements
- [ ] Works with all existing async trait implementations in codebase
- [ ] Compatible with `InstrumentedFuture` and async span infrastructure
- [ ] Maintains backward compatibility with existing `#[span_fn]` usage

## Potential Challenges and Mitigations

### Challenge 1: Macro Expansion Order
**Issue**: `#[async_trait]` and `#[span_fn]` may interfere with each other
**Mitigation**: Design async trait detection that works regardless of expansion order

### Challenge 2: Complex Type Signatures
**Issue**: Async trait methods have complex return types with lifetimes
**Mitigation**: Preserve original signatures and only modify function bodies

### Challenge 3: Boxed Future Compatibility
**Issue**: `InstrumentedFuture` must work with `Pin<Box<dyn Future>>`
**Mitigation**: Ensure generic implementation supports all future types

### Challenge 4: Performance Impact
**Issue**: Additional boxing/unboxing might affect performance
**Mitigation**: Benchmark against untraced async trait methods and optimize

## Future Extensions

### Dynamic Trait Objects
Support for tracing dynamic trait objects:
```rust
let service: Box<dyn MyService> = Box::new(MyServiceImpl);
service.process(data).await; // Should still be traced
```

### Generic Async Traits
Enhanced support for generic async traits with complex type parameters

### Conditional Compilation
Support for conditional tracing based on feature flags or runtime configuration

## Dependencies

### External Crates
- `async-trait` - Already in use, no version changes needed
- `syn`, `quote`, `proc-macro2` - For macro implementation
- `tokio` - For async runtime in tests

### Internal Components
- `rust/tracing/src/async_instrumentation.rs` - Core async tracing infrastructure
- `rust/tracing/src/guards.rs` - `AsyncSpanGuard` implementation
- `rust/tracing/src/spans/events.rs` - Async span event definitions

This plan provides a comprehensive roadmap for adding async trait support to the micromegas tracing system while maintaining compatibility with existing code and following established patterns.
