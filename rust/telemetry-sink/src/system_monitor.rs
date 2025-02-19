use micromegas_tracing::{fmetric, imetric};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

pub fn send_system_metrics_forever() {
    let what_to_refresh = RefreshKind::nothing()
        .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
        .with_memory(MemoryRefreshKind::nothing().with_ram());
    let mut system = System::new_with_specifics(what_to_refresh);
    imetric!("total_memory", "bytes", system.total_memory());
    loop {
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        system.refresh_specifics(what_to_refresh);
        imetric!("used_memory", "bytes", system.used_memory());
        imetric!("free_memory", "bytes", system.free_memory());
        fmetric!("cpu_usage", "percent", system.global_cpu_usage() as f64);
    }
}

pub fn spawn_system_monitor() {
    std::thread::spawn(send_system_metrics_forever);
}
