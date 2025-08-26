//! Tests for FlushMonitor thread safety during concurrent thread lifecycle operations.
//!
//! This test verifies that FlushMonitor can safely iterate through thread streams
//! while threads are being created, parked, and destroyed concurrently without
//! accessing dangling pointers or causing data races.

use micromegas_tracing::dispatch::{
    for_each_thread_stream, force_uninit, init_event_dispatch, shutdown_dispatch,
};
use micromegas_tracing::event::EventSink;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::event::in_memory_sink::InMemorySink;
use micromegas_tracing::flush_monitor::FlushMonitor;
use micromegas_tracing::prelude::*;
use serial_test::serial;
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::time::sleep;

/// A simple async function that causes thread parking, allowing us to test
/// concurrent FlushMonitor operations during thread lifecycle events.
#[span_fn]
async fn work_with_parking() {
    // Short sleep to cause thread parking
    sleep(Duration::from_millis(50)).await;
}

fn init_tracing_with_sink(sink: Arc<dyn EventSink>) {
    init_event_dispatch(1024, 1024, 1024, sink, HashMap::new(), true) // Enable CPU tracing for tests
        .expect("Failed to initialize event dispatch");
}

/// Tests that FlushMonitor can safely operate concurrently with thread
/// lifecycle operations (start, park, stop) without accessing dangling
/// pointers or causing data races.
///
/// This test creates a high-concurrency scenario with:
/// - Multiple worker threads starting and stopping
/// - Thread parking during async operations
/// - Concurrent FlushMonitor ticks accessing thread streams
/// - Thread unregistration during runtime shutdown
///
/// The test verifies that:
/// 1. No crashes occur from dangling pointer access
/// 2. Thread streams are properly synchronized between FlushMonitor and lifecycle
/// 3. All events are correctly recorded despite high concurrency
#[test]
#[serial]
fn test_flush_monitor_concurrent_thread_safety() {
    // Enable CPU tracing for this test
    unsafe {
        std::env::set_var("MICROMEGAS_ENABLE_CPU_TRACING", "true");
    }

    let sink = Arc::new(InMemorySink::new());
    init_tracing_with_sink(sink.clone()); // Track thread creation for debugging
    let thread_counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = thread_counter.clone();

    // Build tokio runtime with full thread lifecycle management
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("flush-safety-test")
        .worker_threads(4) // Multiple threads to increase race opportunities
        .with_tracing_callbacks_and_custom_start(move || {
            let id = counter_clone.fetch_add(1, Ordering::Relaxed);
            eprintln!("Worker thread {} starting - initializing stream", id);
        })
        .build()
        .expect("Failed to build tokio runtime");

    // Create FlushMonitor with short period to increase tick frequency
    let flush_monitor = FlushMonitor::new(1);

    runtime.block_on(async {
        // Spawn multiple concurrent tasks to create thread churn
        let work_tasks = (0..16)
            .map(|i| {
                tokio::task::spawn(async move {
                    eprintln!("Work task {} starting", i);
                    work_with_parking().await;
                    eprintln!("Work task {} completed", i);
                })
            })
            .collect::<Vec<_>>();

        // Concurrently run FlushMonitor ticks while work is happening
        let flush_task = tokio::task::spawn(async move {
            for tick in 0..8 {
                // Wait between ticks to allow work tasks to progress
                tokio::time::sleep(Duration::from_millis(125)).await;

                eprintln!("FlushMonitor tick {} - scanning thread streams", tick);
                flush_monitor.tick();

                // Test direct for_each_thread_stream access (same path as FlushMonitor)
                let mut active_streams = 0;
                for_each_thread_stream(&mut |_stream_ptr| {
                    active_streams += 1;
                });
                eprintln!("  Found {} active thread streams", active_streams);
            }
        });

        // Wait for all work tasks to complete
        for (i, task) in work_tasks.into_iter().enumerate() {
            task.await
                .unwrap_or_else(|e| panic!("Work task {} failed: {}", i, e));
        }

        // Wait for flush monitoring to complete
        flush_task.await.expect("Flush task failed");
    });

    // Clean shutdown - this triggers thread unregistration
    drop(runtime);
    shutdown_dispatch();

    // Verify that events were properly recorded despite concurrency
    let state = sink.state.lock().expect("Failed to lock sink state");
    let total_events: usize = state
        .thread_blocks
        .iter()
        .map(|block| block.nb_objects())
        .sum();

    eprintln!("Test completed - total events recorded: {}", total_events);

    // Each work_with_parking() call generates 2 events (begin + end span)
    // With 16 tasks, expect at least 32 events
    assert!(
        total_events >= 32,
        "Expected at least 32 events from 16 work tasks (2 events each), but found {}",
        total_events
    );

    // Clean up global state
    unsafe { force_uninit() };
}
