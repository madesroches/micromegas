use async_trait::async_trait;
use micromegas_tracing::dispatch::flush_thread_buffer;
use micromegas_tracing::prelude::*;
use micromegas_tracing::spans::ThreadEventQueueAny;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;
use std::collections::HashMap;

/// Simple async trait for testing span_fn macro support
#[async_trait]
trait SimpleService {
    async fn process(&self, input: &str) -> String;
    async fn transform(&self, data: Vec<u8>) -> Vec<u8>;
}

/// Generic async trait for testing with generics
#[async_trait]
trait GenericService<T: Send + Sync> {
    async fn handle(&self, item: T) -> T;
}

/// Async trait with complex signatures
#[async_trait]
trait ComplexService {
    async fn complex_method(
        &self,
        data: &[u8],
        options: HashMap<String, String>,
    ) -> Result<Vec<u8>, String>;
}

/// Implementation of SimpleService that uses span_fn
struct SimpleServiceImpl;

#[async_trait]
impl SimpleService for SimpleServiceImpl {
    #[span_fn]
    async fn process(&self, input: &str) -> String {
        format!("processed: {}", input)
    }

    #[span_fn]
    async fn transform(&self, mut data: Vec<u8>) -> Vec<u8> {
        data.reverse();
        data
    }
}

/// Implementation of GenericService
struct GenericServiceImpl;

#[async_trait]
impl GenericService<String> for GenericServiceImpl {
    #[span_fn]
    async fn handle(&self, item: String) -> String {
        format!("handled: {}", item)
    }
}

/// Implementation of ComplexService
struct ComplexServiceImpl;

#[async_trait]
impl ComplexService for ComplexServiceImpl {
    #[span_fn]
    async fn complex_method(
        &self,
        data: &[u8],
        _options: HashMap<String, String>,
    ) -> Result<Vec<u8>, String> {
        Ok(data.to_vec())
    }
}

/// Control test: regular async function with span_fn (should work)
#[span_fn]
async fn regular_async_function(input: &str) -> String {
    format!("regular: {}", input)
}

/// Control test: sync function with span_fn (should generate thread span events)
#[span_fn]
fn sync_function(input: &str) -> String {
    format!("sync: {}", input)
}

/// Comprehensive test: validates all async trait variations generate correct event types
/// This test covers functionality + event type validation in one robust test
#[test]
#[serial]
fn test_async_trait_comprehensive() {
    let guard = init_in_memory_tracing();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("async-trait-comprehensive")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async {
        tokio::task::spawn(async {
            // Test sync function (control) - should generate thread span events
            let sync_result = sync_function("sync_test");
            assert_eq!(sync_result, "sync: sync_test");
            flush_thread_buffer();

            // Test all async trait variations - should generate async span events
            let simple_service = SimpleServiceImpl;
            let result1 = simple_service.process("test_data").await;
            let result2 = simple_service.transform(vec![1, 2, 3, 4]).await;

            let generic_service = GenericServiceImpl;
            let result3 = generic_service.handle("generic_test".to_string()).await;

            let complex_service = ComplexServiceImpl;
            let mut options = HashMap::new();
            options.insert("key".to_string(), "value".to_string());
            let result4 = complex_service
                .complex_method(b"complex_data", options)
                .await;

            // Regular async function (control) - should generate async span events
            let result5 = regular_async_function("control").await;

            flush_thread_buffer();

            // Verify functionality
            assert_eq!(result1, "processed: test_data");
            assert_eq!(result2, vec![4, 3, 2, 1]);
            assert_eq!(result3, "handled: generic_test");
            assert!(result4.is_ok());
            assert_eq!(result4.unwrap(), b"complex_data");
            assert_eq!(result5, "regular: control");
        })
        .await
        .expect("Task failed")
    });

    drop(runtime);

    // Validate event types using HeterogeneousQueue inspection
    let state = guard.sink.state.lock().expect("Failed to lock sink state");
    let mut sync_span_events = 0;
    let mut async_span_events = 0;
    let mut total_events = 0;

    for block in state.thread_blocks.iter() {
        for event in block.events.iter() {
            total_events += 1;

            match event {
                ThreadEventQueueAny::BeginThreadSpanEvent(_)
                | ThreadEventQueueAny::EndThreadSpanEvent(_) => {
                    sync_span_events += 1;
                }
                ThreadEventQueueAny::BeginAsyncSpanEvent(_)
                | ThreadEventQueueAny::EndAsyncSpanEvent(_) => {
                    async_span_events += 1;
                }
                _ => {} // Named span events, etc.
            }
        }
    }

    println!("=== COMPREHENSIVE VALIDATION RESULTS ===");
    println!("Total events: {}", total_events);
    println!(
        "Sync span events: {} (from sync_function)",
        sync_span_events
    );
    println!(
        "Async span events: {} (from async traits + regular async)",
        async_span_events
    );

    // Expected: 1 sync function (2 events) + 5 async functions (10 events) = 12 total
    assert_eq!(
        total_events, 12,
        "Expected 12 total events but found {}",
        total_events
    );
    assert_eq!(
        sync_span_events, 2,
        "Expected 2 sync span events but found {}",
        sync_span_events
    );
    assert_eq!(
        async_span_events, 10,
        "Expected 10 async span events but found {}",
        async_span_events
    );

    println!("✅ SUCCESS: All function types correctly instrumented!");
    println!("✅ SYNC FUNCTIONS: Generate thread span events");
    println!("✅ ASYNC TRAIT METHODS: Generate async span events (not sync)");
    println!("✅ REGULAR ASYNC FUNCTIONS: Generate async span events");
    println!("✅ COVERAGE: Simple, generic, and complex async trait variations");
}

/// Focused test: validates that async trait methods work identically to regular async functions
#[test]
#[serial]
fn test_async_trait_equivalence() {
    let guard = init_in_memory_tracing();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("async-trait-equivalence")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async {
        tokio::task::spawn(async {
            // Test async trait method vs regular async function
            let service = SimpleServiceImpl;
            let trait_result = service.process("trait_test").await;
            let regular_result = regular_async_function("regular_test").await;

            flush_thread_buffer();

            // Verify functionality
            assert_eq!(trait_result, "processed: trait_test");
            assert_eq!(regular_result, "regular: regular_test");
        })
        .await
        .expect("Task failed")
    });

    drop(runtime);

    // Validate both generate async span events (not sync events)
    let state = guard.sink.state.lock().expect("Failed to lock sink state");
    let mut async_events = 0;
    let mut sync_events = 0;

    for block in state.thread_blocks.iter() {
        for event in block.events.iter() {
            match event {
                ThreadEventQueueAny::BeginAsyncSpanEvent(_)
                | ThreadEventQueueAny::EndAsyncSpanEvent(_) => async_events += 1,
                ThreadEventQueueAny::BeginThreadSpanEvent(_)
                | ThreadEventQueueAny::EndThreadSpanEvent(_) => sync_events += 1,
                _ => {}
            }
        }
    }

    assert_eq!(
        async_events, 4,
        "Expected 4 async span events (2 functions × 2 events)"
    );
    assert_eq!(
        sync_events, 0,
        "Expected 0 sync span events (all should be async)"
    );

    println!("✅ EQUIVALENCE: Async trait methods behave identically to regular async functions");
}
