//! Generator test

use micromegas::micromegas_main;
use micromegas::tracing::prelude::*;
use std::time::Duration;
use tokio::time::sleep;

// Sync span function example
#[span_fn]
fn sync_operation(value: i32) -> i32 {
    info!("performing sync operation with value: {}", value);
    std::thread::sleep(Duration::from_millis(50));
    value * 2
}

// Async span function example
#[span_fn]
async fn async_operation(delay_ms: u64) -> String {
    info!("starting async operation with {}ms delay", delay_ms);
    sleep(Duration::from_millis(delay_ms)).await;
    format!("completed after {}ms", delay_ms)
}

// Nested async function
#[span_fn]
async fn nested_async_operation() {
    let result1 = async_operation(100).await;
    info!("first async result: {}", result1);

    let result2 = async_operation(150).await;
    info!("second async result: {}", result2);
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

#[micromegas_main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    info!("hello from generator");

    // Generate metrics
    imetric!("Frame Time", "ticks", 1000);
    fmetric!("Frame Time", "ticks", 1.0);
    imetric!("Memory Usage", "bytes", 2048);
    fmetric!("CPU Usage", "percent", 75.5);

    // Generate sync span events
    let sync_result = sync_operation(42);
    info!("sync operation result: {}", sync_result);

    // Generate async span events
    let async_result = async_operation(200).await;
    info!("async operation result: {}", async_result);

    // Generate nested async spans
    nested_async_operation().await;

    // Generate manual instrumentation spans
    manual_async_work().await;

    info!("generator completed successfully");
    Ok(())
}
