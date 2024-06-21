//! Analytics Server
//!
//! Feeds data to the analytics-web interface.
//!
//! Env variables:
//!  - `MICROMEGAS_SQL_CONNECTION_STRING` : postgresql server
//!  - `MICROMEGAS_OBJECT_STORE_URI` : payloads, partitions

use anyhow::{Context, Result};
use axum::response::Response;
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

fn bytes_response(result: Result<bytes::Bytes>) -> Response {
    match result {
        Err(e) => {
            error!("Error in request: {e:?}");
            Response::builder()
                .status(500)
                .body(format!("{e:?}").into())
                .unwrap()
        }
        Ok(bytes) => Response::builder().status(200).body(bytes.into()).unwrap(),
    }
}

async fn find_process_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    info!("find_process_request");
    bytes_response(
        service
            .find_process(body)
            .await
            .with_context(|| "find_process"),
    )
}

async fn query_processes_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    info!("query_processes_request");
    bytes_response(
        service
            .query_processes(body)
            .await
            .with_context(|| "query_processes"),
    )
}

async fn query_streams_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    info!("query_streams_request");
    bytes_response(
        service
            .query_streams(body)
            .await
            .with_context(|| "query_streams"),
    )
}

async fn query_blocks_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    info!("query_blocks_request");
    bytes_response(
        service
            .query_blocks(body)
            .await
            .with_context(|| "query_blocks"),
    )
}

async fn query_spans_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    info!("query_spans_request");
    bytes_response(
        service
            .query_spans(body)
            .await
            .with_context(|| "query_spans"),
    )
}

async fn query_thread_events_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    info!("query_thread_events_request");
    bytes_response(
        service
            .query_thread_events(body)
            .await
            .with_context(|| "query_thread_events"),
    )
}

async fn query_log_entries_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    info!("query_log_entries_request");
    bytes_response(
        service
            .query_log_entries(body)
            .await
            .with_context(|| "query_log_entries"),
    )
}

async fn query_metrics_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    info!("query_metrics_request");
    bytes_response(
        service
            .query_metrics(body)
            .await
            .with_context(|| "query_metrics"),
    )
}

async fn serve_http(
    args: &Cli,
    lake: DataLakeConnection,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = AnalyticsService::new(lake);
    let app = Router::new()
        .route("/analytics/find_process", post(find_process_request))
        .route("/analytics/query_processes", post(query_processes_request))
        .route("/analytics/query_streams", post(query_streams_request))
        .route("/analytics/query_blocks", post(query_blocks_request))
        .route("/analytics/query_spans", post(query_spans_request))
        .route(
            "/analytics/query_log_entries",
            post(query_log_entries_request),
        )
        .route("/analytics/query_metrics", post(query_metrics_request))
        .route(
            "/analytics/query_thread_events",
            post(query_thread_events_request),
        )
        .layer(Extension(service));
    let listener = tokio::net::TcpListener::bind(args.listen_endpoint)
        .await
        .unwrap();
    info!("serving on {}", &args.listen_endpoint);
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
