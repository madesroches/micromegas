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
mod grpc_ingestion_service;
mod local_data_lake;
mod remote_data_lake;
mod sql_migration;
mod sql_telemetry_db;
mod web_ingestion_service;

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use clap::{Parser, Subcommand};
use data_lake_connection::DataLakeConnection;
use grpc_ingestion_service::GRPCIngestionService;
use lgn_telemetry_proto::ingestion::telemetry_ingestion_server::TelemetryIngestionServer;
use local_data_lake::connect_to_local_data_lake;
use remote_data_lake::connect_to_remote_data_lake;
use std::net::SocketAddr;
use telemetry_sink::TelemetryGuardBuilder;
use tonic::transport::Server;
use tower_http::auth::AsyncRequireAuthorizationLayer;
use tracing::prelude::*;
use warp::Filter;
use web_ingestion_service::WebIngestionService;

#[derive(Parser, Debug)]
#[clap(name = "Legion Telemetry Ingestion Server")]
#[clap(about = "Legion Telemetry Ingestion Server", version, author)]
#[clap(arg_required_else_help(true))]
struct Cli {
    #[clap(long, default_value = "0.0.0.0:8080")]
    listen_endpoint: SocketAddr, //grpc

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

async fn serve_grpc(
    args: &Cli,
    lake: DataLakeConnection,
) -> Result<(), Box<dyn std::error::Error>> {
    // To enable AWS DynamoDb API key validation, uncomment the following (and possibly adapt the name of the DynamoDb table):
    //let validation = Arc::new(lgn_auth::api_key::TtlCacheValidation::new(
    //    lgn_auth::api_key::AwsDynamoDbValidation::new(None, "legionlabs-telemetry-api-keys")
    //        .await?,
    //    10,                                  // Hold up to 10 API keys in memory.
    //    std::time::Duration::from_secs(600), // Hold them for 10 minutes.
    //));

    // This validates against an in-memory API key.
    // In a real world scenario, you would want to read the API key from the
    // environment at runtime, but for now we'll hardcode it during compilation.
    //let api_key = std::env::var("LGN_TELEMETRY_GRPC_API_KEY")?.into();
    let api_key = env!("LGN_TELEMETRY_GRPC_API_KEY").to_string().into();
    let validation = Arc::new(lgn_auth::api_key::MemoryValidation::new(vec![api_key]));

    let auth_layer =
        AsyncRequireAuthorizationLayer::new(lgn_auth::api_key::RequestAuthorizer::new(validation));

    let layer = tower::ServiceBuilder::new() //todo: compose with cors layer
        .layer(auth_layer)
        .into_inner();

    let service = GRPCIngestionService::new(lake);

    Server::builder()
        .layer(layer)
        .add_service(TelemetryIngestionServer::new(service))
        .serve(args.listen_endpoint)
        .await?;
    Ok(())
}

fn with_service(
    service: WebIngestionService,
) -> impl Filter<Extract = (WebIngestionService,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || service.clone())
}

async fn insert_process_request(
    service: WebIngestionService,
    body: serde_json::value::Value,
) -> Result<warp::reply::Response, warp::Rejection> {
    if let Err(e) = service.insert_process(body).await {
        error!("Error in insert_process_request: {:?}", e);
        Ok(http::response::Response::builder()
            .status(500)
            .body(hyper::body::Body::from("Error in insert_process_request"))
            .unwrap())
    } else {
        Ok(http::response::Response::builder()
            .status(200)
            .body(hyper::body::Body::from("OK"))
            .unwrap())
    }
}

async fn insert_stream_request(
    service: WebIngestionService,
    body: serde_json::value::Value,
) -> Result<warp::reply::Response, warp::Rejection> {
    if let Err(e) = service.insert_stream(body).await {
        error!("Error in insert_stream_request: {:?}", e);
        Ok(http::response::Response::builder()
            .status(500)
            .body(hyper::body::Body::from("Error in insert_process_request"))
            .unwrap())
    } else {
        Ok(http::response::Response::builder()
            .status(200)
            .body(hyper::body::Body::from("OK"))
            .unwrap())
    }
}

async fn insert_block_request(
    service: WebIngestionService,
    body: bytes::Bytes,
) -> Result<warp::reply::Response, warp::Rejection> {
    if let Err(e) = service.insert_block(body).await {
        error!("Error in insert_block_request: {:?}", e);
        Ok(http::response::Response::builder()
            .status(500)
            .body(hyper::body::Body::from("Error in insert_block_request"))
            .unwrap())
    } else {
        Ok(http::response::Response::builder()
            .status(200)
            .body(hyper::body::Body::from("OK"))
            .unwrap())
    }
}

async fn serve_http(
    args: &Cli,
    lake: DataLakeConnection,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = WebIngestionService::new(lake);
    let web_ingestion_filter =
        warp::path!("v1" / "spaces" / "default" / "telemetry" / "ingestion" / ..)
            .and(with_service(service));

    let insert_process_filter = web_ingestion_filter
        .clone()
        .and(warp::path("process"))
        .and(warp::body::json())
        .and_then(insert_process_request);

    let insert_stream_filter = web_ingestion_filter
        .clone()
        .and(warp::path("stream"))
        .and(warp::body::json())
        .and_then(insert_stream_request);

    let insert_block_filter = web_ingestion_filter
        .and(warp::path("block"))
        .and(warp::body::bytes())
        .and_then(insert_block_request);

    let routes = warp::put().and(
        insert_process_filter
            .or(insert_stream_filter)
            .or(insert_block_filter),
    );

    warp::serve(routes).run(args.listen_endpoint_http).await;
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
    tokio::select! {
        _ = serve_grpc(&args, data_lake.clone()) => {
        },
        _ = serve_http(&args, data_lake) => {
        }
    }
    Ok(())
}
