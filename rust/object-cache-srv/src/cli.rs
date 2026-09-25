use crate::handlers::{permits_for_bytes, stream_window_bytes};
use anyhow::{Result, anyhow};
use clap::Parser;
use micromegas::object_cache::blocks::max_run_bytes;
use micromegas::object_cache::range_cache::{
    DEFAULT_DEMAND_RESERVED_FETCH_PERMITS, DEFAULT_FETCH_MEMORY_BUDGET_BYTES,
    DEFAULT_MAX_COALESCED_GET_BYTES, DEFAULT_PROMOTE_WHOLE_BATCH, DEFAULT_TOTAL_FETCH_PERMITS,
};
use std::net::SocketAddr;

/// One MiB, in bytes -- the unit every `*_mb`/`*_gb` CLI knob converts
/// through.
const MIB: u64 = 1024 * 1024;
/// Clap range for `--block-size` and `--max-coalesced-get-bytes`: capping
/// both at 1 GiB keeps `max_run_bytes` -- a byte count passed directly as
/// `acquire_many_owned`'s `u32` permit count -- well below `u32::MAX`.
const MAX_BYTES_KNOB: u64 = 1024 * 1024 * 1024; // 1 GiB

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
        default_value = "1048576",
        value_parser = clap::value_parser!(u64).range(1..=MAX_BYTES_KNOB)
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

    /// JSON array of keyring entries: `{"name","key","allowed_cidrs"?}`. `allowed_cidrs`
    /// (CIDR ranges or bare IPs this key may be presented from) is optional; absent/empty means
    /// unrestricted, the backward-compatible default for every entry written before this field
    /// existed.
    #[clap(long, env = "MICROMEGAS_API_KEYS", default_value = "")]
    pub api_keys: String,

    /// Disable authentication (development mode only)
    #[clap(long)]
    pub disable_auth: bool,

    #[command(flatten)]
    pub common: micromegas::config::CommonServerArgs,

    /// Total number of origin GETs allowed to run concurrently -- a
    /// parallelism cap; transient fetch memory is bounded separately by
    /// `--fetch-memory-budget-mb`.
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_MAX_CONCURRENT_FETCHES",
        default_value_t = DEFAULT_TOTAL_FETCH_PERMITS
    )]
    pub max_concurrent_fetches: usize,

    /// Origin-GET slots always available to demand reads; prefetch is capped
    /// at `max_concurrent_fetches - demand_reserved_fetches`. Each reserved
    /// slot also reserves one full-size run's worth of `--fetch-memory-budget-mb`.
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
        default_value_t = DEFAULT_MAX_COALESCED_GET_BYTES,
        value_parser = clap::value_parser!(u64).range(1..=MAX_BYTES_KNOB)
    )]
    pub max_coalesced_get_bytes: u64,

    /// Cap (MiB) on transient origin-GET buffer memory across all in-flight
    /// coalesced fetch runs -- independent of `--max-concurrent-fetches`,
    /// which only bounds parallelism. Default `256`: the smallest budget at
    /// which the default concurrency (32) stays fully usable for max-size
    /// (8 MiB) runs, so a cold contiguous scan is throttled by parallelism
    /// rather than memory, and a modest share next to the default RAM tier
    /// (`--ram-mb` 512) and streaming budget (`--memory-budget-mb` 1024).
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_FETCH_MEMORY_BUDGET_MB",
        default_value_t = DEFAULT_FETCH_MEMORY_BUDGET_BYTES / MIB,
        value_parser = clap::value_parser!(u64).range(1..=1_048_576)
    )]
    pub fetch_memory_budget_mb: u64,

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

