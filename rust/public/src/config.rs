//! CLI args shared by every long-running server binary.

use std::time::Duration;

/// CLI args shared by every long-running server binary.
/// Flatten into a binary's clap struct with `#[command(flatten)]`.
#[derive(clap::Args, Debug, Clone)]
pub struct CommonServerArgs {
    /// Seconds to wait for in-flight work to complete after SIGTERM.
    #[arg(
        long,
        default_value = "25",
        env = "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS"
    )]
    pub shutdown_grace_period_seconds: u64,
}

impl CommonServerArgs {
    pub fn grace(&self) -> Duration {
        Duration::from_secs(self.shutdown_grace_period_seconds)
    }
}
