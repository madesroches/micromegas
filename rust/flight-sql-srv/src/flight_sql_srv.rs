use anyhow::Context;
use clap::Parser;
use micromegas::analytics::lakehouse::migration::migrate_lakehouse;
use micromegas::analytics::lakehouse::partition_cache::LivePartitionProvider;
use micromegas::analytics::lakehouse::runtime::make_runtime_env;
use micromegas::analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas::analytics::lakehouse::view_factory::default_view_factory;
use micromegas::arrow_flight::flight_service_server::FlightServiceServer;
use micromegas::auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas::auth::multi::MultiAuthProvider;
use micromegas::auth::oidc::{OidcAuthProvider, OidcConfig};
use micromegas::auth::tower::AuthService;
use micromegas::auth::types::AuthProvider;
use micromegas::ingestion::data_lake_connection::connect_to_data_lake;
use micromegas::micromegas_main;
use micromegas::servers::connect_info_layer::ConnectedIncoming;
use micromegas::servers::flight_sql_service_impl::FlightSqlServiceImpl;
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
    let view_factory = Arc::new(default_view_factory(runtime.clone(), data_lake.clone()).await?);
    let partition_provider = Arc::new(LivePartitionProvider::new(data_lake.db_pool.clone()));
    let session_configurator = Arc::new(NoOpSessionConfigurator);
    let svc = FlightServiceServer::new(FlightSqlServiceImpl::new(
        runtime,
        data_lake,
        partition_provider,
        view_factory,
        session_configurator,
    )?)
    .max_decoding_message_size(100 * 1024 * 1024);

    let auth_required = !args.disable_auth;
    let auth_provider: Option<Arc<dyn AuthProvider>> = if auth_required {
        // Initialize API key provider if configured
        let api_key_provider = match std::env::var("MICROMEGAS_API_KEYS") {
            Ok(keys_json) => {
                let keyring = parse_key_ring(&keys_json)?;
                Some(Arc::new(ApiKeyAuthProvider::new(keyring)))
            }
            Err(_) => {
                info!("MICROMEGAS_API_KEYS not set - API key auth disabled");
                None
            }
        };

        // Initialize OIDC provider if configured
        let oidc_provider = match OidcConfig::from_env() {
            Ok(config) => {
                info!("Initializing OIDC authentication");
                Some(Arc::new(OidcAuthProvider::new(config).await?))
            }
            Err(e) => {
                info!("OIDC not configured ({e}) - OIDC auth disabled");
                None
            }
        };

        // Create multi-provider if either is configured
        if api_key_provider.is_some() || oidc_provider.is_some() {
            Some(Arc::new(MultiAuthProvider {
                api_key_provider,
                oidc_provider,
            }) as Arc<dyn AuthProvider>)
        } else {
            return Err("Authentication required but no auth providers configured. Set MICROMEGAS_API_KEYS or MICROMEGAS_OIDC_CONFIG".into());
        }
    } else {
        info!("Authentication disabled (--disable_auth)");
        None
    };

    let layer = ServiceBuilder::new()
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
