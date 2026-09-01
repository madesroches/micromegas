use anyhow::Result;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::LivePartitionProvider;
use micromegas_analytics::lakehouse::read_scope::IsolationConfig;
use micromegas_analytics::lakehouse::session_configurator::SessionConfigurator;
use micromegas_analytics::lakehouse::static_tables_configurator::StaticTablesConfigurator;
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, default_view_factory};
use micromegas_auth::db_api_key::{ApiKeyTable, dedicated_key_store_pool};
use micromegas_auth::db_audience_grants::{DbAudienceGrantsConfig, DbAudienceGrantsSource};
use micromegas_auth::default_provider::ProviderBuilder;
use micromegas_auth::policy::{AudienceGrants, AudienceReadPolicy, ReadPolicy};
use micromegas_auth::tower::AuthService;
use micromegas_auth::types::AuthProvider;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_ingestion::sql_migration::warn_if_data_lake_schema_stale;
use micromegas_tracing::prelude::*;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

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

/// The auth provider plus the default `ReadPolicy`/`IsolationConfig` `build_and_serve`
/// resolves per branch, before `self.read_policy`/`self.isolation_config` (if set) override them.
type AuthAndDefaults = (
    Option<Arc<dyn AuthProvider>>,
    Arc<dyn ReadPolicy>,
    Arc<IsolationConfig>,
);

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
    read_policy: Option<Arc<dyn ReadPolicy>>,
    isolation_config: Option<Arc<IsolationConfig>>,
    use_default_auth: bool,
    max_decoding_message_size: usize,
    listen_addr: SocketAddr,
    shutdown_grace: Duration,
    injected_lakehouse: Option<Arc<LakehouseContext>>,
    injected_shutdown: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    health_listen_addr: Option<SocketAddr>,
}

