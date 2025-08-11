use micromegas_tracing::dispatch::{
    flush_thread_buffer, force_uninit, init_event_dispatch, init_thread_stream, shutdown_dispatch,
    unregister_thread_stream,
};
use micromegas_tracing::event::EventSink;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::event::in_memory_sink::InMemorySink;
use micromegas_tracing::prelude::*;
use serial_test::serial;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

/// A function that deliberately causes thread parking through async sleep.
/// This allows us to test that the on_thread_park callback is properly
/// flushing events when threads become idle.
#[span_fn]
async fn park_inducing_function() {
    eprintln!("Starting async work that will cause thread parking");

    // This sleep should cause the thread to park, triggering on_thread_park callback
    sleep(Duration::from_millis(100)).await;

    eprintln!("Finished async work");
}

fn init_in_mem_tracing(sink: Arc<dyn EventSink>) {
    init_event_dispatch(1024, 1024, 1024, sink, HashMap::new()).unwrap();
}

/// Tests that the tokio runtime's on_thread_park callback properly flushes
/// tracing events when threads become idle. This ensures low-latency event
/// processing by not waiting for thread destruction to flush buffers.
#[test]
#[serial]
fn test_thread_park_flush() {
    let sink = Arc::new(InMemorySink::new());
    init_in_mem_tracing(sink.clone());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("park-test")
        .worker_threads(2) // Use just 2 threads to make parking more predictable
        .on_thread_start(|| {
            eprintln!("Thread started - initializing stream");
            init_thread_stream();
        })
        .on_thread_park(|| {
            eprintln!("Thread parking - flushing buffer");
            flush_thread_buffer();
        })
        .on_thread_stop(|| {
            eprintln!("Thread stopping - unregistering stream");
            unregister_thread_stream();
        })
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async {
        eprintln!("Spawning tasks that will cause thread parking");

        // Spawn multiple tasks to increase chance of thread parking
        let tasks = (0..4)
            .map(|i| {
                tokio::task::spawn(async move {
                    eprintln!("Task {} starting", i);
                    park_inducing_function().await;
                    eprintln!("Task {} finished", i);
                })
            })
            .collect::<Vec<_>>();

        // Wait for all tasks to complete
        for (i, task) in tasks.into_iter().enumerate() {
            task.await
                .unwrap_or_else(|e| panic!("Task {} failed: {}", i, e));
        }
    });

    // Drop the runtime to properly shut down worker threads
    drop(runtime);
    shutdown_dispatch();

    // Check that events were recorded
    let state = sink.state.lock().expect("Failed to lock sink state");
    let total_events: usize = state
        .thread_blocks
        .iter()
        .map(|block| block.nb_objects())
        .sum();

    eprintln!("Total events recorded: {}", total_events);

    // Each call to park_inducing_function should generate 2 events (begin + end span)
    // With 4 tasks, we expect at least 8 events
    assert!(
        total_events >= 8,
        "Expected at least 8 events from 4 function calls but found {}",
        total_events
    );

    unsafe { force_uninit() };
}
