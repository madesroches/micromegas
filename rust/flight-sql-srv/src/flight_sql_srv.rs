mod api_keyring;
mod flight_sql_service_impl;

use anyhow::Context;
use api_keyring::{parse_key_ring, KeyRing};
use arrow_flight::flight_service_server::FlightServiceServer;
use flight_sql_service_impl::FlightSqlServiceImpl;
use micromegas::analytics::lakehouse::migration::migrate_lakehouse;
use micromegas::analytics::lakehouse::partition_cache::LivePartitionProvider;
use micromegas::analytics::lakehouse::view_factory::default_view_factory;
use micromegas::ingestion::data_lake_connection::connect_to_data_lake;
use micromegas::telemetry_sink::TelemetryGuardBuilder;
use micromegas::tracing::prelude::*;
use std::sync::Arc;
use tonic::service::interceptor;
use tonic::transport::Server;
use tonic::{Request, Status};
use tower::ServiceBuilder;

fn check_auth(req: Request<()>, keyring: Arc<KeyRing>) -> Result<Request<()>, Status> {
    let metadata = req.metadata();
    let auth = metadata.get("authorization").ok_or_else(|| {
        Status::internal(format!("No authorization header! metadata = {metadata:?}"))
    })?;
    let str = auth
        .to_str()
        .map_err(|e| Status::internal(format!("Error parsing header: {e}")))?;
    let authorization = str.to_string();
    let bearer = "Bearer ";
    if !authorization.starts_with(bearer) {
        Err(Status::internal("Invalid auth header!"))?;
    }
    let token = authorization[bearer.len()..].to_string();
    if let Some(name) = keyring.get(&token) {
        info!("caller={name}");
        Ok(req)
    } else {
        Err(Status::unauthenticated("invalid API token"))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_ctrlc_handling()
        .with_local_sink_max_level(LevelFilter::Debug)
        .build();
    let keyring = Arc::new(parse_key_ring(
        &std::env::var("MICROMEGAS_API_KEYS").with_context(|| "reading MICROMEGAS_API_KEYS")?,
    )?);
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let data_lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    migrate_lakehouse(data_lake.db_pool.clone())
        .await
        .with_context(|| "migrate_lakehouse")?;
    let view_factory = Arc::new(default_view_factory()?);
    let partition_provider = Arc::new(LivePartitionProvider::new(data_lake.db_pool.clone()));
    let svc = FlightServiceServer::new(FlightSqlServiceImpl::new(
        Arc::new(data_lake),
        partition_provider,
        view_factory,
    ));
    let layer = ServiceBuilder::new()
        .layer(interceptor(move |req| check_auth(req, keyring.clone())))
        .into_inner();
    let addr_str = "0.0.0.0:50051";
    let addr = addr_str.parse()?;
    info!("Listening on {:?}", addr);
    Server::builder()
        .layer(layer)
        .add_service(svc)
        .serve(addr)
        .await?;

    info!("bye");
    Ok(())
}
