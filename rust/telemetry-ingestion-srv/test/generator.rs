//! Generator test

use micromegas::telemetry_sink::TelemetryGuard;
use micromegas::tracing::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _telemetry_guard = TelemetryGuard::new().unwrap();
    info!("hello from generator");
    imetric!("Frame Time", "ticks", 1000);
    fmetric!("Frame Time", "ticks", 1.0);
    Ok(())
}
