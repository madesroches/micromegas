//! Analytics Server
//!
//! Feeds data to the analytics-web interface.
//!
//! Env variables:
//!  - `MICROMEGAS_SQL_CONNECTION_STRING` : postgresql server
//!  - `MICROMEGAS_OBJECT_STORE_URI` : payloads, partitions

use anyhow::{Context, Result};
use axum::middleware;
use axum::response::Response;
use axum::routing::post;
use axum::{Extension, Router};
use clap::Parser;
use micromegas::analytics::analytics_service::AnalyticsService;
use micromegas::analytics::lakehouse::migration::migrate_lakehouse;
use micromegas::analytics::lakehouse::view_factory::ViewFactory;
use micromegas::analytics::response_writer::ResponseWriter;
use micromegas::ingestion::data_lake_connection::{connect_to_data_lake, DataLakeConnection};
use micromegas::telemetry_sink::TelemetryGuardBuilder;
use micromegas::tracing::prelude::*;
use micromegas_axum_utils::{make_body_from_channel_receiver, observability_middleware};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::timeout::TimeoutLayer;

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
    bytes_response(
        service
            .query_metrics(body)
            .await
            .with_context(|| "query_metrics"),
    )
}

async fn query_view_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(service.query_view(body).await.with_context(|| "query_view"))
}

async fn query_partitions_request(Extension(service): Extension<AnalyticsService>) -> Response {
    bytes_response(
        service
            .query_partitions()
            .await
            .with_context(|| "query_partitions"),
    )
}

async fn create_or_update_partitions_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let response_body = make_body_from_channel_receiver(rx);
    let writer = Arc::new(ResponseWriter::new(Some(tx)));
    tokio::spawn(async move {
        if let Err(e) = service
            .create_or_update_partitions(body, writer.clone())
            .await
            .with_context(|| "create_or_update_partitions")
        {
            if writer.is_closed() {
                info!("Error happened, but connection is closed: {e:?}");
            } else {
                // the connection is live, this looks like a real error
                error!("{e:?}");
                if let Err(e) = writer.write_string(&format!("error: {e:?}")).await {
                    //error writing can happen, probably not a big deal
                    info!("{e:?}");
                }
            }
        }
    });
    Response::builder().status(200).body(response_body).unwrap()
}

async fn merge_partitions_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(
        service
            .merge_partitions(body)
            .await
            .with_context(|| "merge_partitions"),
    )
}

async fn retire_partitions_request(
    Extension(service): Extension<AnalyticsService>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(
        service
            .retire_partitions(body)
            .await
            .with_context(|| "retire_partitions"),
    )
}

async fn serve_http(
    args: &Cli,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = AnalyticsService::new(lake, view_factory);
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
        .route("/analytics/query_view", post(query_view_request))
        .route(
            "/analytics/query_thread_events",
            post(query_thread_events_request),
        )
        .route(
            "/analytics/query_partitions",
            post(query_partitions_request),
        )
        .route(
            "/analytics/create_or_update_partitions",
            post(create_or_update_partitions_request),
        )
        .route(
            "/analytics/merge_partitions",
            post(merge_partitions_request),
        )
        .route(
            "/analytics/retire_partitions",
            post(retire_partitions_request),
        )
        .layer(Extension(service))
        .layer(middleware::from_fn(observability_middleware))
        .layer(TimeoutLayer::new(std::time::Duration::from_secs(5 * 60)));
    let listener = tokio::net::TcpListener::bind(args.listen_endpoint)
        .await
        .unwrap();
    info!("serving on {}", &args.listen_endpoint);
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
    let data_lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    migrate_lakehouse(data_lake.db_pool.clone())
        .await
        .with_context(|| "migrate_lakehouse")?;
    let view_factory = ViewFactory::default();
    serve_http(&args, Arc::new(data_lake), Arc::new(view_factory)).await?;
    Ok(())
}
