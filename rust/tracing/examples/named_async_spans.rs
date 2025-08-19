//! Example demonstrating named async span instrumentation
//!
//! This example shows how to use the new named async span functionality
//! to create async spans with different names for similar operations.

use micromegas_tracing::event::NullEventSink;
use micromegas_tracing::guards::TracingSystemGuard;
use micromegas_tracing::intern_string::intern_string;
use micromegas_tracing::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{Duration, sleep};

async fn process_user(user_id: u32) -> String {
    // Simulate some async work
    sleep(Duration::from_millis(100)).await;
    format!("Processed user {}", user_id)
}

async fn database_operation(query: &'static str) -> u32 {
    sleep(Duration::from_millis(50)).await;
    query.len() as u32
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing (in a real app you'd use a proper sink)
    let _tracing_guard = TracingSystemGuard::new(
        1024 * 1024,
        1024 * 1024,
        1024 * 1024,
        Arc::new(NullEventSink {}),
        HashMap::new(),
    )?;

    println!("Starting named async spans example...");

    // Example 1: Using the convenient span_async_named! macro
    let result1 = span_async_named!("user_registration", async { process_user(123).await }).await;
    println!("Result 1: {}", result1);

    // Example 2: Multiple operations with different names
    let result2 = span_async_named!("user_authentication", async { process_user(456).await }).await;
    println!("Result 2: {}", result2);

    // Example 3: Database operations with descriptive names
    let count = span_async_named!("select_users_query", async {
        database_operation("SELECT * FROM users").await
    })
    .await;
    println!("Query returned {} results", count);

    // Example 4: Using the lower-level instrument_named! macro
    let result4 = instrument_named!(process_user(789), "background_user_sync").await;
    println!("Result 4: {}", result4);

    // Example 5: Using interned strings for dynamic span names in a loop
    println!("Processing multiple batches...");
    for batch_id in 1..=3 {
        let span_name = intern_string(&format!("process_batch_{}", batch_id));
        span_async_named!(span_name, async move {
            sleep(Duration::from_millis(30)).await;
            println!("Processed batch {}", batch_id);
            batch_id
        })
        .await;
    }

    // Example 6: Nested named spans
    span_async_named!("user_onboarding_flow", async {
        println!("Starting user onboarding...");

        let _auth = span_async_named!("verify_credentials", async {
            sleep(Duration::from_millis(30)).await;
            "authenticated"
        })
        .await;

        let _profile = span_async_named!("create_profile", async {
            sleep(Duration::from_millis(40)).await;
            "profile created"
        })
        .await;

        let _welcome = span_async_named!("send_welcome_email", async {
            sleep(Duration::from_millis(20)).await;
            "email sent"
        })
        .await;

        "onboarding complete"
    })
    .await;

    println!("Named async spans example completed!");
    Ok(())
}
