use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::prelude::*;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use serial_test::serial;
use std::time::Duration;
use tokio::time::sleep;

/// An instrumented async function that awaits a short sleep, so the
/// resulting span runs across an `.await` point on a tokio worker thread.
#[span_fn]
async fn instrumented_async_work() {
    eprintln!("Starting instrumented async work");

    // The await point lets the task be rescheduled onto any worker thread.
    sleep(Duration::from_millis(100)).await;

    eprintln!("Finished async work");
}

/// Tests that span events recorded on tokio worker threads are flushed when
/// the runtime shuts down (worker threads stop). Events are buffered
/// per-thread and only flushed on `on_thread_stop`, which `drop(runtime)`
/// triggers here.
#[test]
#[serial]
fn test_worker_thread_span_flush() {
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
        eprintln!("Spawning tasks that exercise instrumented async work");

        // Spawn multiple tasks across the worker thread pool
        let tasks = (0..4)
            .map(|i| {
                tokio::task::spawn(async move {
                    eprintln!("Task {} starting", i);
                    instrumented_async_work().await;
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

    // Each call to instrumented_async_work should generate 2 events (begin + end span)
    // With 4 tasks, we expect at least 8 events
    assert!(
        total_events >= 8,
        "Expected at least 8 events from 4 function calls but found {}",
        total_events
    );
}
