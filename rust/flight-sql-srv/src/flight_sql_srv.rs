use anyhow::Context;
use arrow_flight::flight_service_server::FlightServiceServer;
use micromegas::analytics::flight_sql_service_impl::FlightSqlServiceImpl;
use micromegas::analytics::lakehouse::migration::migrate_lakehouse;
use micromegas::analytics::lakehouse::partition_cache::LivePartitionProvider;
use micromegas::analytics::lakehouse::view_factory::default_view_factory;
use micromegas::ingestion::data_lake_connection::connect_to_data_lake;
use micromegas::key_ring::parse_key_ring;
use micromegas::telemetry_sink::TelemetryGuardBuilder;
use micromegas::tonic_auth_interceptor::check_auth;
use micromegas::tracing::prelude::*;
use std::sync::Arc;
use tonic::service::interceptor;
use tonic::transport::Server;
use tower::ServiceBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_ctrlc_handling()
        .with_local_sink_max_level(LevelFilter::Info)
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
        .layer(interceptor(move |req| check_auth(req, &keyring)))
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
