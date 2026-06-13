//! Single-process deployment of all Micromegas roles.
//!
//! Runs ingestion, FlightSQL, maintenance, and web in one tokio runtime
//! sharing a single data-lake connection, cache, and SIGTERM fanout.
//!
//! Env variables (in addition to role-specific vars):
//!  - `MICROMEGAS_SQL_CONNECTION_STRING`      — shared data-lake Postgres
//!  - `MICROMEGAS_OBJECT_STORE_URI`           — shared object store
//!  - `MICROMEGAS_APP_SQL_CONNECTION_STRING`  — web-app Postgres
//!  - `MICROMEGAS_WEB_CORS_ORIGIN`            — required by web role
//!  - `MICROMEGAS_BASE_PATH`                  — required by web role (e.g. `/`)
//!  - `MICROMEGAS_MONOLITH_ROLES`             — comma-separated role list (default `all`)

#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use analytics_web_srv::app_db;
use analytics_web_srv::web_server::{WebServerConfig, run_web_server};
use anyhow::{Context, Result};
use clap::Parser;
use micromegas::analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas::analytics::lakehouse::view_factory::default_view_factory;
use micromegas::auth::default_provider::provider_with_prefix;
use micromegas::ingestion::remote_data_lake::connect_to_remote_data_lake;
use micromegas::micromegas_main;
use micromegas::servers::flight_sql_server::FlightSqlServer;
use micromegas::servers::ingestion::serve_ingestion;
use micromegas::servers::maintenance::{daemon, get_global_views_with_update_group};
use micromegas::servers::shutdown::{ShutdownFanout, wait_for_sigterm};
use micromegas::tracing::prelude::*;
use std::collections::HashSet;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;

// ---------------------------------------------------------------------------
// Role parsing
// ---------------------------------------------------------------------------

const ROLE_INGESTION: &str = "ingestion";
const ROLE_FLIGHTSQL: &str = "flightsql";
const ROLE_MAINTENANCE: &str = "maintenance";
const ROLE_WEB: &str = "web";
const ALL_ROLES: &[&str] = &[ROLE_INGESTION, ROLE_FLIGHTSQL, ROLE_MAINTENANCE, ROLE_WEB];

struct Roles {
    ingestion: bool,
    flightsql: bool,
    maintenance: bool,
    web: bool,
}

impl Roles {
    fn parse(spec: &str) -> Result<Self> {
        let normalized = spec.trim().to_lowercase();
        let set: HashSet<&str> = if normalized == "all" {
            ALL_ROLES.iter().copied().collect()
        } else {
            normalized
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<HashSet<_>>()
                .into_iter()
                .map(|s| {
                    if ALL_ROLES.contains(&s) {
                        Ok(s)
                    } else {
                        anyhow::bail!("Unknown role '{s}'. Valid roles: {}", ALL_ROLES.join(", "))
                    }
                })
                .collect::<Result<HashSet<_>>>()?
        };

        if set.is_empty() {
            anyhow::bail!("At least one role must be specified");
        }

        Ok(Self {
            ingestion: set.contains(ROLE_INGESTION),
            flightsql: set.contains(ROLE_FLIGHTSQL),
            maintenance: set.contains(ROLE_MAINTENANCE),
            web: set.contains(ROLE_WEB),
        })
    }

    fn needs_lakehouse(&self) -> bool {
        self.ingestion || self.flightsql || self.maintenance
    }
}

impl fmt::Display for Roles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut active = vec![];
        if self.ingestion {
            active.push(ROLE_INGESTION);
        }
        if self.flightsql {
            active.push(ROLE_FLIGHTSQL);
        }
        if self.maintenance {
            active.push(ROLE_MAINTENANCE);
        }
        if self.web {
            active.push(ROLE_WEB);
        }
        write!(f, "{}", active.join(", "))
    }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[clap(name = "Micromegas Monolith")]
#[clap(
    about = "Single-process deployment of all Micromegas roles",
    version,
    author
)]
struct Cli {
    /// Roles to enable: comma-separated list of ingestion,flightsql,maintenance,web or "all"
    #[clap(long, env = "MICROMEGAS_MONOLITH_ROLES", default_value = "all")]
    roles: String,

    /// HTTP ingestion listen address
    #[clap(long, default_value = "127.0.0.1:8081")]
    listen_endpoint_http: SocketAddr,

