use criterion::{criterion_group, criterion_main, Criterion};
use lgn_tracing::{error, imetric, span_scope};

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("no_dispatch/log", |b| {
        b.iter(|| {
            error!("test");
        });
    });
    c.bench_function("no_dispatch/metric", |b| {
        b.iter(|| {
            imetric!("name", "unit", 0);
        });
    });
    c.bench_function("no_dispatch/span_scope", |b| {
        b.iter(|| {
            span_scope!("test");
        });
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
