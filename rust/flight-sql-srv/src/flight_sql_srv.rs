use anyhow::Context;
use clap::Parser;
use micromegas::analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas::analytics::lakehouse::migration::migrate_lakehouse;
use micromegas::analytics::lakehouse::partition_cache::LivePartitionProvider;
use micromegas::analytics::lakehouse::runtime::make_runtime_env;
use micromegas::analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas::analytics::lakehouse::view_factory::default_view_factory;
use micromegas::arrow_flight::flight_service_server::FlightServiceServer;
use micromegas::auth::tower::AuthService;
use micromegas::auth::types::AuthProvider;
use micromegas::ingestion::data_lake_connection::connect_to_data_lake;
use micromegas::micromegas_main;
use micromegas::servers::connect_info_layer::ConnectedIncoming;
use micromegas::servers::flight_sql_service_impl::FlightSqlServiceImpl;
use micromegas::servers::grpc_health_service::GrpcHealthService;
use micromegas::servers::log_uri_service::LogUriService;
use micromegas::tracing::prelude::*;
use std::sync::Arc;
use tonic::transport::Server;
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
    let lakehouse = Arc::new(LakehouseContext::new(data_lake.clone(), runtime));
    info!(
        "created lakehouse context with metadata cache: {:?}",
        lakehouse.metadata_cache()
    );
    let view_factory =
        Arc::new(default_view_factory(lakehouse.runtime().clone(), data_lake).await?);
    let partition_provider = Arc::new(LivePartitionProvider::new(lakehouse.lake().db_pool.clone()));
    let session_configurator = Arc::new(NoOpSessionConfigurator);
    let svc = FlightServiceServer::new(FlightSqlServiceImpl::new(
        lakehouse,
        partition_provider,
        view_factory,
        session_configurator,
    ))
    .max_decoding_message_size(100 * 1024 * 1024);

    let auth_required = !args.disable_auth;
    let auth_provider: Option<Arc<dyn AuthProvider>> = if auth_required {
        match micromegas::auth::default_provider::provider().await? {
            Some(provider) => Some(provider),
            None => {
                return Err("Authentication required but no auth providers configured. Set MICROMEGAS_API_KEYS or MICROMEGAS_OIDC_CONFIG".into());
            }
        }
    } else {
        info!("Authentication disabled (--disable_auth)");
        None
    };

    let layer = ServiceBuilder::new()
        .layer(layer_fn(GrpcHealthService::new))
        .layer(layer_fn(|service| LogUriService { service }))
        .layer(tower::layer::layer_fn(move |inner| AuthService {
            inner,
            auth_provider: auth_provider.clone(),
        }))
        .into_inner();

    let addr_str = "0.0.0.0:50051";
    let addr: std::net::SocketAddr = addr_str.parse()?;
    info!("Listening on {:?}", addr);

    // Create TCP listener and wrap with ConnectedIncoming to capture client IPs
    let listener = std::net::TcpListener::bind(addr)?;
    let incoming = ConnectedIncoming::from_std_listener(listener)?;

    Server::builder()
        .layer(layer)
        .add_service(svc)
        .serve_with_incoming(incoming)
        .await?;

    info!("bye");
    Ok(())
}
