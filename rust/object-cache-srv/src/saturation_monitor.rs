//! Periodic saturation gauges for the object cache: fetch-budget occupancy,
//! in-flight entries, accounted RAM-tier usage, memory-budget occupancy,
//! prefetch queue depth, host-level NIC throughput, and the foyer disk
//! engine's own write-path throughput -- the signals #1206 calls out as
//! missing for locating a bottleneck (a solid *counter* layer already
//! exists, but no saturation/queue-depth signal). Modeled on
//! `telemetry-sink::system_monitor::send_system_metrics_forever`: a
//! background task that wakes on an interval and emits `imetric!`/`fmetric!`.
//!
//! The disk gauges used to come from `sysinfo::Disks`, but that reads 0 in
//! the deployed container (the cache device/mount isn't enumerated there);
//! they are now sourced from the foyer engine's own `Statistics` via
//! `RangeCache::backend_disk_stats`, which measures the cache's own device
//! I/O directly instead of relying on host disk enumeration.

use std::sync::Arc;
use std::time::Duration;

use micromegas::object_cache::backend::BackendDiskStats;
use micromegas::object_cache::prefetch::PrefetchItem;
use micromegas::object_cache::range_cache::RangeCache;
use micromegas::tracing::prelude::*;
use sysinfo::Networks;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;

/// How often the sampler wakes to emit gauges.
///
/// There's no existing cadence to match: the `sysinfo` sampler in
/// `system_monitor.rs` sleeps for `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`
/// (~200ms on Linux), but that floor exists only because it's the minimum
/// needed for a valid CPU-usage delta -- it doesn't apply here. 5s is a
/// telemetry-volume tradeoff instead: a shorter interval gives finer
/// saturation resolution at more telemetry volume.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// Emit one round of saturation gauges. Split out from the spawn loop so it
/// can be driven directly (see `saturation_monitor` tests). `pub` so the
/// `object-cache-srv/tests/` integration-test crate can drive it directly.
pub fn sample_once(
    cache: &RangeCache,
    mem_permits: &Semaphore,
    memory_budget_mb: u32,
    prefetch_tx: &mpsc::Sender<PrefetchItem>,
    networks: &mut Networks,
    prev_disk_stats: &mut Option<BackendDiskStats>,
    interval_secs: f64,
) {
    let (shared_available, shared_total, prefetch_available, prefetch_total) =
        cache.fetch_budget_stats();
    imetric!(
        "object_cache_fetch_shared_occupancy",
        "count",
        (shared_total - shared_available) as u64
    );
    imetric!(
        "object_cache_fetch_shared_available",
        "count",
        shared_available as u64
    );
    imetric!(
        "object_cache_fetch_prefetch_occupancy",
        "count",
        (prefetch_total - prefetch_available) as u64
    );
    imetric!(
        "object_cache_fetch_prefetch_available",
        "count",
        prefetch_available as u64
    );

    imetric!(
        "object_cache_inflight_entries",
        "count",
        cache.inflight_len() as u64
    );

    if let Some(ram_bytes) = cache.backend_ram_usage() {
        imetric!(
            "object_cache_ram_tier_usage_bytes",
            "bytes",
            ram_bytes as u64
        );
    }

    if let Some(ram_entries) = cache.backend_ram_entries() {
        imetric!("object_cache_ram_tier_entries", "count", ram_entries as u64);
    }

    let mem_available = mem_permits.available_permits() as u32;
    let mem_occupancy = memory_budget_mb.saturating_sub(mem_available);
    imetric!(
        "object_cache_mem_budget_occupancy_mb",
        "megabytes",
        mem_occupancy as u64
    );
    imetric!(
        "object_cache_mem_budget_available_mb",
        "megabytes",
        mem_available as u64
    );

    let queue_depth = prefetch_tx.max_capacity() - prefetch_tx.capacity();
    imetric!(
        "object_cache_prefetch_queue_depth",
        "count",
        queue_depth as u64
    );

    // NIC: the expected ceiling on the target im4gn.large (#1197) and
    // currently unmeasured. `Networks::refresh` computes the byte delta
    // since the previous refresh internally, so dividing by the sampler's
    // fixed interval gives a rate.
    networks.refresh(false);
    let (rx_bytes, tx_bytes) = networks.list().values().fold((0u64, 0u64), |(rx, tx), n| {
        (rx + n.received(), tx + n.transmitted())
    });
    fmetric!(
        "object_cache_nic_rx_bytes_per_sec",
        "bytes_per_sec",
        rx_bytes as f64 / interval_secs
    );
    fmetric!(
        "object_cache_nic_tx_bytes_per_sec",
        "bytes_per_sec",
        tx_bytes as f64 / interval_secs
    );

    // Foyer disk engine write-path throughput: the counters are cumulative
    // since process start, so a rate needs a delta against the previous
    // sample. `prev_disk_stats` starts `None` (no rate on the first tick),
    // and stays `None` for a backend with no disk tier (e.g. `MemoryBackend`
    // in tests), in which case nothing is emitted here.
    let current_disk_stats = cache.backend_disk_stats();
    if let (Some(prev), Some(current)) = (*prev_disk_stats, current_disk_stats) {
        fmetric!(
            "object_cache_foyer_disk_write_bytes_per_sec",
            "bytes_per_sec",
            current.write_bytes.saturating_sub(prev.write_bytes) as f64 / interval_secs
        );
        fmetric!(
            "object_cache_foyer_disk_read_bytes_per_sec",
            "bytes_per_sec",
            current.read_bytes.saturating_sub(prev.read_bytes) as f64 / interval_secs
        );
        fmetric!(
            "object_cache_foyer_disk_write_ios_per_sec",
            "ops_per_sec",
            current.write_ios.saturating_sub(prev.write_ios) as f64 / interval_secs
        );
        fmetric!(
            "object_cache_foyer_disk_read_ios_per_sec",
            "ops_per_sec",
            current.read_ios.saturating_sub(prev.read_ios) as f64 / interval_secs
        );
    }
    *prev_disk_stats = current_disk_stats;
}

/// Spawn the periodic saturation sampler, waking every `SAMPLE_INTERVAL`.
/// Runs for the lifetime of the process; the returned handle is not expected
/// to be awaited (the caller may drop it -- the spawned task keeps running
/// detached, like `prefetch_queue::spawn_prefetch_worker`'s worker).
pub fn spawn_saturation_monitor(
    cache: RangeCache,
    mem_permits: Arc<Semaphore>,
    memory_budget_mb: u32,
    prefetch_tx: mpsc::Sender<PrefetchItem>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut networks = Networks::new_with_refreshed_list();
        let mut prev_disk_stats: Option<BackendDiskStats> = None;
        loop {
            tokio::time::sleep(SAMPLE_INTERVAL).await;
            sample_once(
                &cache,
                &mem_permits,
                memory_budget_mb,
                &prefetch_tx,
                &mut networks,
                &mut prev_disk_stats,
                SAMPLE_INTERVAL.as_secs_f64(),
            );
        }
    })
}
