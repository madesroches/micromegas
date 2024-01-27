use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use lgn_tracing::{
    error,
    event::NullEventSink,
    guards::{TracingSystemGuard, TracingThreadGuard},
    imetric, span_scope,
};

pub fn criterion_benchmark(c: &mut Criterion) {
    let _tracing_guard = TracingSystemGuard::new(
        10 * 1024 * 1024,
        1024 * 1024,
        10 * 1024 * 1024,
        Arc::new(NullEventSink {}),
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
    c.bench_function("dispatch/span_scope", |b| {
        b.iter(|| {
            span_scope!("test");
        });
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
