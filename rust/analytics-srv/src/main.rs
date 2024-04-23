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

use anyhow::{Context, Result};
use axum::routing::post;
use axum::{Extension, Router};
use clap::Parser;
use micromegas::analytics::analytics_service::AnalyticsService;
use micromegas::ingestion::data_lake_connection::DataLakeConnection;
use micromegas::telemetry::blob_storage::BlobStorage;
use micromegas::telemetry_sink::TelemetryGuardBuilder;
use micromegas::tracing::prelude::*;
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[clap(name = "Analytics Server")]
#[clap(about = "Analytics Server", version, author)]
struct Cli {
    #[clap(long, default_value = "127.0.0.1:8082")]
    listen_endpoint: SocketAddr,
}

async fn query_processes_request(
    Extension(service): Extension<AnalyticsService>,
    _body: bytes::Bytes,
) {
    info!("query_processes_request");
    match service.query_processes(1024).await {
        Err(e) => {
            error!("Error in query_processes: {:?}", e);
        }

        Ok(_record_batch) => {
            info!("ok");
        }
    }
}

async fn serve_http(
    args: &Cli,
    lake: DataLakeConnection,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = AnalyticsService::new(lake);
    let app = Router::new()
        .route("/analytics/query_processes", post(query_processes_request))
        .layer(Extension(service));
    let listener = tokio::net::TcpListener::bind(args.listen_endpoint)
        .await
        .unwrap();
    info!("serving");
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

pub async fn connect_to_data_lake(
    db_uri: &str,
    object_store_url: &str,
) -> Result<DataLakeConnection> {
    info!("connecting to blob storage");
    let blob_storage = Arc::new(
        BlobStorage::connect(object_store_url).with_context(|| "connecting to blob storage")?,
    );
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(db_uri)
        .await
        .with_context(|| String::from("Connecting to telemetry database"))?;
    Ok(DataLakeConnection::new(pool, blob_storage))
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
    let data_lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    serve_http(&args, data_lake).await?;
    Ok(())
}
