//! Telemetry Ingestion Server
//!
//! Accepts telemetry data through grpc, stores the metadata in sqlite and the
//! raw event payload in local binary files.
//!
//! Env variables:
//!  - `LEGION_TELEMETRY_INGESTION_SRC_DATA_DIRECTORY` : local directory where
//!    data will be dumped

// crate-specific lint exceptions:
//#![allow()]

mod data_lake_connection;
mod local_data_lake;
mod remote_data_lake;
mod sql_migration;
mod sql_telemetry_db;
mod web_ingestion_service;

use anyhow::Result;
use axum::extract::DefaultBodyLimit;
use axum::routing::post;
use axum::Extension;
use axum::Json;
use axum::Router;
use clap::{Parser, Subcommand};
use data_lake_connection::DataLakeConnection;
use local_data_lake::connect_to_local_data_lake;
use remote_data_lake::connect_to_remote_data_lake;
use std::net::SocketAddr;
use std::path::PathBuf;
use telemetry_sink::stream_info::StreamInfo;
use telemetry_sink::TelemetryGuardBuilder;
use tower_http::limit::RequestBodyLimitLayer;
use tracing::prelude::*;
use web_ingestion_service::WebIngestionService;

#[derive(Parser, Debug)]
#[clap(name = "Legion Telemetry Ingestion Server")]
#[clap(about = "Legion Telemetry Ingestion Server", version, author)]
#[clap(arg_required_else_help(true))]
struct Cli {
    #[clap(long, default_value = "0.0.0.0:8081")]
    listen_endpoint_http: SocketAddr,

    #[clap(subcommand)]
    spec: DataLakeSpec,
}

#[derive(Subcommand, Debug)]
enum DataLakeSpec {
    Local { path: PathBuf },
    Remote { db_uri: String, s3_url: String },
}

async fn insert_process_request(
    Extension(service): Extension<WebIngestionService>,
    Json(body): Json<serde_json::Value>,
) {
    info!("insert_process_request");
    if let Err(e) = service.insert_process(body).await {
        error!("Error in insert_process_request: {:?}", e);
    }
}

async fn insert_stream_request(
    Extension(service): Extension<WebIngestionService>,
    Json(stream_info): Json<StreamInfo>,
) {
    info!("insert_stream_request");
    if let Err(e) = service.insert_stream(stream_info).await {
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
        .layer(Extension(service));
    let listener = tokio::net::TcpListener::bind(args.listen_endpoint_http)
        .await
        .unwrap();
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
    let data_lake = match &args.spec {
        DataLakeSpec::Local { path } => connect_to_local_data_lake(path.clone()).await?,
        DataLakeSpec::Remote { db_uri, s3_url } => {
            connect_to_remote_data_lake(db_uri, s3_url).await?
        }
    };
    serve_http(&args, data_lake).await?;
    Ok(())
}