impl Default for FlightSqlServerBuilder {
    fn default() -> Self {
        Self {
            view_factory_fn: None,
            session_configurator: None,
            auth_provider: None,
            read_policy: None,
            isolation_config: None,
            use_default_auth: false,
            max_decoding_message_size: 100 * 1024 * 1024,
            listen_addr: "0.0.0.0:50051"
                .parse()
                .expect("valid default listen address"),
            shutdown_grace: Duration::from_secs(25),
            injected_lakehouse: None,
            injected_shutdown: None,
            health_listen_addr: None,
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

    /// Set an explicit `ReadPolicy`, resolved once per request against the caller's
    /// `AuthContext`. This policy wins on every `build_and_serve` branch,
    /// overriding that branch's own default. When never called, the default depends on how auth
    /// is configured: with `with_default_auth()` or `with_auth_provider(..)`,
    /// `AudienceReadPolicy::from_env("")` backed by the DB grant store (so the seeded `public`
    /// read row still resolves); with auth left disabled, `AudienceReadPolicy` with an empty
    /// grant map, though that branch never actually resolves it. **Not** `ReadScope::All`: the
    /// absent-`AuthContext`-extension convention already supplies `All` when no provider is
    /// configured, so this default must not duplicate
    /// that decision -- it only ever resolves a scope when an `AuthContext` is present to resolve
    /// one from.
    pub fn with_read_policy(mut self, policy: Arc<dyn ReadPolicy>) -> Self {
        self.read_policy = Some(policy);
        self
    }

    /// Set an explicit `IsolationConfig` -- the data-isolation deployment knob
    /// (`MICROMEGAS_PUBLIC_VIEW_SETS`) consumed by the row-level filter
    /// (`OwnershipRewrite`). Mirrors `with_read_policy`: wins on
    /// every `build_and_serve` branch, overriding that branch's own default
    /// (`IsolationConfig::from_env("")` on the `use_default_auth` branch, `IsolationConfig::default()`
    /// on the other two).
    pub fn with_isolation_config(mut self, config: Arc<IsolationConfig>) -> Self {
        self.isolation_config = Some(config);
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

    /// Set the grace period for graceful shutdown on SIGTERM (default: 25s).
    pub fn with_shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = grace;
        self
    }

    /// Inject a pre-built `LakehouseContext` instead of calling `LakehouseContext::from_env`.
    ///
    /// Useful for the monolith, which constructs one shared context for all lake-backed roles.
    pub fn with_lakehouse(mut self, lakehouse: Arc<LakehouseContext>) -> Self {
        self.injected_lakehouse = Some(lakehouse);
        self
    }

    /// Inject a custom shutdown future instead of the default `wait_for_sigterm()`.
    ///
    /// The monolith passes `fanout.subscribe()` here so all roles shut down from one signal.
    pub fn with_shutdown(mut self, shutdown: impl Future<Output = ()> + Send + 'static) -> Self {
        self.injected_shutdown = Some(Box::pin(shutdown));
        self
    }

    /// Spawn a lightweight HTTP sidecar (`/health`, `/ready`) on `addr`.
    ///
    /// Enables plain-HTTP ALB health checks without changing the gRPC protocol.
    /// If not set, no sidecar is started.
    pub fn with_health_addr(mut self, addr: SocketAddr) -> Self {
        self.health_listen_addr = Some(addr);
        self
    }

    /// Build and run the FlightSQL server.
    ///
    /// Runs the full setup sequence and blocks until the server shuts down.
    pub async fn build_and_serve(self) -> Result<()> {
        // Use injected lakehouse or build one from environment
        let lakehouse = if let Some(lh) = self.injected_lakehouse {
            lh
        } else {
            LakehouseContext::from_env().await?
        };
        let data_lake = lakehouse.lake().clone();
        let probe_lake = lakehouse.lake().clone();
        // Cloned here, before `lakehouse` is moved into `FlightSqlServiceImpl::new` below, so the
        // refresh task (spawned further down, once the shutdown fanout exists) has its own
        // handle. Only the FlightSQL server builder spawns this -- other `LakehouseContext`
        // holders (maintenance daemon, tests) keep an empty snapshot and never deny anything.
        let query_denials = lakehouse.query_denials().clone();
        info!(
            "created lakehouse context with metadata cache: {:?}",
            lakehouse.metadata_cache()
        );

        let view_factory = if let Some(factory_fn) = self.view_factory_fn {
            Arc::new(factory_fn(lakehouse.runtime().clone(), data_lake).await?)
        } else {
            Arc::new(
                default_view_factory(
                    lakehouse.runtime().clone(),
                    data_lake,
                    lakehouse.default_audience(),
                )
                .await?,
            )
        };

        let partition_provider =
            Arc::new(LivePartitionProvider::new(lakehouse.lake().db_pool.clone()));
        // Cloned here, before `lakehouse` is moved into `FlightSqlServiceImpl::new`
        // below, so `dedicated_key_store_pool` has a pool to read connect options
        // from.
        let lake_pool_for_keys = lakehouse.lake().db_pool.clone();

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

        // `FlightSqlServiceImpl::new` takes the resolved `ReadPolicy` as a constructor argument,
        // so auth/policy resolution happens here, before construction.
        //
        // `self.read_policy` (set via `with_read_policy`) must win on every branch below --
        // each branch only computes its own *default*, used solely when the caller never set an
        // explicit policy. This keeps `with_read_policy` order-independent with respect to
        // `with_auth_provider` / `with_default_auth`.
        let (auth_provider, default_policy, default_isolation_config): AuthAndDefaults =
            if let Some(provider) = self.auth_provider {
                // Injected-provider path (the monolith's `with_auth_provider` call): the caller is
                // expected to pair `with_auth_provider` with its own `with_read_policy` call, but
                // when it didn't, this default must still resolve the seeded `public` read grant
                // rather than the empty set -- an empty-grant-map default was only ever safe
                // while `resolve` had a built-in `PUBLIC_AUDIENCE` arm; with that arm removed it
                // would silently deny every query for an embedder that forgot `with_read_policy`.
                // So this builds the same env+store-backed policy the `use_default_auth` branch
                // below does.
                //
                // Built only when `self.read_policy` is unset -- `with_read_policy` always wins
                // below, and the only in-repo `with_auth_provider` caller (the monolith) always
                // pairs the two, so skipping this avoids a DB round trip -- and a needless
                // `from_env("")` failure mode on a malformed *unprefixed* env var a caller may
                // have deliberately overridden via `with_read_policy` -- whenever it would be
                // discarded anyway.
                let policy: Arc<dyn ReadPolicy> = if self.read_policy.is_none() {
                    let key_store_pool = dedicated_key_store_pool(&lake_pool_for_keys);
                    warn_if_data_lake_schema_stale(&lake_pool_for_keys).await;
                    let audience_grants_config = DbAudienceGrantsConfig::from_env_with_prefix("");
                    let audience_grants_store = Arc::new(DbAudienceGrantsSource::new(
                        key_store_pool,
                        audience_grants_config,
                    ));
                    Arc::new(
                        AudienceReadPolicy::from_env("")?.with_store(Some(audience_grants_store)),
                    )
                } else {
                    Arc::new(AudienceReadPolicy::new(AudienceGrants::empty()))
                };
                (Some(provider), policy, Arc::new(IsolationConfig::default()))
            } else if self.use_default_auth {
                let key_store_pool = dedicated_key_store_pool(&lake_pool_for_keys);
                warn_if_data_lake_schema_stale(&lake_pool_for_keys).await;
                let provider = match ProviderBuilder::new("")
                    .with_db_key_store(key_store_pool, ApiKeyTable::Analytics)
                    .build()
                    .await?
                {
                    Some(provider) => provider,
                    None => {
                        anyhow::bail!(
                            "Authentication required but no auth providers configured. Set \
                         MICROMEGAS_API_KEYS or MICROMEGAS_OIDC_CONFIG, or populate the \
                         analytics_api_keys DB table"
                        );
                    }
                };
                // One shared snapshot cache for this process, built from
                // its own dedicated pool -- not `key_store_pool` above, which
                // `DbApiKeyAuthProvider` already owns -- via the same `dedicated_key_store_pool`
                // convention. Same prefix (`""`) `AudienceReadPolicy::from_env` beside it
                // resolves under, so the cache-TTL knob follows the same `{prefix}_` fallback.
                let audience_grants_pool = dedicated_key_store_pool(&lake_pool_for_keys);
                let audience_grants_config = DbAudienceGrantsConfig::from_env_with_prefix("");
                let audience_grants_store = Arc::new(DbAudienceGrantsSource::new(
                    audience_grants_pool,
                    audience_grants_config,
                ));
                let policy: Arc<dyn ReadPolicy> = Arc::new(
                    AudienceReadPolicy::from_env("")?.with_store(Some(audience_grants_store)),
                );
                let isolation_config = Arc::new(IsolationConfig::from_env("")?);
                (Some(provider), policy, isolation_config)
            } else {
                info!("Authentication disabled");
                // No `AuthContext` extension is ever inserted on this path (no `AuthService`
                // provider configured), so the absent-extension convention already supplies
                // `ReadScope::All` -- this default policy is never actually resolved against a
                // real caller, but must still exist since `FlightSqlServiceImpl::new` requires
                // one. This never-resolved property is now load-bearing: with the built-in
                // `PUBLIC_AUDIENCE` read arm removed, an empty grant map resolves to the empty
                // set, not `{public}` -- fine here only because `resolve` is never actually
                // called on this branch.
                (
                    None,
                    Arc::new(AudienceReadPolicy::new(AudienceGrants::empty())),
                    Arc::new(IsolationConfig::default()),
                )
            };
        let read_policy = self.read_policy.unwrap_or(default_policy);
        let isolation_config = self.isolation_config.unwrap_or(default_isolation_config);

        // Whether this deployment can ever produce an admin principal at all -- computed once
        // here, ahead of every request, so it covers all three branches above (including the
        // injected-provider branch the monolith takes via `with_auth_provider`), not just
        // `use_default_auth`. A `None` provider means auth is disabled, where every caller is
        // already treated as admin by the absent-header convention (`user_attribution.rs`), so
        // `true` is both correct and the conservative choice there.
        let admin_principal_possible = auth_provider.as_ref().is_none_or(|p| p.can_grant_admin());

        let svc = FlightServiceServer::new(FlightSqlServiceImpl::new(
            lakehouse,
            partition_provider,
            view_factory,
            session_configurator,
            read_policy,
            isolation_config,
            admin_principal_possible,
        ))
        .max_decoding_message_size(self.max_decoding_message_size);

        let layer = ServiceBuilder::new()
            .layer(layer_fn(GrpcHealthService::new))
            .layer(layer_fn(|service| LogUriService { service }))
            .layer(layer_fn(move |inner| AuthService {
                inner,
                auth_provider: auth_provider.clone(),
            }))
            .into_inner();

        use super::shutdown::{ShutdownFanout, wait_for_sigterm};

        info!("Listening on {:?}", self.listen_addr);
        let listener = std::net::TcpListener::bind(self.listen_addr)?;
        let incoming = ConnectedIncoming::from_std_listener(listener)?;

        // Use injected shutdown future or default to SIGTERM
        let shutdown_future: Pin<Box<dyn Future<Output = ()> + Send + 'static>> = self
            .injected_shutdown
            .unwrap_or_else(|| Box::pin(wait_for_sigterm()));
        let fanout = ShutdownFanout::new(shutdown_future);
        let grace_secs = self.shutdown_grace.as_secs();
        let grace = self.shutdown_grace;

        // Admin-managed query deny list: refreshes the shared snapshot every
        // `MICROMEGAS_QUERY_DENY_REFRESH_SECONDS`, once immediately and then on a tick, until
        // this replica shuts down.
        query_denials.spawn_refresh_task(fanout.subscribe());

        if let Some(health_addr) = self.health_listen_addr {
            use super::readiness::ReadinessProbe;
            use axum::Extension;
            use axum::Router;
            use axum::routing::get;
            use tokio::net::TcpListener;

            let probe = std::sync::Arc::new(ReadinessProbe::new(probe_lake));
            let sidecar_listener = TcpListener::bind(health_addr).await?;
            let shutdown_rx = fanout.subscribe();
            tokio::spawn(async move {
                async fn sidecar_ready(
                    Extension(p): Extension<std::sync::Arc<ReadinessProbe>>,
                ) -> axum::http::StatusCode {
                    if p.check_ready().await {
                        axum::http::StatusCode::OK
                    } else {
                        axum::http::StatusCode::SERVICE_UNAVAILABLE
                    }
                }
                let sidecar_app = Router::new()
                    .route("/health", get(|| async { axum::http::StatusCode::OK }))
                    .route("/ready", get(sidecar_ready))
                    .layer(Extension(probe));
                let _ = axum::serve(sidecar_listener, sidecar_app)
                    .with_graceful_shutdown(shutdown_rx)
                    .await;
                info!("FlightSQL health sidecar stopped");
            });
            info!("FlightSQL health sidecar listening on {health_addr}");
        }

        let serve = Server::builder()
            .layer(layer)
            .add_service(svc)
            .serve_with_incoming_shutdown(incoming, fanout.subscribe());

        let deadline = {
            let d = fanout.subscribe();
            async move {
                d.await;
                tokio::time::sleep(grace).await;
            }
        };

        tokio::select! {
            res = serve => {
                info!("drain completed");
                res?;
            }
            _ = deadline => {
                warn!("grace period of {grace_secs}s elapsed with work still in flight");
            }
        }

        info!("bye");
        Ok(())
    }
}
