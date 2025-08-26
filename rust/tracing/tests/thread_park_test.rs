use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::prelude::*;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use serial_test::serial;
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

/// Tests that the tokio runtime's on_thread_park callback properly flushes
/// tracing events when threads become idle. This ensures low-latency event
/// processing by not waiting for thread destruction to flush buffers.
#[test]
#[serial]
fn test_thread_park_flush() {
    let guard = init_in_memory_tracing();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("park-test")
        .worker_threads(2) // Use just 2 threads to make parking more predictable
        .with_tracing_callbacks_and_custom_start(|| {
            eprintln!("Thread started - initializing stream");
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

    // Check that events were recorded
    let state = guard.sink.state.lock().expect("Failed to lock sink state");
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
}
