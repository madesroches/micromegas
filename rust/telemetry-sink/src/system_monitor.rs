use micromegas_tracing::{fmetric, imetric};
use std::time::{Duration, Instant};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

/// The sampling frequency for the process/jemalloc memory gauges -- much
/// coarser than the loop's own ~200ms `sysinfo` CPU-update floor (that floor
/// is only needed for a valid CPU-usage delta). Matches the cadence
/// object-cache-srv's saturation_monitor.rs already chose for the same
/// telemetry-volume reason; these gauges don't need 200ms resolution to
/// catch an hour-plus OOM climb.
const SLOW_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// True once at least `SLOW_SAMPLE_INTERVAL` has elapsed since the last slow
/// sample. Extracted as a pure function (mirroring `saturation_monitor.rs`'s
/// `sample_once` extraction) so the gating decision itself is directly
/// testable with plain `Duration` values, rather than only reachable from
/// inside the infinite loop below.
pub fn should_sample_slow(elapsed_since_last_sample: Duration) -> bool {
    elapsed_since_last_sample >= SLOW_SAMPLE_INTERVAL
}

/// Emits this process's own RSS/virtual memory size. Allocator-agnostic
/// (reads from the OS via `sysinfo`), so this runs for every consumer of
/// `spawn_system_monitor`, including non-jemalloc binaries.
///
/// Takes a caller-owned `System` so repeated calls reuse it instead of each
/// constructing a fresh instance (which re-reads `/proc/stat` etc.).
pub fn emit_process_memory_stats(system: &mut System) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
    let Ok(pid) = sysinfo::get_current_pid() else {
        return;
    };
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        false,
        ProcessRefreshKind::nothing().with_memory(),
    );
    if let Some(process) = system.process(pid) {
        imetric!("process_resident_bytes", "bytes", process.memory());
        imetric!("process_virtual_bytes", "bytes", process.virtual_memory());
    }
}

/// Emits jemalloc's own runtime accounting (`stats.allocated`/`resident`/
/// `mapped`/`retained`). Only meaningful when jemalloc is this process's
/// global allocator, hence the feature + platform gate.
#[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
pub fn emit_jemalloc_stats() {
    use tikv_jemalloc_ctl::{epoch, stats};
    // jemalloc caches these counters; advance the epoch to refresh them
    // before reading, per tikv-jemalloc-ctl's documented usage.
    if epoch::advance().is_err() {
        return;
    }
    if let Ok(v) = stats::allocated::read() {
        imetric!("jemalloc_allocated_bytes", "bytes", v as u64);
    }
    if let Ok(v) = stats::resident::read() {
        imetric!("jemalloc_resident_bytes", "bytes", v as u64);
    }
    if let Ok(v) = stats::mapped::read() {
        imetric!("jemalloc_mapped_bytes", "bytes", v as u64);
    }
    if let Ok(v) = stats::retained::read() {
        imetric!("jemalloc_retained_bytes", "bytes", v as u64);
    }
}

#[cfg(not(all(feature = "jemalloc", not(target_os = "windows"))))]
pub fn emit_jemalloc_stats() {}

/// Continuously sends system-wide CPU and memory usage metrics.
///
/// This function runs in a loop, refreshing system information at regular intervals
/// and emitting `imetric!` and `fmetric!` events for total memory, used memory,
/// free memory, and global CPU usage. Every `SLOW_SAMPLE_INTERVAL`, it also
/// emits this process's RSS/virtual memory size and, when built with the
/// `jemalloc` feature on a non-Windows target, jemalloc's own runtime stats.
pub fn send_system_metrics_forever() {
    let what_to_refresh = RefreshKind::nothing()
        .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
        .with_memory(MemoryRefreshKind::nothing().with_ram());
    let mut system = System::new_with_specifics(what_to_refresh);
    imetric!("total_memory", "bytes", system.total_memory());
    let mut process_system = System::new();
    let mut last_slow_sample = Instant::now();
    loop {
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        system.refresh_specifics(what_to_refresh);
        imetric!("used_memory", "bytes", system.used_memory());
        imetric!("free_memory", "bytes", system.free_memory());
        fmetric!("cpu_usage", "percent", system.global_cpu_usage() as f64);

        if should_sample_slow(last_slow_sample.elapsed()) {
            emit_process_memory_stats(&mut process_system);
            emit_jemalloc_stats();
            last_slow_sample = Instant::now();
        }
    }
}

/// Spawns a new thread to run the `send_system_metrics_forever` function.
///
/// This allows system metrics to be collected and reported in the background.
pub fn spawn_system_monitor() {
    std::thread::spawn(send_system_metrics_forever);
}
