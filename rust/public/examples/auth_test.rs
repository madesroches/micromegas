//! Test program to verify automatic authentication configuration
//!
//! This example demonstrates the automatic authentication feature of micromegas_main.
//! Run with different environment variables to test different authentication methods:
//!
//! # No authentication (default)
//! ```bash
//! cargo run --example auth_test
//! ```
//!
//! # API key authentication
//! ```bash
//! MICROMEGAS_INGESTION_API_KEY=test-key-123 \
//! MICROMEGAS_TELEMETRY_URL=http://localhost:9000 \
//! cargo run --example auth_test
//! ```
//!
//! # OIDC client credentials authentication
//! ```bash
//! MICROMEGAS_OIDC_TOKEN_ENDPOINT=https://accounts.google.com/o/oauth2/token \
//! MICROMEGAS_OIDC_CLIENT_ID=my-service@project.iam.gserviceaccount.com \
//! MICROMEGAS_OIDC_CLIENT_SECRET=secret-from-manager \
//! MICROMEGAS_TELEMETRY_URL=http://localhost:9000 \
//! cargo run --example auth_test
//! ```

use micromegas::micromegas_main;
use micromegas::tracing::prelude::*;

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    info!("Auth test starting");
    info!("Check the startup logs for authentication configuration");

    // Log some test events
    for i in 0..5 {
        info!("Test event {}", i);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    info!("Auth test complete - check ingestion server logs if MICROMEGAS_TELEMETRY_URL was set");

    // Give time for telemetry to flush
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    Ok(())
}
