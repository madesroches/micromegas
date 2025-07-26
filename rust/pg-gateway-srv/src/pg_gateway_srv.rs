mod api_error;
mod extended;
mod factory;
mod simple;
mod startup;
mod state;
use clap::Parser;
use micromegas::{
    datafusion_postgres::pgwire,
    telemetry_sink::TelemetryGuardBuilder,
    tracing::{debug, error, info, levels::LevelFilter},
};
use pgwire::tokio::{process_socket, tokio_rustls::rustls};
use std::net::SocketAddr;
use std::{fmt::Debug, sync::Arc};
use tokio::net::TcpListener;

#[derive(Parser, Debug)]
#[clap(name = "pg-gateway-srv")]
#[clap(about = "Postgresql->Micromegas gateway server", version, author)]
struct Cli {
    #[clap(long, default_value = "0.0.0.0:8432")]
    listen_endpoint_tcp: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_ctrlc_handling()
        .with_local_sink_max_level(LevelFilter::Info)
        .build();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let args = Cli::parse();
    let listener = TcpListener::bind(args.listen_endpoint_tcp).await?;
    info!("Listening to {}", args.listen_endpoint_tcp);
    loop {
        let incoming_socket = listener.accept().await?;
        debug!("incoming_socket = {incoming_socket:?}");
        let factory = Arc::new(factory::HandlerFactory::new());
        tokio::spawn(async move {
            if let Err(e) = process_socket(incoming_socket.0, None, factory).await {
                error!("process_socket: {e:?}");
            }
            info!("done processing socket");
        });
    }
}
