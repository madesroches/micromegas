use micromegas_tracing::dispatch::{flush_thread_buffer, init_thread_stream};
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::intern_string::intern_string;
use micromegas_tracing::prelude::*;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use rand::Rng;
use serial_test::serial;
use std::time::Duration;
use tokio::time::sleep;

async fn manual_inner() {
    let ms = rand::thread_rng().gen_range(0..=1000);
    eprintln!("wainting for {ms} ms");
    sleep(Duration::from_millis(ms)).await;
}

async fn manual_outer() {
    static_span_desc!(INNER1_DESC, "manual_inner_1");
    manual_inner().instrument(&INNER1_DESC).await;
    static_span_desc!(INNER2_DESC, "manual_inner_2");
    manual_inner().instrument(&INNER2_DESC).await;
}

#[span_fn]
async fn macro_inner() {
    let ms = rand::thread_rng().gen_range(0..=1000);
    eprintln!("waiting for {ms} ms");
    sleep(Duration::from_millis(ms)).await;
}

#[span_fn]
async fn macro_outer() {
    macro_inner().await;
    macro_inner().await;
}

#[span_fn]
fn instrumented_sync_function() {
    eprintln!("This is a sync function");
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
#[serial]
fn test_async_span_manual_instrumentation() {
    unsafe {
        std::env::set_var("MICROMEGAS_ENABLE_CPU_TRACING", "true");
    }
    let guard = init_in_memory_tracing();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("tracing-test")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");
    static_span_desc!(OUTER_DESC, "manual_outer");
    runtime.block_on(async {
        tokio::task::spawn(async {
            let output = manual_outer().instrument(&OUTER_DESC).await;
            flush_thread_buffer();
            output
        })
        .await
        .expect("Task failed")
    });

    // Drop the runtime to properly shut down worker threads
    drop(runtime);

    // Check that the correct number of events were recorded from manual instrumentation
    let state = guard.sink.state.lock().expect("Failed to lock sink state");
    let total_events: usize = state
        .thread_blocks
        .iter()
        .map(|block| block.nb_objects())
        .sum();

    // manual_outer wrapper (2 events) + 2 calls to manual_inner with wrappers (2 events each) = 6 events total
    assert_eq!(
        total_events, 6,
        "Expected 6 events from manual instrumentation (outer: 2 + inner: 2×2) but found {}",
        total_events
    );
}

#[test]
#[serial]
fn test_async_span_macro() {
    unsafe {
        std::env::set_var("MICROMEGAS_ENABLE_CPU_TRACING", "true");
    }
    let guard = init_in_memory_tracing();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("tracing-test")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");

    // Spawn macro_outer on a worker thread instead of running on main thread
    runtime.block_on(async {
        tokio::task::spawn(async {
            let output = macro_outer().await;
            flush_thread_buffer();
            output
        })
        .await
        .expect("Task failed")
    });

    // Drop the runtime to properly shut down worker threads
    drop(runtime);

    // Check that the correct number of events were recorded
    let state = guard.sink.state.lock().expect("Failed to lock sink state");
    let total_events: usize = state
        .thread_blocks
        .iter()
        .map(|block| block.nb_objects())
        .sum();

    // macro_outer (2 events) + 2 calls to macro_inner (2 events each) = 6 events total
    assert_eq!(
        total_events, 6,
        "Expected 6 events (macro_outer: 2 + macro_inner: 2×2) but found {}",
        total_events
    );
}

#[test]
#[serial]
fn sync_span_macro() {
    let guard = init_in_memory_tracing();
    init_thread_stream();
    instrumented_sync_function();
    flush_thread_buffer();

    // Check that the correct number of events were recorded
    let state = guard.sink.state.lock().expect("Failed to lock sink state");
    let total_events: usize = state
        .thread_blocks
        .iter()
        .map(|block| block.nb_objects())
        .sum();

    // instrumented_sync_function should generate 2 events: begin span and end span
    assert_eq!(
        total_events, 2,
        "Expected 2 events (begin + end span) but found {}",
        total_events
    );
}

async fn named_inner_work(operation: &'static str) {
    let ms = rand::thread_rng().gen_range(0..=500);
    eprintln!("doing {} for {} ms", operation, ms);
    sleep(Duration::from_millis(ms)).await;
}

async fn test_named_spans() {
    // Test named spans in a loop with interned strings containing iteration numbers
    for i in 0..5 {
        let span_name = intern_string(&format!("iteration_{}", i));
        span_async_named!(span_name, async {
            named_inner_work("loop work").await;
        })
        .await;
    }

    // Test the lower-level API with interned strings in a loop
    for i in 0..3 {
        let operation_name = intern_string(&format!("background_operation_{}", i));
        instrument_named!(named_inner_work("background task"), operation_name).await;
    }
}

#[test]
#[serial]
fn test_async_named_span_instrumentation() {
    unsafe {
        std::env::set_var("MICROMEGAS_ENABLE_CPU_TRACING", "true");
    }
    let guard = init_in_memory_tracing();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("tracing-test")
        .with_tracing_callbacks()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async {
        tokio::task::spawn(async {
            let output = test_named_spans().await;
            flush_thread_buffer();
            output
        })
        .await
        .expect("Task failed")
    });

    // Drop the runtime to properly shut down worker threads
    drop(runtime);

    // Check that the correct number of events were recorded
    let state = guard.sink.state.lock().expect("Failed to lock sink state");
    let total_events: usize = state
        .thread_blocks
        .iter()
        .map(|block| block.nb_objects())
        .sum();

    // We should have:
    // - 5 spans from first loop (iteration_0 through iteration_4) = 10 events (begin + end each)
    // - 3 spans from second loop (background_operation_0 through background_operation_2) = 6 events
    // Total = 16 events
    assert_eq!(
        total_events, 16,
        "Expected 16 events from named async spans (5 + 3 named spans, 2 events each) but found {}",
        total_events
    );
}
