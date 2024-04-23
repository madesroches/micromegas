//! Analytics Server
//!
//! Feeds data to the analytics-web interface.
//!
//! Env variables:
//!  - `MICROMEGAS_SQL_CONNECTION_STRING` : postgresql server
//!  - `MICROMEGAS_OBJECT_STORE_URI` : payloads, partitions

// mod analytics_service;
// mod auth;
// mod cache;
// mod call_tree;
// mod cumulative_call_graph;
// mod cumulative_call_graph_handler;
// mod cumulative_call_graph_node;
// mod lakehouse;
// mod log_entry;
// mod metrics;
// mod scope;
// mod thread_block_processor;

use anyhow::Result;
use axum::Router;
use clap::Parser;
use micromegas_telemetry_sink::TelemetryGuardBuilder;
use micromegas_tracing::prelude::*;
use std::net::SocketAddr;

#[derive(Parser, Debug)]
#[clap(name = "Analytics Server")]
#[clap(about = "Analytics Server", version, author)]
struct Cli {
    #[clap(long, default_value = "127.0.0.1:8082")]
    listen_endpoint: SocketAddr,
}

async fn serve_http(args: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new();
    let listener = tokio::net::TcpListener::bind(args.listen_endpoint)
        .await
        .unwrap();
    info!("serving");
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_ctrlc_handling()
        .with_local_sink_max_level(LevelFilter::Debug)
        .build();
    let args = Cli::parse();
    serve_http(&args).await?;
    Ok(())
}
