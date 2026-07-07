use anyhow::{Result, anyhow};
use clap::Parser;
use micromegas_object_cache::range_cache::{
    DEFAULT_DEMAND_RESERVED_FETCH_PERMITS, DEFAULT_MAX_COALESCED_GET_BYTES,
    DEFAULT_PROMOTE_WHOLE_BATCH, DEFAULT_TOTAL_FETCH_PERMITS,
};
use std::net::SocketAddr;

#[derive(Parser, Debug)]
#[clap(name = "micromegas-object-cache-srv")]
#[clap(about = "Shared object range cache service", version, author)]
pub struct Cli {
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_LISTEN",
        default_value = "0.0.0.0:8080"
    )]
    pub listen: SocketAddr,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_ORIGIN_URI")]
    pub origin_uri: String,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_RAM_MB", default_value = "512")]
    pub ram_mb: usize,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_DISK_PATH")]
    pub disk_path: String,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_DISK_GB", default_value = "50")]
    pub disk_gb: usize,

    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_BLOCK_SIZE",
        default_value = "1048576"
    )]
    pub block_size: u64,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_NAMESPACE", default_value = "")]
    pub namespace: String,

    /// Allowed key prefixes (repeat `--prefix`, or comma-separate the env var,
    /// e.g. `blobs,views`). A key is served only if it equals a prefix or lies
    /// under `{prefix}/`. Empty by default: the server refuses to start unless
    /// at least one prefix is set or `--allow-all-prefixes` is passed.
    #[clap(
        long = "prefix",
        env = "MICROMEGAS_OBJECT_CACHE_PREFIX",
        value_delimiter = ','
    )]
    pub allowed_prefixes: Vec<String>,

    /// Serve the entire bucket, bypassing prefix containment (development mode
    /// only). Mirrors `--disable-auth`: an explicit opt-out, never the default.
    #[clap(long)]
    pub allow_all_prefixes: bool,

    #[clap(long, env = "MICROMEGAS_API_KEYS", default_value = "")]
    pub api_keys: String,

    /// Disable authentication (development mode only)
    #[clap(long)]
    pub disable_auth: bool,

    #[clap(
        long,
        default_value = "25",
        env = "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS"
    )]
    pub shutdown_grace_period_seconds: u64,

    /// Total number of origin GETs allowed to run concurrently.
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_MAX_CONCURRENT_FETCHES",
        default_value_t = DEFAULT_TOTAL_FETCH_PERMITS
    )]
    pub max_concurrent_fetches: usize,

    /// Origin-GET slots always available to demand reads; prefetch is capped
    /// at `max_concurrent_fetches - demand_reserved_fetches`.
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_DEMAND_RESERVED_FETCHES",
        default_value_t = DEFAULT_DEMAND_RESERVED_FETCH_PERMITS
    )]
    pub demand_reserved_fetches: usize,

    /// Max byte span of one coalesced run GET; larger contiguous runs are
    /// split at block boundaries.
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_MAX_COALESCED_GET_BYTES",
        default_value_t = DEFAULT_MAX_COALESCED_GET_BYTES
    )]
    pub max_coalesced_get_bytes: u64,

    /// Cross-request cap (MiB) on concurrent in-flight streaming windows: a
    /// small response charges close to its actual size, while a large one
    /// clamps to a fixed per-stream window, so this bounds concurrent
    /// large-streaming-request memory rather than total response bytes.
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_MEMORY_BUDGET_MB",
        default_value = "1024"
    )]
    pub memory_budget_mb: u32,

    /// On a demand hit into a prefetch batch, promote the whole batch
    /// (anticipatory) instead of only the covering run (default, precise).
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_PROMOTE_WHOLE_BATCH",
        default_value_t = DEFAULT_PROMOTE_WHOLE_BATCH,
        action = clap::ArgAction::Set
    )]
    pub promote_whole_batch: bool,

    /// Depth of the bounded `/prefetch` queue; items beyond this are
    /// load-shed (counted as `dropped`) rather than blocking the caller.
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY",
        default_value = "4096"
    )]
    pub prefetch_queue_capacity: usize,

    /// Concurrent in-flight prefetch fills the queue worker drives. A soft
    /// knob; the hard ceiling remains the scheduler's prefetch permits.
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_PREFETCH_WORKER_CONCURRENCY",
        default_value = "8"
    )]
    pub prefetch_worker_concurrency: usize,

    /// foyer disk-engine flusher count (`BlockEngineConfig::with_flushers`),
    /// roughly 1 per vCPU on the deployment-tuned target box. More flushers
    /// let more blocks be written to disk concurrently, raising the write
    /// throughput the submit queue can drain before overflowing.
    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_FLUSHERS", default_value = "2")]
    pub flushers: usize,

    /// foyer disk-engine flush buffer pool size, in MiB
    /// (`BlockEngineConfig::with_buffer_pool_size`). The submit-queue
    /// overflow threshold is set to 2x this value. foyer splits the pool as
    /// `buffer_pool_size / flushers`, and the engine block size is 16 MiB
    /// (`BlockEngineConfig` default) -- 128 MiB / 2 flushers gives each
    /// flusher a 4-block buffer.
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_WRITE_BUFFER_MB",
        default_value = "128"
    )]
    pub write_buffer_mb: usize,
}

/// Validate the write-tuning knobs added for the foyer 0.22 upgrade,
/// mirroring the fatal `anyhow!` startup guards in `object_cache_srv.rs`'s
/// `main` for the other numeric knobs. Split out as a plain function (rather
/// than inlined in `main`) so it is directly unit-testable from the
/// integration-test crate.
pub fn validate_write_tuning(flushers: usize, write_buffer_mb: usize) -> Result<()> {
    if flushers == 0 {
        return Err(anyhow!(
            "MICROMEGAS_OBJECT_CACHE_FLUSHERS must be greater than 0"
        ));
    }
    if write_buffer_mb == 0 {
        return Err(anyhow!(
            "MICROMEGAS_OBJECT_CACHE_WRITE_BUFFER_MB must be greater than 0"
        ));
    }
    Ok(())
}
