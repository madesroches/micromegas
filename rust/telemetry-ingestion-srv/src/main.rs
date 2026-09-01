//! Telemetry Ingestion Server
//!
//! Accepts telemetry data through http, stores the metadata in postgresql and the
//! raw event payload in the object store.
//!
//! Env variables:
//!  - `MICROMEGAS_SQL_CONNECTION_STRING` : to connect to postgresql
//!  - `MICROMEGAS_OBJECT_STORE_URI` : to write the payloads
//!  - `MICROMEGAS_API_KEYS` : (optional) JSON array of API keys, legacy/bootstrap path
//!  - `MICROMEGAS_OIDC_CONFIG` : (optional) OIDC configuration JSON
//!
//! Authentication is satisfied by any of: `MICROMEGAS_API_KEYS`,
//! `MICROMEGAS_OIDC_CONFIG`, or a non-empty `ingestion_api_keys` DB table
//! — the last of these is always checked, since this binary always attaches a
//! DB-backed key store built from the data lake's own connection.

#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
micromegas::declare_jemalloc_conf!();

use anyhow::Result;
use clap::Parser;
use micromegas::auth::db_api_key::{ApiKeyTable, dedicated_key_store_pool};
use micromegas::auth::default_provider::ProviderBuilder;
use micromegas::ingestion::data_lake_config::DataLakeConfig;
use micromegas::ingestion::remote_data_lake::connect_to_remote_data_lake;
use micromegas::micromegas_main;
use micromegas::servers::ingestion::serve_ingestion;
use micromegas::tracing::prelude::*;
use std::net::SocketAddr;

#[derive(Parser, Debug)]
#[clap(name = "Telemetry Ingestion Server")]
#[clap(about = "Telemetry Ingestion Server", version, author)]
struct Cli {
    #[clap(long, default_value = "127.0.0.1:8081")]
    listen_endpoint_http: SocketAddr,

    /// Disable authentication (development mode only)
    #[clap(long)]
    disable_auth: bool,

    #[command(flatten)]
    common: micromegas::config::CommonServerArgs,
}

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();
    let cfg = DataLakeConfig::from_env()?;
    let data_lake =
        connect_to_remote_data_lake(&cfg.sql_connection_string, &cfg.object_store_uri).await?;

    let auth_provider = if args.disable_auth {
        info!("Authentication disabled (--disable-auth)");
        None
    } else {
        let key_store_pool = dedicated_key_store_pool(&data_lake.db_pool);
        match ProviderBuilder::new("")
            .with_db_key_store(key_store_pool, ApiKeyTable::Ingestion)
            .build()
            .await?
        {
            Some(p) => Some(p),
            None => {
                return Err("Authentication required but no auth providers configured. \
                     Set MICROMEGAS_API_KEYS or MICROMEGAS_OIDC_CONFIG, populate the \
                     ingestion_api_keys DB table, or use --disable-auth for development"
                    .into());
            }
        }
    };

    let grace = args.common.grace();
    serve_ingestion(
        args.listen_endpoint_http,
        data_lake,
        auth_provider,
        micromegas::servers::shutdown::wait_for_sigterm(),
        grace,
    )
    .await?;
    Ok(())
}