    /// Web server port
    #[clap(long, default_value = "3000", env = "MICROMEGAS_PORT")]
    port: u16,

    /// Frontend build directory (web role)
    #[clap(long, default_value = "/app/frontend")]
    frontend_dir: String,

    /// Disable authentication for all roles (development only)
    #[clap(long)]
    disable_auth: bool,

    /// Disable authentication for the ingestion role only (useful when web uses OIDC)
    #[clap(long)]
    disable_ingestion_auth: bool,

    /// Seconds to wait for in-flight requests to complete after shutdown signal
    #[clap(
        long,
        default_value = "25",
        env = "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS"
    )]
    shutdown_grace_period_seconds: u64,

    /// Opt out of auto-seeding the local FlightSQL data source in the web app DB
    #[clap(long)]
    no_seed_data_source: bool,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[micromegas_main(interop_max_level = "info", max_level_override = "debug")]
async fn main() -> Result<()> {
    let args = Cli::parse();
    let roles = Roles::parse(&args.roles)?;
    let grace = Duration::from_secs(args.shutdown_grace_period_seconds);

    info!("Starting micromegas-monolith with roles: {roles}");

    // Build shared data lake for lake-backed roles
    let lakehouse: Option<Arc<LakehouseContext>> = if roles.needs_lakehouse() {
        let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
            .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
        let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
            .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
        info!("Connecting to data lake (migrate_db + migrate_lakehouse)");
        let lake = connect_to_remote_data_lake(&connection_string, &object_store_uri)
            .await
            .with_context(|| "connecting to data lake")?;
        let lh = LakehouseContext::from_connection(Arc::new(lake))
            .await
            .with_context(|| "building LakehouseContext")?;
        Some(lh)
    } else {
        None
    };

    // Role-scoped auth providers (ingestion = machine-to-machine, analytics = human/tooling)
    let ingestion_auth = if roles.ingestion && !args.disable_auth && !args.disable_ingestion_auth {
        match provider_with_prefix("MICROMEGAS_INGESTION").await? {
            Some(p) => Some(p),
            None => {
                anyhow::bail!(
                    "Ingestion auth required but no providers configured. \
                     Set MICROMEGAS_INGESTION_API_KEYS, MICROMEGAS_API_KEYS, or --disable-auth"
                );
            }
        }
    } else {
        None
    };

    let analytics_auth = if roles.flightsql && !args.disable_auth {
        match provider_with_prefix("MICROMEGAS_ANALYTICS").await? {
            Some(p) => Some(p),
            None => {
                anyhow::bail!(
                    "Analytics auth required but no providers configured. \
                     Set MICROMEGAS_ANALYTICS_OIDC_CONFIG, MICROMEGAS_OIDC_CONFIG, or --disable-auth"
                );
            }
        }
    } else {
        None
    };

    // Resolve the analytics admin var once (web and FlightSQL share the same source)
    let analytics_admin_var = if std::env::var("MICROMEGAS_ANALYTICS_ADMINS").is_ok() {
        "MICROMEGAS_ANALYTICS_ADMINS".to_string()
    } else {
        "MICROMEGAS_ADMINS".to_string()
    };

    // One SIGTERM drives all roles
    let fanout = ShutdownFanout::new(wait_for_sigterm());

    let mut join_set: JoinSet<Result<()>> = JoinSet::new();

    // ── Ingestion ──────────────────────────────────────────────────────────
    if roles.ingestion {
        let lake = lakehouse
            .as_ref()
            .expect("lakehouse must be Some when ingestion role is enabled")
            .lake()
            .as_ref()
            .clone();
        let shutdown = fanout.subscribe();
        let listen_addr = args.listen_endpoint_http;
        let grace_c = grace;
        let auth = ingestion_auth;
        join_set.spawn(
            async move { serve_ingestion(listen_addr, lake, auth, shutdown, grace_c).await },
        );
    }

    // ── FlightSQL ──────────────────────────────────────────────────────────
    if roles.flightsql {
        let lh = lakehouse
            .as_ref()
            .expect("lakehouse must be Some when flightsql role is enabled")
            .clone();
        let shutdown = fanout.subscribe();
        let grace_c = grace;
        let auth = analytics_auth;
        join_set.spawn(async move {
            let mut builder = FlightSqlServer::builder()
                .with_lakehouse(lh)
                .with_shutdown(shutdown)
                .with_shutdown_grace(grace_c);
            if let Some(provider) = auth {
                builder = builder.with_auth_provider(provider);
            }
            builder.build_and_serve().await
        });
    }

    // ── Maintenance ────────────────────────────────────────────────────────
    if roles.maintenance {
        let lh = lakehouse
            .as_ref()
            .expect("lakehouse must be Some when maintenance role is enabled")
            .clone();
        let shutdown = fanout.subscribe();
        let grace_c = grace;
        join_set.spawn(async move {
            let view_factory =
                default_view_factory(lh.runtime().clone(), lh.lake().clone()).await?;
            let views_to_update = get_global_views_with_update_group(&view_factory);
            daemon(lh, views_to_update, shutdown, grace_c).await
        });
    }

    // ── Web ────────────────────────────────────────────────────────────────
    if roles.web {
        let cors_origin = std::env::var("MICROMEGAS_WEB_CORS_ORIGIN")
            .context("MICROMEGAS_WEB_CORS_ORIGIN environment variable not set")?;
        let base_path_raw = std::env::var("MICROMEGAS_BASE_PATH")
            .context("MICROMEGAS_BASE_PATH environment variable not set")?;
        let base_path = {
            let p = base_path_raw.trim_end_matches('/').to_string();
            if !p.is_empty() && !p.starts_with('/') {
                anyhow::bail!(
                    "MICROMEGAS_BASE_PATH must start with '/' (e.g., '/', '/micromegas')"
                );
            }
            p
        };
        let app_db_string = std::env::var("MICROMEGAS_APP_SQL_CONNECTION_STRING")
            .context("MICROMEGAS_APP_SQL_CONNECTION_STRING environment variable not set")?;
        let maps_uri = std::env::var("MICROMEGAS_MAPS_OBJECT_STORE_URI").ok();
        let max_upload_bytes = std::env::var("MICROMEGAS_MAPS_MAX_UPLOAD_BYTES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok());

        // Auto-seed the local FlightSQL data source when web + flightsql are both enabled
        if roles.flightsql
            && !args.no_seed_data_source
            && let Err(e) = seed_local_data_source(&app_db_string).await
        {
            warn!("Failed to seed local FlightSQL data source: {e}");
        }

        let shutdown = fanout.subscribe();
        let grace_c = grace;
        let admin_var = analytics_admin_var;
        let web_config = WebServerConfig {
            port: args.port,
            frontend_dir: args.frontend_dir.clone(),
            base_path,
            cors_origin,
            app_db_string,
            maps_uri,
            max_upload_bytes,
            disable_auth: args.disable_auth,
            admin_var_name: admin_var,
        };

        join_set.spawn(async move { run_web_server(web_config, shutdown, grace_c).await });
    }

    // Wait for all roles; fail-fast on first error
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                error!("Role failed: {e:#}");
                join_set.abort_all();
                return Err(e);
            }
            Err(join_err) => {
                error!("Role task panicked: {join_err}");
                join_set.abort_all();
                anyhow::bail!("Role task panicked: {join_err}");
            }
        }
    }

    info!("All roles shut down cleanly");
    Ok(())
}

// ---------------------------------------------------------------------------
// Default data-source seeding
// ---------------------------------------------------------------------------

/// Idempotent first-run seed: insert a "local" FlightSQL data source pointing
/// at the in-process loopback listener when the app DB has no data sources.
async fn seed_local_data_source(app_db_string: &str) -> Result<()> {
    let pool = sqlx::PgPool::connect(app_db_string)
        .await
        .with_context(|| "seed_local_data_source: connecting to app DB")?;
    // Ensure the schema exists before querying (idempotent)
    app_db::execute_migration(pool.clone()).await?;

    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM data_sources")
        .fetch_one(&pool)
        .await?;

    if count == 0 {
        sqlx::query(
            "INSERT INTO data_sources (name, config, is_default, created_by, updated_by) \
             VALUES ('local', $1, TRUE, 'monolith', 'monolith')",
        )
        .bind(serde_json::json!({"url": "http://127.0.0.1:50051"}))
        .execute(&pool)
        .await?;
        info!("Seeded local FlightSQL data source pointing at http://127.0.0.1:50051");
    }
    Ok(())
}
