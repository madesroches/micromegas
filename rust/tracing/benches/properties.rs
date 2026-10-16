use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Barrier, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Instant,
};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use micromegas_tracing::{
    dispatch::{flush_log_buffer, flush_metrics_buffer},
    dyn_deps::DynDeps,
    event::{EventSink, ExtractDeps},
    guards::TracingSystemGuard,
    imetric, info,
    levels::{LevelFilter, set_max_level},
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    property_set::{Property, PropertySet},
    spans::{ThreadBlock, ThreadStream},
};
use micromegas_transit::HeterogeneousQueue;

static LABELS: [&str; 4] = ["alpha", "beta", "gamma", "delta"];
static SET: OnceLock<&'static PropertySet> = OnceLock::new();

fn shared_set() -> &'static PropertySet {
    SET.get_or_init(|| {
        PropertySet::find_or_create(vec![
            Property::new("x", "1"),
            Property::new("y", "2"),
            Property::new("z", "3"),
        ])
    })
}

/// Accepts every log and discards every block, unless `capture` is set.
struct BenchSink {
    capture: AtomicBool,
    log_blocks: Mutex<Vec<Arc<LogBlock>>>,
    metrics_blocks: Mutex<Vec<Arc<MetricsBlock>>>,
    // Blocks are freed on another thread, like the HTTP sink does: freeing a full block on the
    // recording thread charges the benchmark for work production never does there.
    discard: mpsc::Sender<Box<dyn Send>>,
}

impl EventSink for BenchSink {
    fn on_startup(&self, _: Arc<ProcessInfo>) {}
    fn on_shutdown(&self) {}

    fn on_log_enabled(&self, _: &LogMetadata) -> bool {
        true
    }
    fn on_log(&self, _: &LogMetadata, _: &[DynProperty<'_>], _: i64, _: fmt::Arguments<'_>) {}
    fn on_init_log_stream(&self, _: &LogStream) {}
    fn on_process_log_block(&self, block: Arc<LogBlock>) {
        if self.capture.load(Ordering::Relaxed) {
            self.log_blocks.lock().unwrap().push(block);
        } else {
            let _ = self.discard.send(Box::new(block));
        }
    }

    fn on_init_metrics_stream(&self, _: &MetricsStream) {}
    fn on_process_metrics_block(&self, block: Arc<MetricsBlock>) {
        if self.capture.load(Ordering::Relaxed) {
            self.metrics_blocks.lock().unwrap().push(block);
        } else {
            let _ = self.discard.send(Box::new(block));
        }
    }

    fn on_init_thread_stream(&self, _: &ThreadStream) {}
    fn on_process_thread_block(&self, _: Arc<ThreadBlock>) {}

    fn is_busy(&self) -> bool {
        false
    }
}

type Case = fn(usize);

const LOG_CASES: [(&str, Case); 7] = [
    ("static", |_| info!("static")),
    ("formatted", |i| info!("value {}", i)),
    ("literals", |_| info!(a = "1", b = "2", c = "3", "static")),
    ("set", |_| info!(..shared_set(), "static")),
    ("dyn_low_card", |i| info!(l = LABELS[i % 4], "static")),
    ("dyn_high_card", |i| info!(id = %i, "static")),
    (
        "dyn_mixed",
        |i| info!(a = "1", b = "2", c = "3", id = %i, "static"),
    ),
];

const METRIC_CASES: [(&str, Case); 6] = [
    ("plain", |_| imetric!("name", "unit", 1)),
    ("literals", |_| imetric!("name", "unit", tier = "ram", 1)),
    ("set", |_| imetric!("name", "unit", ..shared_set(), 1)),
    ("literals_set", |_| {
        imetric!("name", "unit", tier = "ram", ..shared_set(), 1)
    }),
    ("dyn_low_card", |i| {
        imetric!("name", "unit", l = LABELS[i % 4], 1)
    }),
    ("dyn_high_card", |i| imetric!("name", "unit", id = %i, 1)),
];

fn log_case(name: &str) -> Case {
    LOG_CASES.iter().find(|(n, _)| *n == name).unwrap().1
}

fn metric_case(name: &str) -> Case {
    METRIC_CASES.iter().find(|(n, _)| *n == name).unwrap().1
}

fn bench_cases(c: &mut Criterion, group_name: &str, cases: &[(&str, Case)]) {
    let mut group = c.benchmark_group(group_name);
    for (name, case) in cases {
        // Persists across warm-up and sample rounds so ids keep being new to the block.
        let mut i = 0usize;
        group.bench_function(*name, |b| {
            b.iter(|| {
                case(i);
                i = i.wrapping_add(1);
            })
        });
    }
    group.finish();
}

fn bench_extract<Q>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    id: &str,
    events: &Q,
    dyn_deps: &DynDeps<Q::DepsQueue>,
) where
    Q: ExtractDeps + HeterogeneousQueue,
{
    let nb_objects = events.nb_objects();
    println!(
        "{id}: object bytes/event = {}, dependency bytes/event = {}",
        events.len_bytes() / nb_objects,
        events.extract(dyn_deps).len_bytes() / nb_objects
    );
    group.throughput(Throughput::Elements(nb_objects as u64));
    group.bench_function(id, |b| b.iter(|| events.extract(dyn_deps)));
}

