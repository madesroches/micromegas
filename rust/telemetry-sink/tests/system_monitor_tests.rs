//! Coverage for the process-memory gauges and tick-gating in
//! `system_monitor.rs`. No feature requirement and no platform gate:
//! `emit_process_memory_stats` reads from the OS via `sysinfo` regardless of
//! which allocator is active, and `should_sample_slow` is plain arithmetic.

use micromegas_telemetry_sink::system_monitor::{emit_process_memory_stats, should_sample_slow};
use micromegas_tracing::dispatch::flush_metrics_buffer;
use micromegas_tracing::event::in_memory_sink::InMemorySink;
use micromegas_tracing::metrics::MetricsMsgQueueAny;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;
use sysinfo::System;

/// Values fired for the untagged integer metric `name` since the guard was
/// created. Requires `flush_metrics_buffer` first.
fn integer_metric_values(sink: &InMemorySink, name: &str) -> Vec<u64> {
    let state = sink.state.lock().expect("sink lock");
    let mut out = Vec::new();
    for block in &state.metrics_blocks {
        for evt in block.events.iter() {
            if let MetricsMsgQueueAny::IntegerMetricEvent(e) = evt
                && e.desc.name == name
            {
                out.push(e.value);
            }
        }
    }
    out
}

#[test]
#[serial]
fn process_memory_gauges_fire_with_nonzero_values() {
    let guard = init_in_memory_tracing();

    let mut system = System::new();
    emit_process_memory_stats(&mut system);
    flush_metrics_buffer();

    let resident = integer_metric_values(&guard.sink, "process_resident_bytes");
    assert_eq!(resident.len(), 1, "process_resident_bytes must fire once");
    assert!(
        resident[0] > 0,
        "any running process has nonzero RSS: {resident:?}"
    );

    let virt = integer_metric_values(&guard.sink, "process_virtual_bytes");
    assert_eq!(virt.len(), 1, "process_virtual_bytes must fire once");
    assert!(
        virt[0] > 0,
        "any running process has nonzero virtual size: {virt:?}"
    );
}

#[test]
fn should_sample_slow_fires_at_or_past_the_stated_interval() {
    use std::time::Duration;

    assert!(should_sample_slow(Duration::from_secs(5)));
    assert!(should_sample_slow(Duration::from_secs(6)));
    assert!(!should_sample_slow(Duration::from_millis(200)));
    assert!(!should_sample_slow(Duration::from_secs(4)));
    assert!(!should_sample_slow(Duration::ZERO));
}
