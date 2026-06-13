#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use clap::Parser;
use micromegas::micromegas_main;
use micromegas::servers::flight_sql_server::FlightSqlServer;
use std::time::Duration;

#[derive(Parser, Debug)]
#[clap(name = "Micromegas FlightSQL server")]
#[clap(about = "Micromegas FlightSQL server", version, author)]
struct Cli {
    #[clap(long)]
    disable_auth: bool,

    /// Seconds to wait for in-flight RPCs to complete after SIGTERM
    #[clap(
        long,
        default_value = "25",
        env = "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS"
    )]
    shutdown_grace_period_seconds: u64,
}

#[micromegas_main(interop_max_level = "info", max_level_override = "debug")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();

    let mut builder = FlightSqlServer::builder()
        .with_shutdown_grace(Duration::from_secs(args.shutdown_grace_period_seconds));

    if !args.disable_auth {
        builder = builder.with_default_auth();
    }

    builder.build_and_serve().await?;
    Ok(())
}
