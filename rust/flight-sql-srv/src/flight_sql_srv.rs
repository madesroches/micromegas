use anyhow::Context;
use clap::Parser;
use micromegas::analytics::lakehouse::migration::migrate_lakehouse;
use micromegas::analytics::lakehouse::partition_cache::LivePartitionProvider;
use micromegas::analytics::lakehouse::runtime::make_runtime_env;
use micromegas::analytics::lakehouse::view_factory::default_view_factory;
use micromegas::arrow_flight::flight_service_server::FlightServiceServer;
use micromegas::ingestion::data_lake_connection::connect_to_data_lake;
use micromegas::micromegas_main;
use micromegas::servers::flight_sql_service_impl::FlightSqlServiceImpl;
use micromegas::servers::key_ring::{KeyRing, parse_key_ring};
use micromegas::servers::log_uri_service::LogUriService;
use micromegas::servers::tonic_auth_interceptor::check_auth;
use micromegas::tonic::service::interceptor::InterceptorLayer;
use micromegas::tonic::transport::Server;
use micromegas::tracing::prelude::*;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower::layer::layer_fn;

#[derive(Parser, Debug)]
#[clap(name = "Micromegas FlightSQL server")]
#[clap(about = "Micromegas FlightSQL server", version, author)]
struct Cli {
    #[clap(long)]
    disable_auth: bool,
}

#[micromegas_main(interop_max_level = "info", max_level_override = "debug")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let data_lake = Arc::new(connect_to_data_lake(&connection_string, &object_store_uri).await?);
    migrate_lakehouse(data_lake.db_pool.clone())
        .await
        .with_context(|| "migrate_lakehouse")?;
    let runtime = Arc::new(make_runtime_env()?);
    let view_factory = Arc::new(default_view_factory(runtime.clone(), data_lake.clone()).await?);
    let partition_provider = Arc::new(LivePartitionProvider::new(data_lake.db_pool.clone()));
    let svc = FlightServiceServer::new(FlightSqlServiceImpl::new(
        runtime,
        data_lake,
        partition_provider,
        view_factory,
    )?)
    .max_decoding_message_size(100 * 1024 * 1024);
    let auth_required = !args.disable_auth;
    let keyring = if auth_required {
        Arc::new(parse_key_ring(
            &std::env::var("MICROMEGAS_API_KEYS").with_context(|| "reading MICROMEGAS_API_KEYS")?,
        )?)
    } else {
        Arc::new(KeyRing::new())
    };
    let layer = ServiceBuilder::new()
        .layer(layer_fn(|service| LogUriService { service }))
        .layer(InterceptorLayer::new(move |req| {
            if auth_required {
                check_auth(req, &keyring)
            } else {
                Ok(req)
            }
        }))
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
