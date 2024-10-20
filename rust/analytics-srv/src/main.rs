//! Analytics Server
//!
//! Feeds data to the analytics-web interface.
//!
//! Env variables:
//!  - `MICROMEGAS_SQL_CONNECTION_STRING` : postgresql server
//!  - `MICROMEGAS_OBJECT_STORE_URI` : payloads, partitions

use anyhow::{Context, Result};
use axum::http::Method;
use axum::middleware;
use axum::{Extension, Router};
use clap::Parser;
use micromegas::analytics::analytics_service::AnalyticsService;
use micromegas::analytics::lakehouse::migration::migrate_lakehouse;
use micromegas::analytics::lakehouse::view_factory::{default_view_factory, ViewFactory};
use micromegas::axum_utils::observability_middleware;
use micromegas::ingestion::data_lake_connection::{connect_to_data_lake, DataLakeConnection};
use micromegas::servers::analytics::register_routes;
use micromegas::telemetry_sink::TelemetryGuardBuilder;
use micromegas::tracing::prelude::*;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::timeout::TimeoutLayer;

#[derive(Parser, Debug)]
#[clap(name = "Analytics Server")]
#[clap(about = "Analytics Server", version, author)]
struct Cli {
    #[clap(long, default_value = "127.0.0.1:8082")]
    listen_endpoint: SocketAddr,
}

async fn serve_http(
    args: &Cli,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = Arc::new(AnalyticsService::new(lake, view_factory));

    let app = register_routes(Router::new())
        .layer(Extension(service))
        .layer(middleware::from_fn(observability_middleware))
        .layer(TimeoutLayer::new(std::time::Duration::from_secs(5 * 60)))
        .layer(
            CorsLayer::new()
                .allow_methods([Method::POST])
                .allow_origin(Any),
        );
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
    let view_factory = default_view_factory()?;
    serve_http(&args, Arc::new(data_lake), Arc::new(view_factory)).await?;
    Ok(())
}
