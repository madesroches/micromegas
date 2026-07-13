//! Telemetry Ingestion Server
//!
//! Accepts telemetry data through http, stores the metadata in postgresql and the
//! raw event payload in the object store.
//!
//! Env variables:
//!  - `MICROMEGAS_SQL_CONNECTION_STRING` : to connect to postgresql
//!  - `MICROMEGAS_OBJECT_STORE_URI` : to write the payloads
//!  - `MICROMEGAS_API_KEYS` : (optional) JSON array of API keys
//!  - `MICROMEGAS_OIDC_CONFIG` : (optional) OIDC configuration JSON

#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::Result;
use clap::Parser;
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
        match micromegas::auth::default_provider::provider().await? {
            Some(p) => Some(p),
            None => {
                return Err("Authentication required but no auth providers configured. \
                     Set MICROMEGAS_API_KEYS or MICROMEGAS_OIDC_CONFIG, \
                     or use --disable-auth for development"
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
