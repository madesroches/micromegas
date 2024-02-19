//! Legion Analytics Server
//!
//! Feeds data to the analytics-web interface.
//!
//! Env variables:
//!  - `LEGION_TELEMETRY_INGESTION_SRC_DATA_DIRECTORY` : local telemetry
//!    directory
//!  - `LEGION_TELEMETRY_CACHE_DIRECTORY` : local directory where reusable
//!    computations will be stored

// crate-specific lint exceptions:
//#![allow()]

mod analytics_service;
mod auth;
mod cache;
mod call_tree;
mod cumulative_call_graph;
mod cumulative_call_graph_handler;
mod cumulative_call_graph_node;
mod health_check_service;
mod lakehouse;
mod log_entry;
mod metrics;
mod scope;
mod thread_block_processor;

use std::str::FromStr;
use std::{path::PathBuf, sync::Arc};

use analytics_service::AnalyticsService;
use anyhow::{Context, Result};
use auth::AuthLayer;
use clap::{Parser, Subcommand};
use health_check_service::HealthCheckService;
use lakehouse::jit_lakehouse::JitLakehouse;
use lakehouse::local_jit_lakehouse::LocalJitLakehouse;
use lakehouse::remote_jit_lakehouse::RemoteJitLakehouse;
use lgn_blob_storage::{
    AwsS3BlobStorage, AwsS3Url, BlobStorage, LocalBlobStorage, Lz4BlobStorageAdapter,
};
use lgn_telemetry_proto::analytics::performance_analytics_server::PerformanceAnalyticsServer;
use lgn_telemetry_proto::health::health_server::HealthServer;
use telemetry_sink::TelemetryGuardBuilder;
use tracing::prelude::*;
use std::net::SocketAddr;
use tonic::transport::Server;

#[derive(Parser, Debug)]
#[clap(name = "Legion Performance Analytics Server")]
#[clap(about = "Legion Performance Analytics Server", version, author)]
#[clap(arg_required_else_help(true))]
struct Cli {
    #[clap(long, default_value = "[::1]:9090")]
    listen_endpoint: SocketAddr,

    #[clap(subcommand)]
    spec: DataLakeSpec,

    #[clap(help = "optional local path or s3://bucket/root")]
    lakehouse_uri: Option<String>,
}

#[derive(Subcommand, Debug)]
enum DataLakeSpec {
    Local {
        data_lake_path: PathBuf,
        cache_path: PathBuf,
    },
    Remote {
        db_uri: String,
        s3_lake_url: String,
        s3_cache_url: String,
    },
}

async fn new_jit_lakehouse(
    uri: String,
    pool: sqlx::AnyPool,
    data_lake_blobs: Arc<dyn BlobStorage>,
) -> Result<Arc<dyn JitLakehouse>> {
    if uri.starts_with("s3://") {
        Ok(Arc::new(
            RemoteJitLakehouse::new(pool, data_lake_blobs, AwsS3Url::from_str(&uri)?).await,
        ))
    } else {
        Ok(Arc::new(LocalJitLakehouse::new(
            pool,
            data_lake_blobs,
            PathBuf::from(uri),
        )))
    }
}

/// ``connect_to_local_data_lake`` serves a locally hosted data lake
///
/// # Errors
/// block storage must exist and sqlite database must accept connections
#[span_fn]
pub async fn connect_to_local_data_lake(
    data_lake_path: PathBuf,
    cache_path: PathBuf,
    lakehouse_uri: Option<String>,
) -> Result<AnalyticsService> {
    let blocks_folder = data_lake_path.join("blobs");
    let data_lake_blobs = Arc::new(LocalBlobStorage::new(blocks_folder).await?);
    let cache_blobs = Arc::new(Lz4BlobStorageAdapter::new(
        LocalBlobStorage::new(cache_path.clone()).await?,
    ));
    let db_path = data_lake_path.join("telemetry.db3");
    let db_uri = format!("sqlite://{}", db_path.to_str().unwrap().replace('\\', "/"));
    let pool = sqlx::any::AnyPoolOptions::new()
        .connect(&db_uri)
        .await
        .with_context(|| String::from("Connecting to telemetry database"))?;
    let lakehouse = new_jit_lakehouse(
        lakehouse_uri.unwrap_or_else(|| cache_path.join("tables").to_string_lossy().to_string()),
        pool.clone(),
        data_lake_blobs.clone(),
    )
    .await?;
    Ok(AnalyticsService::new(
        pool,
        data_lake_blobs,
        cache_blobs,
        lakehouse,
    ))
}

