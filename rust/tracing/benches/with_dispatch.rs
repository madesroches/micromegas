use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicU32},
};

use criterion::{Criterion, criterion_group, criterion_main};
use micromegas_tracing::{
    dispatch::log_interop,
    error,
    event::NullEventSink,
    guards::{TracingSystemGuard, TracingThreadGuard},
    imetric,
    levels::Level,
    logs::{FILTER_LEVEL_UNSET_VALUE, LogMetadata},
    span_scope, span_scope_named, span_scope_named_dyn,
};

pub fn criterion_benchmark(c: &mut Criterion) {
    let _tracing_guard = TracingSystemGuard::new(
        10 * 1024 * 1024,
        1024 * 1024,
        10 * 1024 * 1024,
        Arc::new(NullEventSink {}),
        HashMap::new(),
        true, // Enable CPU tracing for benchmark
    );
    let _thread_guard = TracingThreadGuard::new();

    c.bench_function("dispatch/log", |b| {
        b.iter(|| {
            error!("test");
        });
    });
    c.bench_function("dispatch/metric", |b| {
        b.iter(|| {
            imetric!("name", "unit", 0);
        });
    });
    let correlation_id = String::from("c0ffee");
    c.bench_function("dispatch/metric_dyn_prop", |b| {
        b.iter(|| {
            imetric!("name", "unit", correlation_id = correlation_id, 0);
        });
    });
    let target = String::from("bench::interop");
    let interop_desc = LogMetadata {
        level: Level::Info,
        level_filter: AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "",
        target: &target,
        module_path: "",
        file: "",
        line: 0,
    };
    let interop_msg = String::from("test");
    c.bench_function("dispatch/log_interop", |b| {
        b.iter(|| {
            log_interop(&interop_desc, format_args!("{interop_msg}"));
        });
    });
    c.bench_function("dispatch/span_scope", |b| {
        b.iter(|| {
            span_scope!("test");
        });
    });
    c.bench_function("dispatch/span_scope_named", |b| {
        b.iter(|| {
            span_scope_named!("test");
        });
    });
    let name = String::from("test");
    c.bench_function("dispatch/span_scope_named_dyn", |b| {
        b.iter(|| {
            span_scope_named_dyn!(&name);
        });
    });
    let names: Vec<String> = (0..10_000).map(|i| format!("span_name_{i}")).collect();
    let mut next = 0;
    c.bench_function("dispatch/span_scope_named_dyn_high_card", |b| {
        b.iter(|| {
            next = (next + 1) % names.len();
            span_scope_named_dyn!(&names[next]);
        });
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
