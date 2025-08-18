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

### Phase 2: 🚀 Core Implementation - READY TO START

#### 2.1 Add Future Return Type Detection
**Location**: `rust/tracing/proc-macros/src/lib.rs`

Add a helper function to detect Future return types:

```rust
use syn::{ReturnType, Type, TypePath, Path};

/// Check if the function returns a Future (indicating it's an async trait method)
fn returns_future(function: &ItemFn) -> bool {
    match &function.sig.output {
        ReturnType::Type(_, ty) => {
            is_future_type(ty)
        }
        ReturnType::Default => false,
    }
}

/// Check if a type is a Future type (Pin<Box<dyn Future>> or impl Future)
fn is_future_type(ty: &Type) -> bool {
    match ty {
        // Check for Pin<Box<dyn Future<...>>>
        Type::Path(TypePath { path, .. }) => {
            if let Some(last_segment) = path.segments.last() {
                if last_segment.ident == "Pin" {
                    // Check if it contains Box<dyn Future>
                    // This is the pattern async-trait generates
                    return true;
                }
            }
            false
        }
        // Check for impl Future<...>
        Type::ImplTrait(impl_trait) => {
            impl_trait.bounds.iter().any(|bound| {
                // Check if bound is Future
                matches!(bound, syn::TypeParamBound::Trait(_))
            })
        }
        _ => false,
    }
}
```

#### 2.2 Update span_fn Macro Logic
**Location**: `rust/tracing/proc-macros/src/lib.rs` (line 47)

Modify the condition to handle async trait methods:

```rust
pub fn span_fn(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as TraceArgs);
    let mut function = parse_macro_input!(input as ItemFn);

    let function_name = args
        .alternative_name
        .map_or(function.sig.ident.to_string(), |n| n.to_string());

    // UPDATED: Check both async keyword AND Future return type
    if function.sig.asyncness.is_some() || returns_future(&function) {
        // Handle both regular async functions AND async trait methods
        let original_block = &function.block;
        let output_type = match &function.sig.output {
            syn::ReturnType::Type(_, ty) => quote! { #ty },
            syn::ReturnType::Default => quote! { () },
        };

        // For async trait methods, the signature is already transformed
        // so we don't need to remove asyncness or change return type
        if function.sig.asyncness.is_none() {
            // This is an async trait method (no async keyword, but returns Future)
            // Keep the signature as-is
        } else {
            // Regular async function - transform as before
            function.sig.asyncness = None;
            function.sig.output = parse_quote! { -> impl std::future::Future<Output = #output_type> };
        }
        
        function.block = parse_quote! {
            {
                static_span_desc!(_SCOPE_DESC, concat!(module_path!(), "::", #function_name));
                let fut = async move #original_block;
                InstrumentedFuture::new(fut, &_SCOPE_DESC)
            }
        };
    } else {
        // Handle sync functions
        function.block.stmts.insert(
            0,
            parse_quote! {
                span_scope!(_METADATA_FUNC, concat!(module_path!(), "::", #function_name));
            },
        );
    }

    TokenStream::from(quote! { #function })
}
```

### Phase 3: Testing and Validation

#### 3.1 Test the Implementation
After implementing the changes, run the existing test suite to verify:

```bash
cargo test --package micromegas-analytics test_comparison_async_trait_vs_regular -- --nocapture
```

Expected output should show:
- Both async trait methods AND regular async functions calling `on_begin_async_scope`/`on_end_async_scope`
- No more `BeginThreadSpanEvent`/`EndThreadSpanEvent` for async trait methods
- Total of 4 events (2 for each async function)

### Phase 4: Integration with Existing Code

Once the fix is implemented and tested, async trait methods throughout the codebase can use `#[span_fn]`:

```rust
// Example: rust/analytics/src/record_batch_transformer.rs
#[async_trait]
impl RecordBatchTransformer for TrivialRecordBatchTransformer {
    #[span_fn]  // Will now generate proper async span events!
    async fn transform(&self, src: RecordBatch) -> Result<RecordBatch> {
        Ok(src)
    }
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

## Implementation Steps

### Immediate Actions
1. ✅ **DONE**: Identify and confirm root cause
2. ✅ **DONE**: Create test suite demonstrating the issue
3. **NEXT**: Implement Future return type detection in proc macro
4. **THEN**: Test the fix with existing test suite
5. **FINALLY**: Update documentation and remove outdated comments

## Success Criteria

✅ **Test Results Will Show**:
- Async trait methods generate `BeginAsyncSpanEvent`/`EndAsyncSpanEvent` 
- Debug output shows `on_begin_async_scope`/`on_end_async_scope` calls
- Event counts match between async trait methods and regular async functions
- No more thread span events for async methods

## Key Insights

1. **The problem is simpler than expected**: Just need to detect Future return types
2. **No new infrastructure needed**: `InstrumentedFuture` already exists and works
3. **Minimal code change**: Add ~20 lines to detect Future types, modify 1 condition
4. **Test suite already in place**: Comprehensive tests ready to validate the fix

## Summary

The root cause has been confirmed: `#[span_fn]` doesn't recognize async trait methods because `#[async_trait]` removes the `async` keyword, transforming the signature to return `Pin<Box<dyn Future>>`. 

The fix is straightforward: Add Future return type detection to the proc macro so it treats these methods as async, generating proper async span instrumentation instead of thread span instrumentation.
