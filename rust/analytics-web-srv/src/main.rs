#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use analytics_web_srv::web_server::{WebCliArgs, WebServerConfig, run_web_server};
use anyhow::Result;
use clap::Parser;
use micromegas::micromegas_main;
use micromegas::servers::shutdown::wait_for_sigterm;

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

    #[command(flatten)]
    common: micromegas::config::CommonServerArgs,
}

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config = WebServerConfig::from_cli_and_env(WebCliArgs {
        port: args.port,
        frontend_dir: args.frontend_dir,
        disable_auth: args.disable_auth,
        admin_var_name: "MICROMEGAS_ADMINS".to_string(),
    })?;

    run_web_server(config, wait_for_sigterm(), args.common.grace()).await
}