/// ``connect_to_remote_data_lake`` serves a remote data lake through mysql and s3
///
/// # Errors
/// block storage must exist and mysql database must accept connections
#[span_fn]
pub async fn connect_to_remote_data_lake(
    db_uri: &str,
    s3_url_data_lake: &str,
    s3_url_cache: String,
    lakehouse_uri: Option<String>,
) -> Result<AnalyticsService> {
    info!("connecting to blob storage");
    let data_lake_blobs =
        Arc::new(AwsS3BlobStorage::new(AwsS3Url::from_str(s3_url_data_lake)?).await);
    let cache_blobs = Arc::new(Lz4BlobStorageAdapter::new(
        AwsS3BlobStorage::new(AwsS3Url::from_str(&s3_url_cache)?).await,
    ));
    let pool = sqlx::any::AnyPoolOptions::new()
        .max_connections(10)
        .connect(db_uri)
        .await
        .with_context(|| String::from("Connecting to telemetry database"))?;

    let lakehouse = new_jit_lakehouse(
        lakehouse_uri.unwrap_or_else(|| {
            if s3_url_cache.ends_with('/') {
                s3_url_cache + "tables/"
            } else {
                s3_url_cache + "/tables/"
            }
        }),
        pool.clone(),
        data_lake_blobs.clone(),
    )
    .await?;

    Ok(AnalyticsService::new(
        pool,
        data_lake_blobs,
        cache_blobs,
        lakehouse,
    ))
}

#[tokio::main]
#[span_fn]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_ctrlc_handling()
        .build();
    info!("starting analytics server");
    span_scope!("analytics-srv::main");
    let args = Cli::parse();
    let analytics_service = match args.spec {
        DataLakeSpec::Local {
            data_lake_path,
            cache_path,
        } => connect_to_local_data_lake(data_lake_path, cache_path, args.lakehouse_uri).await?,
        DataLakeSpec::Remote {
            db_uri,
            s3_lake_url,
            s3_cache_url,
        } => {
            connect_to_remote_data_lake(&db_uri, &s3_lake_url, s3_cache_url, args.lakehouse_uri)
                .await?
        }
    };

    // cors fails: Access to fetch at 'https://analytics-api.playground.legionlabs.com:9090/analytics.PerformanceAnalytics/list_recent_processes' from origin 'https://analytics.legionengine.com' has been blocked by CORS policy: Response to preflight request doesn't pass access control check: No 'Access-Control-Allow-Origin' header is present on the requested resource. If an opaque response serves your needs, set the request's mode to 'no-cors' to fetch the resource with CORS disabled.
    // let origins = vec![
    //     "http://localhost:3000".parse().unwrap(),
    //     "http://localhost".parse().unwrap(),
    //     "https://analytics.legionengine.com/".parse().unwrap(),
    // ];

    // let cors = CorsLayer::new()
    //     .allow_origin(Origin::list(origins))
    //     .allow_credentials(true)
    //     .max_age(Duration::from_secs(60 * 60))
    //     .allow_headers(vec![
    //         header::ACCEPT,
    //         header::ACCEPT_LANGUAGE,
    //         header::AUTHORIZATION,
    //         header::CONTENT_LANGUAGE,
    //         header::CONTENT_TYPE,
    //         http::header::HeaderName::from_static("x-grpc-web"),
    //     ])
    //     .allow_methods(vec![
    //         Method::GET,
    //         Method::POST,
    //         Method::PUT,
    //         Method::HEAD,
    //         Method::OPTIONS,
    //         Method::CONNECT,
    //     ])
    //     .expose_headers(tower_http::cors::Any {});

    let auth_layer = tower::ServiceBuilder::new() //todo: compose with cors layer
        .layer(AuthLayer {
            user_info_url: String::from(
                "https://legionengine.auth.ca-central-1.amazoncognito.com/oauth2/userInfo",
            ),
        })
        .into_inner();
    let analytics_server = PerformanceAnalyticsServer::new(analytics_service);
    let health_check_server = HealthServer::new(HealthCheckService {});
    Server::builder()
        .accept_http1(true)
        .layer(auth_layer)
        .add_service(tonic_web::enable(health_check_server))
        .add_service(tonic_web::enable(analytics_server))
        .serve(args.listen_endpoint)
        .await?;
    Ok(())
}
