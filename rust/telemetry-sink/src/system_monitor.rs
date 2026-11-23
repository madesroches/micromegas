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

/// Subset of cgroup v2 `cpu.stat` that is useful for spotting CPU throttling.
#[derive(Debug, PartialEq)]
pub struct CgroupCpuStat {
    pub usage_usec: u64,
    pub nr_throttled: Option<u64>,
    pub throttled_usec: Option<u64>,
}

/// Parses the body of a cgroup v2 `cpu.stat` file. `usage_usec` is required;
/// the throttling counters are absent when the cpu controller has no limit.
pub fn parse_cpu_stat(body: &str) -> Option<CgroupCpuStat> {
    let mut usage_usec = None;
    let mut nr_throttled = None;
    let mut throttled_usec = None;
    for line in body.lines() {
        let mut parts = line.split_whitespace();
        let (Some(key), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else {
            continue;
        };
        match key {
            "usage_usec" => usage_usec = Some(value),
            "nr_throttled" => nr_throttled = Some(value),
            "throttled_usec" => throttled_usec = Some(value),
            _ => {}
        }
    }
    Some(CgroupCpuStat {
        usage_usec: usage_usec?,
        nr_throttled,
        throttled_usec,
    })
}

const ROOT_CPU_STAT_PATH: &str = "/sys/fs/cgroup/cpu.stat";

/// Resolves this process's own cgroup `cpu.stat` from the contents of
/// `/proc/self/cgroup`. Without a cgroup namespace the root file would report
/// host-wide usage, so the `0::<path>` line is followed; the root file is the
/// fallback when there is no v2 line.
pub fn cgroup_cpu_stat_path(proc_self_cgroup: &str) -> String {
    let Some(rel) = proc_self_cgroup
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .map(|p| p.trim().trim_matches('/'))
    else {
        return ROOT_CPU_STAT_PATH.to_string();
    };
    if rel.is_empty() {
        ROOT_CPU_STAT_PATH.to_string()
    } else {
        format!("/sys/fs/cgroup/{rel}/cpu.stat")
    }
}

/// Emits the cgroup's cumulative CPU counters. Emits nothing when the file is
/// missing (non-Linux, cgroup v1, no cgroup).
pub fn emit_cgroup_cpu_stats() {
    let own_path = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .map(|c| cgroup_cpu_stat_path(&c));
    let Ok(body) = own_path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map_or_else(|| std::fs::read_to_string(ROOT_CPU_STAT_PATH), Ok)
    else {
        return;
    };
    let Some(stat) = parse_cpu_stat(&body) else {
        return;
    };
    imetric!("cgroup_cpu_usage_usec", "us", stat.usage_usec);
    if let Some(v) = stat.nr_throttled {
        imetric!("cgroup_cpu_nr_throttled", "count", v);
    }
    if let Some(v) = stat.throttled_usec {
        imetric!("cgroup_cpu_throttled_usec", "us", v);
    }
}

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
            emit_cgroup_cpu_stats();
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
