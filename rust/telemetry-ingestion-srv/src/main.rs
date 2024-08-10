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
use axum::routing::post;
use axum::Extension;
use axum::Router;
use clap::Parser;
use micromegas::axum_utils::observability_middleware;
use micromegas::ingestion::data_lake_connection::DataLakeConnection;
use micromegas::ingestion::remote_data_lake::connect_to_remote_data_lake;
use micromegas::ingestion::web_ingestion_service::WebIngestionService;
use micromegas::telemetry_sink::TelemetryGuardBuilder;
use micromegas::tracing::prelude::*;
use std::net::SocketAddr;
use tower_http::limit::RequestBodyLimitLayer;

#[derive(Parser, Debug)]
#[clap(name = "Telemetry Ingestion Server")]
#[clap(about = "Telemetry Ingestion Server", version, author)]
struct Cli {
    #[clap(long, default_value = "127.0.0.1:8081")]
    listen_endpoint_http: SocketAddr,
}

async fn insert_process_request(
    Extension(service): Extension<WebIngestionService>,
    body: bytes::Bytes,
) {
    if let Err(e) = service.insert_process(body).await {
        error!("Error in insert_process_request: {:?}", e);
    }
}

async fn insert_stream_request(
    Extension(service): Extension<WebIngestionService>,
    body: bytes::Bytes,
) {
    if let Err(e) = service.insert_stream(body).await {
        error!("Error in insert_stream_request: {:?}", e);
    }
}

async fn insert_block_request(
    Extension(service): Extension<WebIngestionService>,
    body: bytes::Bytes,
) {
    if body.is_empty() {
        error!("insert_block_request: empty body");
        return;
    }
    if let Err(e) = service.insert_block(body).await {
        error!("Error in insert_block_request: {:?}", e);
    }
}

async fn serve_http(
    args: &Cli,
    lake: DataLakeConnection,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = WebIngestionService::new(lake);

    let app = Router::new()
        .route("/ingestion/insert_process", post(insert_process_request))
        .route("/ingestion/insert_stream", post(insert_stream_request))
        .route("/ingestion/insert_block", post(insert_block_request))
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
