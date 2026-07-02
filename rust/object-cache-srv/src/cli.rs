use clap::Parser;
use std::net::SocketAddr;

#[derive(Parser, Debug)]
#[clap(name = "micromegas-object-cache-srv")]
#[clap(about = "Shared object range cache service", version, author)]
pub(crate) struct Cli {
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_LISTEN",
        default_value = "0.0.0.0:8080"
    )]
    pub(crate) listen: SocketAddr,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_ORIGIN_URI")]
    pub(crate) origin_uri: String,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_RAM_MB", default_value = "512")]
    pub(crate) ram_mb: usize,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_DISK_PATH")]
    pub(crate) disk_path: String,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_DISK_GB", default_value = "50")]
    pub(crate) disk_gb: usize,

    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_BLOCK_SIZE",
        default_value = "1048576"
    )]
    pub(crate) block_size: u64,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_NAMESPACE", default_value = "")]
    pub(crate) namespace: String,

    /// Allowed key prefixes (repeat `--prefix`, or comma-separate the env var,
    /// e.g. `blobs,views`). A key is served only if it equals a prefix or lies
    /// under `{prefix}/`. Empty by default: the server refuses to start unless
    /// at least one prefix is set or `--allow-all-prefixes` is passed.
    #[clap(
        long = "prefix",
        env = "MICROMEGAS_OBJECT_CACHE_PREFIX",
        value_delimiter = ','
    )]
    pub(crate) allowed_prefixes: Vec<String>,

    /// Serve the entire bucket, bypassing prefix containment (development mode
    /// only). Mirrors `--disable-auth`: an explicit opt-out, never the default.
    #[clap(long)]
    pub(crate) allow_all_prefixes: bool,

    #[clap(long, env = "MICROMEGAS_API_KEYS", default_value = "")]
    pub(crate) api_keys: String,

    /// Disable authentication (development mode only)
    #[clap(long)]
    pub(crate) disable_auth: bool,

    #[clap(
        long,
        default_value = "25",
        env = "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS"
    )]
    pub(crate) shutdown_grace_period_seconds: u64,
}
