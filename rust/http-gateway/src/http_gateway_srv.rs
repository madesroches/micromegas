use std::net::SocketAddr;

use anyhow::Result;
use axum::Router;
use clap::Parser;
use micromegas::servers;
use micromegas::telemetry_sink::TelemetryGuardBuilder;
use micromegas::tracing::levels::LevelFilter;
use micromegas::{axum, tracing::info};

#[derive(Parser, Debug)]
#[clap(name = "Micromegas http gateway server")]
#[clap(about = "Micromegas http gateway server", version, author)]
struct Cli {
    #[clap(long, default_value = "0.0.0.0:3000")]
    listen_endpoint_http: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_ctrlc_handling()
        .with_local_sink_max_level(LevelFilter::Debug)
        .with_interop_max_level_override(LevelFilter::Info)
        .build();
    let args = Cli::parse();
    let app = servers::http_gateway::register_routes(Router::new());
    let listener = tokio::net::TcpListener::bind(args.listen_endpoint_http).await?;
    info!("Server running on {}", args.listen_endpoint_http);
    axum::serve(listener, app).await?;
    Ok(())
}