fn capture_log_block(sink: &BenchSink, case: Case, nb_events: usize) -> Arc<LogBlock> {
    flush_log_buffer();
    sink.log_blocks.lock().unwrap().clear();
    sink.capture.store(true, Ordering::Relaxed);
    for i in 0..nb_events {
        case(i);
    }
    flush_log_buffer();
    sink.capture.store(false, Ordering::Relaxed);
    let mut blocks = std::mem::take(&mut *sink.log_blocks.lock().unwrap());
    assert_eq!(blocks.len(), 1, "events must fit in a single block");
    blocks.pop().unwrap()
}

fn capture_metrics_block(sink: &BenchSink, case: Case, nb_events: usize) -> Arc<MetricsBlock> {
    flush_metrics_buffer();
    sink.metrics_blocks.lock().unwrap().clear();
    sink.capture.store(true, Ordering::Relaxed);
    for i in 0..nb_events {
        case(i);
    }
    flush_metrics_buffer();
    sink.capture.store(false, Ordering::Relaxed);
    let mut blocks = std::mem::take(&mut *sink.metrics_blocks.lock().unwrap());
    assert_eq!(blocks.len(), 1, "events must fit in a single block");
    blocks.pop().unwrap()
}

fn bench_extraction_group(c: &mut Criterion, sink: &BenchSink) {
    let mut group = c.benchmark_group("extract");
    for name in [
        "static",
        "set",
        "dyn_low_card",
        "dyn_high_card",
        "dyn_mixed",
    ] {
        let block = capture_log_block(sink, log_case(name), 10_000);
        bench_extract(
            &mut group,
            &format!("log/{name}"),
            &block.events,
            &block.dyn_deps,
        );
    }
    for name in ["plain", "set", "dyn_low_card", "dyn_high_card"] {
        let block = capture_metrics_block(sink, metric_case(name), 5_000);
        bench_extract(
            &mut group,
            &format!("metric/{name}"),
            &block.events,
            &block.dyn_deps,
        );
    }
    group.finish();
}

fn bench_contention_group(c: &mut Criterion) {
    let cases: [(&str, Case); 5] = [
        ("log/static", log_case("static")),
        ("log/formatted", log_case("formatted")),
        ("log/dyn_high_card", log_case("dyn_high_card")),
        ("metric/plain", metric_case("plain")),
        ("metric/dyn_high_card", metric_case("dyn_high_card")),
    ];
    let mut group = c.benchmark_group("contention");
    for (name, case) in cases {
        for threads in [1u64, 4, 8] {
            group.throughput(Throughput::Elements(threads));
            // Advanced after every round so ids stay unique across threads and rounds.
            let mut base = 0u64;
            group.bench_with_input(BenchmarkId::new(name, threads), &threads, |b, &threads| {
                b.iter_custom(|iters| {
                    let barrier = Barrier::new(threads as usize + 1);
                    let elapsed = std::thread::scope(|s| {
                        for t in 0..threads {
                            let barrier = &barrier;
                            let start = base + t * iters;
                            s.spawn(move || {
                                barrier.wait();
                                for i in start..start + iters {
                                    case(i as usize);
                                }
                            });
                        }
                        barrier.wait();
                        Instant::now()
                    })
                    .elapsed();
                    base += threads * iters;
                    elapsed
                })
            });
        }
    }
    group.finish();
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let (discard, discarded) = mpsc::channel::<Box<dyn Send>>();
    std::thread::spawn(move || for _block in discarded {});
    let sink = Arc::new(BenchSink {
        capture: AtomicBool::new(false),
        log_blocks: Mutex::new(vec![]),
        metrics_blocks: Mutex::new(vec![]),
        discard,
    });
    let _tracing_guard = TracingSystemGuard::new(
        10 * 1024 * 1024,
        1024 * 1024,
        10 * 1024 * 1024,
        sink.clone(),
        HashMap::new(),
        false,
    )
    .expect("init telemetry");
    set_max_level(LevelFilter::Trace);

    bench_cases(c, "log", &LOG_CASES);
    bench_cases(c, "metric", &METRIC_CASES);
    bench_extraction_group(c, &sink);
    bench_contention_group(c);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
