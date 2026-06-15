#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use clap::Parser;
use micromegas::micromegas_main;
use micromegas::servers::flight_sql_server::FlightSqlServer;
use std::net::SocketAddr;
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

    /// Optional address for the HTTP health/readiness sidecar (e.g. 127.0.0.1:8082)
    #[clap(long)]
    health_listen_addr: Option<SocketAddr>,
}

#[micromegas_main(interop_max_level = "info", max_level_override = "debug")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();

    let mut builder = FlightSqlServer::builder()
        .with_shutdown_grace(Duration::from_secs(args.shutdown_grace_period_seconds));

    if !args.disable_auth {
        builder = builder.with_default_auth();
    }

    if let Some(addr) = args.health_listen_addr {
        builder = builder.with_health_addr(addr);
    }

    builder.build_and_serve().await?;
    Ok(())
}
