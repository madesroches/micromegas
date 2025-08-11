//! Generator test

use micromegas::micromegas_main;
use micromegas::tracing::prelude::*;

#[micromegas_main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    info!("hello from generator");
    imetric!("Frame Time", "ticks", 1000);
    fmetric!("Frame Time", "ticks", 1.0);
    Ok(())
}
