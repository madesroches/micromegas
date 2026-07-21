//! Coverage for the jemalloc runtime-stats gauges in `system_monitor.rs`.
//! `required-features = ["jemalloc"]` (set on this test's `[[test]]` entry in
//! `Cargo.toml`) gates the whole file; `#![cfg(not(target_os = "windows"))]`
//! makes it compile to an empty harness on Windows, where the `jemalloc`
//! feature can still be unified in transitively but `tikv-jemallocator` is
//! not available as a dev-dependency.
//!
//! This test binary carries its own `#[global_allocator]` declaration
//! (production services declare theirs in their own bin entry point, not in
//! this library) so that jemalloc's `stats.*` counters genuinely reflect
//! this process's allocation activity.
#![cfg(not(target_os = "windows"))]

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use micromegas_telemetry_sink::system_monitor::emit_jemalloc_stats;
use micromegas_tracing::dispatch::flush_metrics_buffer;
use micromegas_tracing::event::in_memory_sink::InMemorySink;
use micromegas_tracing::metrics::MetricsMsgQueueAny;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;

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
fn jemalloc_allocated_bytes_grows_after_a_large_allocation() {
    let guard = init_in_memory_tracing();

    emit_jemalloc_stats();
    flush_metrics_buffer();
    let before = integer_metric_values(&guard.sink, "jemalloc_allocated_bytes");
    assert_eq!(before.len(), 1, "jemalloc_allocated_bytes must fire once");

    let big = vec![7u8; 8 * 1024 * 1024];
    let big = std::hint::black_box(big);

    emit_jemalloc_stats();
    flush_metrics_buffer();
    let after = integer_metric_values(&guard.sink, "jemalloc_allocated_bytes");
    assert_eq!(after.len(), 2, "jemalloc_allocated_bytes must fire again");
    assert!(
        after[1] > before[0] + 1024 * 1024,
        "allocating 8MiB must grow jemalloc's reported allocated bytes by a plausible margin: before={before:?} after={after:?}"
    );

    drop(big);
}

#[test]
#[serial]
fn all_four_jemalloc_gauges_fire_exactly_once_per_call() {
    let guard = init_in_memory_tracing();

    emit_jemalloc_stats();
    flush_metrics_buffer();

    for name in [
        "jemalloc_allocated_bytes",
        "jemalloc_resident_bytes",
        "jemalloc_mapped_bytes",
        "jemalloc_retained_bytes",
    ] {
        let values = integer_metric_values(&guard.sink, name);
        assert_eq!(values.len(), 1, "{name} must fire exactly once: {values:?}");
    }
}
