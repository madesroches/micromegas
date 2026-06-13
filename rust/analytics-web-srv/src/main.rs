#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use analytics_web_srv::web_server::{WebServerConfig, run_web_server};
use anyhow::{Context, Result};
use clap::Parser;
use micromegas::micromegas_main;
use micromegas::servers::shutdown::wait_for_sigterm;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Server port
    #[arg(short, long, default_value = "3000", env = "MICROMEGAS_PORT")]
    port: u16,

    /// Frontend build directory
    #[arg(long, default_value = "../analytics-web-app/dist")]
    frontend_dir: String,

    /// Disable authentication (development only)
    #[arg(long)]
    disable_auth: bool,

    /// Seconds to wait for in-flight requests to complete after SIGTERM
    #[arg(
        long,
        default_value = "25",
        env = "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS"
    )]
    shutdown_grace_period_seconds: u64,
}

fn read_base_path() -> Result<String> {
    let raw = std::env::var("MICROMEGAS_BASE_PATH")
        .context("MICROMEGAS_BASE_PATH environment variable not set")?;
    let base_path = raw.trim_end_matches('/').to_string();
    if !base_path.is_empty() && !base_path.starts_with('/') {
        anyhow::bail!("MICROMEGAS_BASE_PATH must start with '/' (e.g., '/', '/micromegas')");
    }
    Ok(base_path)
}

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let cors_origin = std::env::var("MICROMEGAS_WEB_CORS_ORIGIN")
        .context("MICROMEGAS_WEB_CORS_ORIGIN environment variable not set")?;
    let base_path = read_base_path()?;
    let app_db_string = std::env::var("MICROMEGAS_APP_SQL_CONNECTION_STRING")
        .context("MICROMEGAS_APP_SQL_CONNECTION_STRING environment variable not set")?;
    let maps_uri = std::env::var("MICROMEGAS_MAPS_OBJECT_STORE_URI").ok();
    let max_upload_bytes = std::env::var("MICROMEGAS_MAPS_MAX_UPLOAD_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok());

    let grace = Duration::from_secs(args.shutdown_grace_period_seconds);
    let config = WebServerConfig {
        port: args.port,
        frontend_dir: args.frontend_dir,
        base_path,
        cors_origin,
        app_db_string,
        maps_uri,
        max_upload_bytes,
        disable_auth: args.disable_auth,
        admin_var_name: "MICROMEGAS_ADMINS".to_string(),
    };

    run_web_server(config, wait_for_sigterm(), grace).await
}
