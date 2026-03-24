use anyhow::{Context, Result};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::migration::migrate_lakehouse;
use micromegas_analytics::lakehouse::partition_cache::LivePartitionProvider;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::SessionConfigurator;
use micromegas_analytics::lakehouse::static_tables_configurator::StaticTablesConfigurator;
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, default_view_factory};
use micromegas_auth::tower::AuthService;
use micromegas_auth::types::AuthProvider;
use micromegas_ingestion::data_lake_connection::{DataLakeConnection, connect_to_data_lake};
use micromegas_tracing::prelude::*;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use arrow_flight::flight_service_server::FlightServiceServer;
use datafusion::execution::runtime_env::RuntimeEnv;
use tonic::transport::Server;
use tower::ServiceBuilder;
use tower::layer::layer_fn;

use super::connect_info_layer::ConnectedIncoming;
use super::flight_sql_service_impl::FlightSqlServiceImpl;
use super::grpc_health_service::GrpcHealthService;
use super::log_uri_service::LogUriService;

type ViewFactoryFn = Box<
    dyn FnOnce(
            Arc<RuntimeEnv>,
            Arc<DataLakeConnection>,
        ) -> Pin<Box<dyn Future<Output = Result<ViewFactory>> + Send>>
        + Send,
>;

/// Builder for assembling and running a FlightSQL server.
///
/// Encapsulates the full setup sequence: data lake connection, lakehouse migration,
/// runtime env, view factory, partition provider, session configurator, auth, and
/// the gRPC tower layer stack.
///
/// # Example
///
/// ```rust,no_run
/// use micromegas::servers::flight_sql_server::FlightSqlServer;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// FlightSqlServer::builder()
///     .with_default_auth()
///     .build_and_serve()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct FlightSqlServer;

impl FlightSqlServer {
    pub fn builder() -> FlightSqlServerBuilder {
        FlightSqlServerBuilder::default()
    }
}

pub struct FlightSqlServerBuilder {
    view_factory_fn: Option<ViewFactoryFn>,
    session_configurator: Option<Arc<dyn SessionConfigurator>>,
    auth_provider: Option<Arc<dyn AuthProvider>>,
    use_default_auth: bool,
    max_decoding_message_size: usize,
    listen_addr: SocketAddr,
}

impl Default for FlightSqlServerBuilder {
    fn default() -> Self {
        Self {
            view_factory_fn: None,
            session_configurator: None,
            auth_provider: None,
            use_default_auth: false,
            max_decoding_message_size: 100 * 1024 * 1024,
            listen_addr: "0.0.0.0:50051"
                .parse()
                .expect("valid default listen address"),
        }
    }
}

impl FlightSqlServerBuilder {
    /// Override the default view factory with a custom closure.
    ///
    /// The closure receives the runtime and data lake created by the builder.
    pub fn with_view_factory_fn<F, Fut>(mut self, f: F) -> Self
    where
        F: FnOnce(Arc<RuntimeEnv>, Arc<DataLakeConnection>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<ViewFactory>> + Send + 'static,
    {
        self.view_factory_fn = Some(Box::new(move |runtime, lake| Box::pin(f(runtime, lake))));
        self
    }

    /// Override the default session configurator.
    ///
    /// By default the builder loads static tables from `MICROMEGAS_STATIC_TABLES_URL`.
    /// Use this to replace that behavior entirely.
    pub fn with_session_configurator(mut self, cfg: Arc<dyn SessionConfigurator>) -> Self {
        self.session_configurator = Some(cfg);
        self
    }

    /// Set an explicit auth provider.
    pub fn with_auth_provider(mut self, provider: Arc<dyn AuthProvider>) -> Self {
        self.auth_provider = Some(provider);
        self.use_default_auth = false;
        self
    }

    /// Use the default auth provider from env vars during build.
    ///
    /// Errors if no auth providers are configured (fail-fast).
    pub fn with_default_auth(mut self) -> Self {
        self.use_default_auth = true;
        self.auth_provider = None;
        self
    }

    /// Set the max decoding message size (default: 100 MB).
    pub fn with_max_decoding_message_size(mut self, bytes: usize) -> Self {
        self.max_decoding_message_size = bytes;
        self
    }

    /// Set the listen address (default: `0.0.0.0:50051`).
    pub fn with_listen_addr(mut self, addr: SocketAddr) -> Self {
        self.listen_addr = addr;
        self
    }

    /// Build and run the FlightSQL server.
    ///
    /// Runs the full setup sequence and blocks until the server shuts down.
    pub async fn build_and_serve(self) -> Result<()> {
        let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
            .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
        let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
            .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;

        let data_lake =
            Arc::new(connect_to_data_lake(&connection_string, &object_store_uri).await?);
        migrate_lakehouse(data_lake.db_pool.clone())
            .await
            .with_context(|| "migrate_lakehouse")?;

        let runtime = Arc::new(make_runtime_env()?);
        let lakehouse = Arc::new(LakehouseContext::new(data_lake.clone(), runtime));
        info!(
            "created lakehouse context with metadata cache: {:?}",
            lakehouse.metadata_cache()
        );

        let view_factory = if let Some(factory_fn) = self.view_factory_fn {
            Arc::new(factory_fn(lakehouse.runtime().clone(), data_lake).await?)
        } else {
            Arc::new(default_view_factory(lakehouse.runtime().clone(), data_lake).await?)
        };

        let partition_provider =
            Arc::new(LivePartitionProvider::new(lakehouse.lake().db_pool.clone()));

        let session_configurator: Arc<dyn SessionConfigurator> =
            if let Some(cfg) = self.session_configurator {
                cfg
            } else {
                StaticTablesConfigurator::from_env(
                    "MICROMEGAS_STATIC_TABLES_URL",
                    lakehouse.runtime().clone(),
                )
                .await?
            };

        let svc = FlightServiceServer::new(FlightSqlServiceImpl::new(
            lakehouse,
            partition_provider,
            view_factory,
            session_configurator,
        ))
        .max_decoding_message_size(self.max_decoding_message_size);

        let auth_provider: Option<Arc<dyn AuthProvider>> = if let Some(provider) =
            self.auth_provider
        {
            Some(provider)
        } else if self.use_default_auth {
            match micromegas_auth::default_provider::provider().await? {
                Some(provider) => Some(provider),
                None => {
                    anyhow::bail!(
                        "Authentication required but no auth providers configured. Set MICROMEGAS_API_KEYS or MICROMEGAS_OIDC_CONFIG"
                    );
                }
            }
        } else {
            info!("Authentication disabled");
            None
        };

        let layer = ServiceBuilder::new()
            .layer(layer_fn(GrpcHealthService::new))
            .layer(layer_fn(|service| LogUriService { service }))
            .layer(layer_fn(move |inner| AuthService {
                inner,
                auth_provider: auth_provider.clone(),
            }))
            .into_inner();

        info!("Listening on {:?}", self.listen_addr);
        let listener = std::net::TcpListener::bind(self.listen_addr)?;
        let incoming = ConnectedIncoming::from_std_listener(listener)?;

        Server::builder()
            .layer(layer)
            .add_service(svc)
            .serve_with_incoming(incoming)
            .await?;

        info!("bye");
        Ok(())
    }
}
