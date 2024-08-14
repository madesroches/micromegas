//! Telemetry Ingestion Server
//!
//! Accepts telemetry data through http, stores the metadata in postgresql and the
//! raw event payload in the object store.
//!
//! Env variables:
//!  - `MICROMEGAS_SQL_CONNECTION_STRING` : to connect to postgresql
//!  - `MICROMEGAS_OBJECT_STORE_URI` : to write the payloads

use anyhow::{Context, Result};
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::Extension;
use axum::Router;
use clap::Parser;
use micromegas::axum_utils::observability_middleware;
use micromegas::ingestion::data_lake_connection::DataLakeConnection;
use micromegas::ingestion::remote_data_lake::connect_to_remote_data_lake;
use micromegas::ingestion::web_ingestion_service::WebIngestionService;
use micromegas::servers;
use micromegas::telemetry_sink::TelemetryGuardBuilder;
use micromegas::tracing::prelude::*;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::limit::RequestBodyLimitLayer;

#[derive(Parser, Debug)]
#[clap(name = "Telemetry Ingestion Server")]
#[clap(about = "Telemetry Ingestion Server", version, author)]
struct Cli {
    #[clap(long, default_value = "127.0.0.1:8081")]
    listen_endpoint_http: SocketAddr,
}

async fn serve_http(
    args: &Cli,
    lake: DataLakeConnection,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = Arc::new(WebIngestionService::new(lake));

    let app = servers::ingestion::register_routes(Router::new())
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(100 * 1024 * 1024))
        .layer(Extension(service))
        .layer(middleware::from_fn(observability_middleware));
    let listener = tokio::net::TcpListener::bind(args.listen_endpoint_http)
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
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let data_lake = connect_to_remote_data_lake(&connection_string, &object_store_uri).await?;
    serve_http(&args, data_lake).await?;
    Ok(())
}
