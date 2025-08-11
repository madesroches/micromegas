use micromegas_tracing::dispatch::{
    flush_thread_buffer, force_uninit, init_event_dispatch, init_thread_stream, shutdown_dispatch,
};
use micromegas_tracing::event::EventSink;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::event::in_memory_sink::InMemorySink;
use micromegas_tracing::{prelude::*, static_span_desc};
use rand::Rng;
use serial_test::serial;
use std::collections::HashMap;
use std::sync::Arc;
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

fn init_in_mem_tracing(sink: Arc<dyn EventSink>) {
    init_event_dispatch(1024, 1024, 1024, sink, HashMap::new()).unwrap();
}

#[test]
#[serial]
fn test_async_span_manual_instrumentation() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("tracing-test")
        .on_thread_start(|| {
            init_thread_stream();
        })
        .on_thread_stop(|| {
            flush_thread_buffer();
        })
        .build()
        .expect("failed to build tokio runtime");
    static_span_desc!(OUTER_DESC, "manual_outer");
    runtime.block_on(async {
        let result = tokio::task::spawn(async {
            let output = manual_outer().instrument(&OUTER_DESC).await;
            flush_thread_buffer();
            output
        })
        .await
        .expect("Task failed");
        result
    });

    // Drop the runtime to properly shut down worker threads
    drop(runtime);
    shutdown_dispatch();

    // Check that the correct number of events were recorded from manual instrumentation
    let state = sink.state.lock().expect("Failed to lock sink state");
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

    unsafe { force_uninit() };
}

#[test]
#[serial]
fn test_async_span_macro() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("tracing-test")
        .on_thread_start(|| {
            init_thread_stream();
        })
        .on_thread_stop(|| {
            flush_thread_buffer();
        })
        .build()
        .expect("failed to build tokio runtime");

    // Spawn macro_outer on a worker thread instead of running on main thread
    runtime.block_on(async {
        let result = tokio::task::spawn(async {
            let output = macro_outer().await;
            flush_thread_buffer();
            output
        })
        .await
        .expect("Task failed");
        result
    });

    // Drop the runtime to properly shut down worker threads
    drop(runtime);
    shutdown_dispatch();

    // Check that the correct number of events were recorded
    let state = sink.state.lock().expect("Failed to lock sink state");
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

    unsafe { force_uninit() };
}

#[test]
#[serial]
fn sync_span_macro() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());
    init_thread_stream();
    instrumented_sync_function();
    flush_thread_buffer();
    shutdown_dispatch();

    // Check that the correct number of events were recorded
    let state = sink.state.lock().expect("Failed to lock sink state");
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

    unsafe { force_uninit() };
}
