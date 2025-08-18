use async_trait::async_trait;
use micromegas_tracing::dispatch::{
    flush_thread_buffer, force_uninit, init_event_dispatch, shutdown_dispatch,
};
use micromegas_tracing::event::in_memory_sink::InMemorySink;
use micromegas_tracing::event::{EventSink, TracingBlock};
use micromegas_tracing::prelude::*;
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;
use std::collections::HashMap;
use std::sync::Arc;

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
    #[span_async_trait]
    async fn process(&self, input: &str) -> String {
        format!("processed: {}", input)
    }

    #[span_async_trait]
    async fn transform(&self, mut data: Vec<u8>) -> Vec<u8> {
        data.reverse();
        data
    }
}

/// Implementation of GenericService
struct GenericServiceImpl;

#[async_trait]
impl GenericService<String> for GenericServiceImpl {
    #[span_async_trait]
    async fn handle(&self, item: String) -> String {
        format!("handled: {}", item)
    }
}

/// Implementation of ComplexService
struct ComplexServiceImpl;

#[async_trait]
impl ComplexService for ComplexServiceImpl {
    #[span_async_trait]
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

fn init_in_mem_tracing(sink: Arc<dyn EventSink>) {
    init_event_dispatch(1024, 1024, 1024, sink, HashMap::new()).unwrap();
}

#[test]
#[serial]
fn test_async_trait_span_fn_comprehensive() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("async-trait-comprehensive")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async {
        tokio::task::spawn(async {
            // Test SimpleService
            let simple_service = SimpleServiceImpl;
            let result1 = simple_service.process("test_data").await;
            let result2 = simple_service.transform(vec![1, 2, 3, 4]).await;

            // Test GenericService
            let generic_service = GenericServiceImpl;
            let result3 = generic_service.handle("generic_test".to_string()).await;

            // Test ComplexService
            let complex_service = ComplexServiceImpl;
            let mut options = HashMap::new();
            options.insert("key".to_string(), "value".to_string());
            let result4 = complex_service
                .complex_method(b"complex_data", options)
                .await;

            // Control: regular async function
            let result5 = regular_async_function("control").await;

            flush_thread_buffer();

            // Verify basic functionality works
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
    shutdown_dispatch();

    // Check what events were actually recorded
    let state = sink.state.lock().expect("Failed to lock sink state");
    let total_events: usize = state
        .thread_blocks
        .iter()
        .map(|block| block.nb_objects())
        .sum();

    println!("Total events recorded: {}", total_events);

    // We expect:
    // - SimpleServiceImpl::process: 2 events (begin + end span)
    // - SimpleServiceImpl::transform: 2 events (begin + end span)
    // - GenericServiceImpl::handle: 2 events (begin + end span)
    // - ComplexServiceImpl::complex_method: 2 events (begin + end span)
    // - regular_async_function: 2 events (begin + end span)
    // Total expected: 10 events

    assert_eq!(
        total_events, 10,
        "Expected 10 events (5 async functions × 2 events each) but found {}",
        total_events
    );

    println!("SUCCESS: #[span_fn] works with ALL async trait variations!");
    println!("- Simple async trait methods: ✓");
    println!("- Generic async trait methods: ✓");
    println!("- Complex async trait methods: ✓");
    println!("- Regular async functions: ✓");

    unsafe { force_uninit() };
}

#[test]
#[serial]
fn test_async_trait_span_fn_current_behavior() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("async-trait-test")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async {
        tokio::task::spawn(async {
            let service = SimpleServiceImpl;

            // Test async trait method calls
            let result1 = service.process("test_data").await;
            let result2 = service.transform(vec![1, 2, 3, 4]).await;

            // Control: regular async function
            let result3 = regular_async_function("control").await;

            flush_thread_buffer();

            // Verify basic functionality works
            assert_eq!(result1, "processed: test_data");
            assert_eq!(result2, vec![4, 3, 2, 1]);
            assert_eq!(result3, "regular: control");
        })
        .await
        .expect("Task failed")
    });

    drop(runtime);
    shutdown_dispatch();

    // Check what events were actually recorded
    let state = sink.state.lock().expect("Failed to lock sink state");
    let total_events: usize = state
        .thread_blocks
        .iter()
        .map(|block| block.nb_objects())
        .sum();

    println!("Total events recorded: {}", total_events);

    // Print detailed events for debugging
    for (i, block) in state.thread_blocks.iter().enumerate() {
        println!("Block {}: {} events", i, block.nb_objects());
    }

    // Analysis of results:
    // We expect:
    // - SimpleServiceImpl::process: 2 events (begin + end span)
    // - SimpleServiceImpl::transform: 2 events (begin + end span)
    // - regular_async_function: 2 events (begin + end span)
    // Total expected: 6 events

    if total_events == 6 {
        println!("SUCCESS: #[span_fn] IS WORKING with async trait methods!");
        println!("All three async functions generated span events:");
        println!("- SimpleServiceImpl::process: 2 events");
        println!("- SimpleServiceImpl::transform: 2 events");
        println!("- regular_async_function: 2 events");
        println!("TOTAL: 6 events");

        // This means the feature already works! Update the plan.
    } else if total_events == 2 {
        println!("CURRENT BEHAVIOR: Only regular_async_function generated events");
        println!("Async trait methods with #[span_fn] do not generate span events");
    } else {
        println!(
            "UNEXPECTED BEHAVIOR: Got {} events, need to investigate",
            total_events
        );
        // This will help us understand what actually happens
    }

    unsafe { force_uninit() };
}

#[test]
#[serial]
fn test_simple_service_process_events() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("simple-service-events")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async {
        tokio::task::spawn(async {
            let service = SimpleServiceImpl;

            // Call ONLY SimpleService::process to isolate its events
            println!("About to call SimpleService::process...");
            let result = service.process("test_input").await;
            println!("SimpleService::process returned: {}", result);
            assert_eq!(result, "processed: test_input");

            flush_thread_buffer();
        })
        .await
        .expect("Task failed")
    });

    drop(runtime);
    shutdown_dispatch();

    // Inspect the actual events
    let state = sink.state.lock().expect("Failed to lock sink state");

    println!("=== EVENTS FROM SimpleService::process ===");
    println!("Number of blocks: {}", state.thread_blocks.len());

    let total_events: usize = state
        .thread_blocks
        .iter()
        .map(|block| block.nb_objects())
        .sum();

    println!("Total events: {}", total_events);

    for (block_idx, block) in state.thread_blocks.iter().enumerate() {
        println!("Block {}: {} events", block_idx, block.nb_objects());
    }

    // What we want to prove:
    if total_events == 2 {
        println!("✓ SUCCESS: SimpleService::process with #[span_fn] generated 2 events");
        println!("  This proves that #[span_fn] WORKS with async trait methods!");
        println!("  Expected: BeginAsyncSpanEvent + EndAsyncSpanEvent");
    } else if total_events == 0 {
        println!("✗ LIMITATION: SimpleService::process with #[span_fn] generated 0 events");
        println!("  This confirms the limitation mentioned in the proc-macro comment");
    } else {
        println!(
            "? UNEXPECTED: Got {} events, need investigation",
            total_events
        );
    }

    unsafe { force_uninit() };
}