impl Cli {
    /// Validate all numeric knobs. Fatal-at-startup config errors, kept next to
    /// the type and unit-testable from the integration-test crate (like
    /// `validate_write_tuning`).
    pub fn validate(&self) -> Result<()> {
        // `block_size` is the divisor for block-index math (`start / block_size`);
        // a value of 0 would panic on the first range read. Reject it at the startup
        // boundary as a fatal config error rather than letting it reach the cache.
        if self.block_size == 0 {
            return Err(anyhow!(
                "MICROMEGAS_OBJECT_CACHE_BLOCK_SIZE must be greater than 0"
            ));
        }

        if self.max_concurrent_fetches == 0 {
            return Err(anyhow!(
                "MICROMEGAS_OBJECT_CACHE_MAX_CONCURRENT_FETCHES must be greater than 0"
            ));
        }
        // `FetchScheduler` computes `total - demand_reserved` as a plain
        // subtraction; a misconfigured pair would panic deep inside the cache
        // instead of at startup.
        if self.demand_reserved_fetches >= self.max_concurrent_fetches {
            return Err(anyhow!(
                "MICROMEGAS_OBJECT_CACHE_DEMAND_RESERVED_FETCHES ({}) must be less than \
                 MICROMEGAS_OBJECT_CACHE_MAX_CONCURRENT_FETCHES ({})",
                self.demand_reserved_fetches,
                self.max_concurrent_fetches
            ));
        }
        // The byte-budget analog of the count-budget check above:
        // `FetchScheduler::new` asserts the same relationship and panics
        // deep inside the cache if it's violated, since a run larger than
        // the byte budget's prefetch pool would hang forever acquiring its
        // `bytes` permits (`acquire_many_owned` never completes past the
        // semaphore's total). `saturating_mul`/`saturating_add` since
        // `demand_reserved_fetches` is an unranged `usize`; a saturated
        // floor is simply rejected below.
        let max_run = max_run_bytes(self.block_size, self.max_coalesced_get_bytes);
        let floor_bytes = (self.demand_reserved_fetches as u64)
            .saturating_add(1)
            .saturating_mul(max_run);
        let budget_bytes = self.fetch_memory_budget_mb.saturating_mul(MIB);
        if budget_bytes < floor_bytes {
            let floor_mb = floor_bytes.div_ceil(MIB);
            return Err(anyhow!(
                "MICROMEGAS_OBJECT_CACHE_FETCH_MEMORY_BUDGET_MB ({}) must be at least {floor_mb} \
                 MiB -- the floor implied by MICROMEGAS_OBJECT_CACHE_DEMAND_RESERVED_FETCHES ({}), \
                 MICROMEGAS_OBJECT_CACHE_MAX_COALESCED_GET_BYTES ({}), and \
                 MICROMEGAS_OBJECT_CACHE_BLOCK_SIZE ({}): each reserved demand slot must fit one \
                 full-size run, or a run could hang forever acquiring its byte-budget permits",
                self.fetch_memory_budget_mb,
                self.demand_reserved_fetches,
                self.max_coalesced_get_bytes,
                self.block_size,
            ));
        }
        // A zero budget would make every non-empty data request hang forever
        // acquiring its mem_permits charge while /health and /ready still pass;
        // fail at startup instead.
        if self.memory_budget_mb == 0 {
            return Err(anyhow!(
                "MICROMEGAS_OBJECT_CACHE_MEMORY_BUDGET_MB must be greater than 0"
            ));
        }
        // A large streaming read still charges a full window's worth of permits
        // (`stream_window_bytes`, capped rather than rejected outright — see
        // `handlers::stream_window_bytes`), and `Semaphore::acquire_many_owned`
        // never completes (and never errors) if the requested count exceeds the
        // semaphore's total permits. Without this floor, a deployment configured
        // with a smaller `--memory-budget-mb` would hang every large read
        // instead of failing fast here at startup.
        let window_mb = permits_for_bytes(stream_window_bytes(self.block_size));
        if self.memory_budget_mb < window_mb {
            return Err(anyhow!(
                "MICROMEGAS_OBJECT_CACHE_MEMORY_BUDGET_MB ({}) must be at least {window_mb} MiB \
                 (2 * DEMAND_WINDOW_BLOCKS * block_size, the largest charge a single streaming \
                 request can make), or every large read would hang acquiring mem_permits",
                self.memory_budget_mb
            ));
        }
        if self.prefetch_queue_capacity == 0 {
            return Err(anyhow!(
                "MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY must be greater than 0"
            ));
        }
        if self.prefetch_worker_concurrency == 0 {
            return Err(anyhow!(
                "MICROMEGAS_OBJECT_CACHE_PREFETCH_WORKER_CONCURRENCY must be greater than 0"
            ));
        }
        validate_write_tuning(self.flushers, self.write_buffer_mb)
    }
}
