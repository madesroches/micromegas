//! Telemetry maintenance daemon

#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::Result;
use clap::Parser;
use micromegas::analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas::analytics::lakehouse::view_factory::default_view_factory;
use micromegas::micromegas_main;
use micromegas::servers::maintenance::{daemon, get_global_views_with_update_group};
use micromegas::servers::shutdown::wait_for_sigterm;
use std::time::Duration;

#[derive(Parser, Debug)]
#[clap(name = "Micromegas Telemetry Maintenance")]
#[clap(
    about = "Maintenance daemon for a Micromegas telemetry data lake",
    version,
    author
)]
struct Cli {
    /// Seconds to wait for in-flight tasks to complete after SIGTERM
    #[clap(
        long,
        default_value = "25",
        env = "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS"
    )]
    shutdown_grace_period_seconds: u64,

    /// Delete lake data older than this many days (retention horizon)
    #[clap(long, default_value = "90", env = "MICROMEGAS_RETENTION_DAYS")]
    retention_days: i32,
}

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let lakehouse = LakehouseContext::from_env().await?;
    let data_lake = lakehouse.lake().clone();
    let view_factory = default_view_factory(lakehouse.runtime().clone(), data_lake.clone()).await?;
    let views_to_update = get_global_views_with_update_group(&view_factory);
    let grace = Duration::from_secs(args.shutdown_grace_period_seconds);
    daemon(
        lakehouse,
        views_to_update,
        args.retention_days,
        wait_for_sigterm(),
        grace,
    )
    .await
}