#[test]
#[serial]
fn test_comparison_async_trait_vs_regular() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("comparison-test")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async {
        tokio::task::spawn(async {
            println!("=== TESTING ASYNC TRAIT METHOD ===");
            let service = SimpleServiceImpl;
            let result1 = service.process("async_trait_test").await;
            println!("Async trait result: {}", result1);

            println!("=== TESTING REGULAR ASYNC FUNCTION ===");
            let result2 = regular_async_function("regular_test").await;
            println!("Regular async result: {}", result2);

            flush_thread_buffer();
        })
        .await
        .expect("Task failed")
    });

    drop(runtime);
    shutdown_dispatch();

    let state = sink.state.lock().expect("Failed to lock sink state");
    let total_events: usize = state
        .thread_blocks
        .iter()
        .map(|block| block.nb_objects())
        .sum();

    println!("=== COMPARISON RESULTS ===");
    println!("Total events from both calls: {}", total_events);

    for (block_idx, block) in state.thread_blocks.iter().enumerate() {
        println!("Block {}: {} events", block_idx, block.nb_objects());
    }

    // We expect:
    // - SimpleService::process: 2 events
    // - regular_async_function: 2 events
    // Total: 4 events

    if total_events == 4 {
        println!(
            "✓ PERFECT: Both async trait method and regular async function generated identical event counts!"
        );
        println!("  - Async trait method: 2 events (BeginAsyncSpanEvent + EndAsyncSpanEvent)");
        println!("  - Regular async function: 2 events (BeginAsyncSpanEvent + EndAsyncSpanEvent)");
        println!(
            "  CONCLUSION: #[span_fn] works identically for async traits and regular async functions"
        );
    } else {
        println!(
            "? UNEXPECTED: Expected 4 events total, got {}",
            total_events
        );
    }

    unsafe { force_uninit() };
}

#[test]
#[serial]
fn test_detailed_simple_service_process_events() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("detailed-events")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async {
        tokio::task::spawn(async {
            let service = SimpleServiceImpl;

            println!("About to call SimpleService::process...");
            let result = service.process("detailed_test").await;
            println!("SimpleService::process returned: {}", result);

            flush_thread_buffer();
        })
        .await
        .expect("Task failed")
    });

    drop(runtime);
    shutdown_dispatch();

    // Process the collected events using the analytics infrastructure
    let state = sink.state.lock().expect("Failed to lock sink state");

    println!("=== DETAILED EVENTS FROM SimpleService::process ===");

    for (block_idx, block) in state.thread_blocks.iter().enumerate() {
        println!("Block {}: {} events", block_idx, block.nb_objects());
        println!("  Process ID: {}", block.process_id);
        println!("  Stream ID: {}", block.stream_id);
        println!("  Begin time: {:?}", block.begin);
        println!("  End time: {:?}", block.end);
        println!("  Event offset: {}", block.event_offset);

        // The events are stored in block.events (ThreadEventQueue)
        // Let's see what we can extract from it
        let events_size = block.events.len_bytes();
        let events_count = block.events.nb_objects();
        let events_capacity = block.events.capacity_bytes();

        println!("  Events queue stats:");
        println!("    Size: {} bytes", events_size);
        println!("    Count: {} objects", events_count);
        println!("    Capacity: {} bytes", events_capacity);
    }

    let total_events: usize = state
        .thread_blocks
        .iter()
        .map(|block| block.nb_objects())
        .sum();

    println!("=== SUMMARY ===");
    println!("Total events: {}", total_events);

    if total_events == 2 {
        println!("✓ SUCCESS: SimpleService::process generated exactly 2 async span events!");
        println!("  These are likely BeginAsyncSpanEvent + EndAsyncSpanEvent");
        println!("  This proves #[span_fn] works perfectly with async trait methods");
    }

    unsafe { force_uninit() };
}
