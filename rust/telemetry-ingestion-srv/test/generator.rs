//! Generator test

use clap::Parser;
use micromegas::micromegas_main;
use micromegas::tracing::prelude::*;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Parser, Debug)]
#[command(name = "telemetry-generator")]
#[command(about = "Generates telemetry data including async spans")]
struct Args {
    /// Duration to run the generator in seconds
    #[arg(long, default_value = "3")]
    duration: u64,

    /// Number of async tasks to spawn
    #[arg(long, default_value = "5")]
    async_tasks: usize,

    /// Number of threads for concurrent operations
    #[arg(long, default_value = "4")]
    threads: usize,
}

// Sync span function example
#[span_fn]
fn sync_operation(value: i32) -> i32 {
    info!("performing sync operation with value: {value}");
    std::thread::sleep(Duration::from_millis(50));
    value * 2
}

// Async span function example
#[span_fn]
async fn async_operation(delay_ms: u64) -> String {
    info!("starting async operation with {delay_ms}ms delay");
    sleep(Duration::from_millis(delay_ms)).await;
    format!("completed after {delay_ms}ms")
}

// Nested async function
#[span_fn]
async fn nested_async_operation() {
    let result1 = async_operation(100).await;
    info!("first async result: {result1}");

    let result2 = async_operation(150).await;
    info!("second async result: {result2}");
}

// Manual span instrumentation example
async fn manual_async_work() {
    static_span_desc!(WORK_DESC, "manual_work");

    async {
        info!("doing manual async work");
        sleep(Duration::from_millis(75)).await;
        info!("manual work completed");
    }
    .instrument(&WORK_DESC)
    .await;
}

// Multi-threaded async function that creates spans across different threads
#[span_fn]
async fn multi_threaded_async_work(task_id: usize) -> String {
    info!("starting multi-threaded task {}", task_id);

    // Create nested async work that may be scheduled on different threads
    let subtask1 = async_subtask(task_id, "validation").await;
    let subtask2 = async_subtask(task_id, "computation").await;

    // Simulate some async I/O that can cause thread migration
    sleep(Duration::from_millis(30 + (task_id as u64 * 10))).await;

    info!("multi-threaded task {} completed", task_id);
    format!("task_{}_result: {} + {}", task_id, subtask1, subtask2)
}

// Async subtask that can be scheduled on any thread
#[span_fn]
async fn async_subtask(task_id: usize, operation: &str) -> String {
    info!("executing subtask {} for task {}", operation, task_id);

    // Variable delay to create different scheduling patterns
    let delay = 20 + (task_id as u64 * 5);
    sleep(Duration::from_millis(delay)).await;

    format!("{}_{}", operation, task_id)
}

// Concurrent async task with potential for work stealing
#[span_fn]
async fn concurrent_async_task(operation: &str, base_delay: u64) -> String {
    info!("starting concurrent operation: {}", operation);

    // Create parent-child async span relationship
    let preparation = prepare_async_operation(operation).await;
    let execution = execute_async_operation(operation, base_delay).await;
    let cleanup = cleanup_async_operation(operation).await;

    info!("concurrent operation {} completed", operation);
    format!(
        "{}: {} -> {} -> {}",
        operation, preparation, execution, cleanup
    )
}

#[span_fn]
async fn prepare_async_operation(operation: &str) -> String {
    info!("preparing {}", operation);
    sleep(Duration::from_millis(15)).await;
    format!("prepared_{}", operation)
}

#[span_fn]
async fn execute_async_operation(operation: &str, delay: u64) -> String {
    info!("executing {} with {}ms delay", operation, delay);
    sleep(Duration::from_millis(delay)).await;
    format!("executed_{}", operation)
}

#[span_fn]
async fn cleanup_async_operation(operation: &str) -> String {
    info!("cleaning up {}", operation);
    sleep(Duration::from_millis(10)).await;
    format!("cleaned_{}", operation)
}

#[micromegas_main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    info!(
        "hello from generator - creating multi-threaded async spans (duration={}s, async_tasks={}, threads={})",
        args.duration, args.async_tasks, args.threads
    );

    // Generate metrics
    imetric!("Frame Time", "ticks", 1000);
    fmetric!("Frame Time", "ticks", 1.0);
    imetric!("Memory Usage", "bytes", 2048);
    fmetric!("CPU Usage", "percent", 75.5);

    // Generate sync span events
    let sync_result = sync_operation(42);
    info!("sync operation result: {sync_result}");

    // Generate async span events
    let async_result = async_operation(200).await;
    info!("async operation result: {async_result}");

    // Generate nested async spans
    nested_async_operation().await;

    // Generate manual instrumentation spans
    manual_async_work().await;

    // Generate multi-threaded async spans to create cross-stream async events
    info!("starting multi-threaded async operations");

    let mut handles = vec![];

    // Create multiple async tasks that run on different threads (using args.async_tasks)
    for i in 0..args.async_tasks {
        let handle = tokio::spawn(async move { multi_threaded_async_work(i).await });
        handles.push(handle);
    }

    // Create concurrent async tasks with work-stealing potential (using args.threads)
    let operations = [
        "database_query",
        "api_call",
        "file_processing",
        "cache_lookup",
        "network_request",
        "disk_io",
    ];
    let concurrent_tasks: Vec<_> = (0..args.threads)
        .map(|i| {
            let op_name = operations[i % operations.len()];
            let base_delay = 60 + (i as u64 * 20);
            tokio::spawn(concurrent_async_task(op_name, base_delay))
        })
        .collect();

    // Wait for all tasks to complete
    for handle in handles {
        if let Ok(result) = handle.await {
            info!("multi-threaded task completed: {}", result);
        }
    }

    for task in concurrent_tasks {
        if let Ok(result) = task.await {
            info!("concurrent task result: {}", result);
        }
    }

    // Run for the specified duration
    info!(
        "running for {} seconds to generate continuous telemetry",
        args.duration
    );
    tokio::time::sleep(Duration::from_secs(args.duration)).await;

    info!("generator completed successfully with multi-threaded async spans");
    Ok(())
}
