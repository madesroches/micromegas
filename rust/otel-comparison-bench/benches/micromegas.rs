//! micromegas-tracing side of the instrumentation overhead comparison. The OpenTelemetry side is
//! `benches/opentelemetry.rs`; both use the same group and case ids.

use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use micromegas_otel_comparison_bench::{
    Case, SERIES_COUNTS, bench_cases, bench_contention, print_bytes,
};
use micromegas_telemetry_sink::{
    stream_block::StreamBlock, tracing_interop::install_tracing_interop,
};
use micromegas_tracing::{
    dispatch::{
        flush_log_buffer, flush_metrics_buffer, flush_thread_buffer, init_thread_stream,
        unregister_thread_stream,
    },
    event::{EventBlock, EventSink, ExtractDeps, TracingBlock},
    guards::TracingSystemGuard,
    imetric, info,
    levels::{LevelFilter, set_max_level},
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    property_set::{Property, PropertySet},
    span_scope, span_scope_named_dyn,
    spans::{ThreadBlock, ThreadStream},
};
use micromegas_transit::HeterogeneousQueue;

// The soft and hard byte caps of `HttpSinkConfig`.
const SOFT_QUEUE_BYTES: usize = 128 * 1024 * 1024;
const HARD_QUEUE_BYTES: usize = 256 * 1024 * 1024;

const ENCODE_LOG_EVENTS: usize = 10_000;
const ENCODE_SPANS: usize = 10_000;
// Metric events in the block whose encoding is timed, so every batch size fills a block with the
// same number of events.
const ENCODE_METRIC_EVENTS: usize = 10_000;

static LABELS: [&str; 4] = ["alpha", "beta", "gamma", "delta"];
static SERIES: OnceLock<Vec<&'static PropertySet>> = OnceLock::new();

/// One static set per series: three fixed pairs and the series identifier.
fn series_sets() -> &'static [&'static PropertySet] {
    SERIES.get_or_init(|| {
        (0..*SERIES_COUNTS.iter().max().unwrap())
            .map(|k| {
                PropertySet::find_or_create(vec![
                    Property::new("x", "1"),
                    Property::new("y", "2"),
                    Property::new("z", "3"),
                    Property::new("series", Box::leak(k.to_string().into_boxed_str())),
                ])
            })
            .collect()
    })
}

struct QueuedBlock {
    block: Arc<dyn StreamBlock>,
    bytes: usize,
}

/// The HTTP sink minus the network: blocks wait in a byte-capped queue and a worker thread runs
/// the production encoding (`encode_bin`) on them.
struct WorkerQueue {
    items: Mutex<VecDeque<QueuedBlock>>,
    wake: Condvar,
    queued_bytes: AtomicUsize,
    // Queued plus being encoded.
    pending: AtomicUsize,
    idle: Condvar,
    idle_lock: Mutex<()>,
    dropped_events: AtomicU64,
}

impl WorkerQueue {
    /// Mirrors `SharedQueue::try_push`: thread blocks are dropped at the soft cap, log and metric
    /// blocks at the hard cap, both checked against the queue's total bytes.
    fn try_push(&self, block: Arc<dyn StreamBlock>, bytes: usize, events: usize, soft: bool) {
        let cap = if soft {
            SOFT_QUEUE_BYTES
        } else {
            HARD_QUEUE_BYTES
        };
        if self.queued_bytes.load(Ordering::Relaxed) >= cap {
            self.dropped_events
                .fetch_add(events as u64, Ordering::Relaxed);
            return;
        }
        self.queued_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.pending.fetch_add(1, Ordering::Relaxed);
        self.items
            .lock()
            .unwrap()
            .push_back(QueuedBlock { block, bytes });
        self.wake.notify_one();
    }

    fn run_worker(&self, process_info: &OnceLock<Arc<ProcessInfo>>) {
        loop {
            let item = {
                let mut items = self.items.lock().unwrap();
                loop {
                    if let Some(item) = items.pop_front() {
                        break item;
                    }
                    items = self.wake.wait(items).unwrap();
                }
            };
            self.queued_bytes.fetch_sub(item.bytes, Ordering::Relaxed);
            let process_info = process_info.get().expect("blocks arrive after startup");
            let _ = std::hint::black_box(item.block.encode_bin(process_info));
            if self.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                // Taking the lock orders this notification after a waiter's check of `pending`.
                drop(self.idle_lock.lock().unwrap());
                self.idle.notify_all();
            }
        }
    }

    fn wait_idle(&self) {
        let mut guard = self.idle_lock.lock().unwrap();
        while self.pending.load(Ordering::Acquire) > 0 {
            guard = self.idle.wait(guard).unwrap();
        }
    }
}

/// Accepts every log, encodes every block on a worker thread, and keeps blocks instead while
/// `capture` is set.
struct BenchSink {
    capture: AtomicBool,
    log_blocks: Mutex<Vec<Arc<LogBlock>>>,
    metrics_blocks: Mutex<Vec<Arc<MetricsBlock>>>,
    thread_blocks: Mutex<Vec<Arc<ThreadBlock>>>,
    process_info: Arc<OnceLock<Arc<ProcessInfo>>>,
    queue: Arc<WorkerQueue>,
}

impl EventSink for BenchSink {
    fn on_startup(&self, process_info: Arc<ProcessInfo>) {
        let _ = self.process_info.set(process_info);
    }
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
            let (bytes, events) = (block.len_bytes(), block.nb_objects());
            self.queue.try_push(block, bytes, events, false);
        }
    }

    fn on_init_metrics_stream(&self, _: &MetricsStream) {}
    fn on_process_metrics_block(&self, block: Arc<MetricsBlock>) {
        if self.capture.load(Ordering::Relaxed) {
            self.metrics_blocks.lock().unwrap().push(block);
        } else {
            let (bytes, events) = (block.len_bytes(), block.nb_objects());
            self.queue.try_push(block, bytes, events, false);
        }
    }

    fn on_init_thread_stream(&self, _: &ThreadStream) {}
    fn on_process_thread_block(&self, block: Arc<ThreadBlock>) {
        if self.capture.load(Ordering::Relaxed) {
            self.thread_blocks.lock().unwrap().push(block);
        } else {
            // A span is a begin and an end event.
            let (bytes, spans) = (block.len_bytes(), block.nb_objects() / 2);
            self.queue.try_push(block, bytes, spans, true);
        }
    }

    fn is_busy(&self) -> bool {
        false
    }
}

static SINK: OnceLock<Arc<BenchSink>> = OnceLock::new();

fn sink() -> &'static BenchSink {
    SINK.get()
        .expect("sink is initialized before any case runs")
}

/// Blocks that already reached the worker queue are encoded; the calling thread's partially
/// filled block stays where it is, as it would in normal operation.
fn drain() {
    sink().queue.wait_idle();
}

fn after_case(id: &str, emitted: u64) {
    let queue = &sink().queue;
    queue.wait_idle();
    let dropped = queue.dropped_events.swap(0, Ordering::Relaxed);
    assert_eq!(dropped, 0, "{id}: dropped {dropped} of {emitted} events");
}

const LOG_CASES: [(&str, Case); 5] = [
    ("static", |_| info!("static")),
    ("formatted", |i| info!("value {}", i)),
    ("static_attrs", |_| {
        info!(a = "1", b = "2", c = "3", "static")
    }),
    ("dyn_low_card", |i| info!(l = LABELS[i % 4], "static")),
    ("dyn_high_card", |i| info!(id = %i, "static")),
];

const TRACING_LOG_CASES: [(&str, Case); 3] = [
    ("static", |_| tracing::info!("static")),
    ("formatted", |i| tracing::info!("value {}", i)),
    ("dyn_high_card", |i| tracing::info!(id = %i, "static")),
];

/// One batch: a value for each of `N` series, from the persistent counter.
fn record_batch<const N: usize>(i: usize) {
    for set in &series_sets()[..N] {
        imetric!("name", "unit", ..*set, i as u64);
    }
}

fn metric_batch_cases() -> [(&'static str, Case); 2] {
    [
        ("record/10", record_batch::<10>),
        ("record/100", record_batch::<100>),
    ]
}

fn metric_batch_export_cases() -> [(usize, Case); 2] {
    [(10, record_batch::<10>), (100, record_batch::<100>)]
}

const SPAN_CASES: [(&str, Case); 3] = [
    ("static", |_| {
        span_scope!("static");
    }),
    ("attrs", |i| {
        let _span = span_scope!(name = "static", id = %i);
    }),
    ("dyn_name", |i| {
        span_scope_named_dyn!(format!("span {i}"));
    }),
];

const CONTENTION_CASES: [(&str, Case); 3] = [
    ("log/static", LOG_CASES[0].1),
    ("log/dyn_high_card", LOG_CASES[4].1),
    ("span/static", SPAN_CASES[0].1),
];

fn capture_block<B>(
    blocks: &Mutex<Vec<Arc<B>>>,
    flush: fn(),
    case: Case,
    nb_events: usize,
) -> Arc<B> {
    flush();
    blocks.lock().unwrap().clear();
    sink().capture.store(true, Ordering::Relaxed);
    for i in 0..nb_events {
        case(i);
    }
    flush();
    sink().capture.store(false, Ordering::Relaxed);
    let mut blocks = std::mem::take(&mut *blocks.lock().unwrap());
    assert_eq!(blocks.len(), 1, "events must fit in a single block");
    blocks.pop().unwrap()
}

fn raw_bytes<Q>(block: &EventBlock<Q>) -> usize
where
    Q: HeterogeneousQueue + ExtractDeps,
    Q::DepsQueue: HeterogeneousQueue,
{
    block.events.len_bytes() + block.events.extract(&block.dyn_deps).len_bytes()
}

fn bench_encode<Q>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: &str,
    block: &EventBlock<Q>,
    nb_events: usize,
) where
    Q: HeterogeneousQueue + ExtractDeps,
    Q::DepsQueue: HeterogeneousQueue,
    EventBlock<Q>: StreamBlock,
{
    let process_info = sink().process_info.get().unwrap();
    let wire = block.encode_bin(process_info).unwrap().len();
    print_bytes(
        &format!("encode/{name}/micromegas"),
        nb_events,
        raw_bytes(block),
        wire,
    );
    group.throughput(Throughput::Elements(nb_events as u64));
    group.bench_function(BenchmarkId::new(name, "micromegas"), |b| {
        b.iter(|| block.encode_bin(process_info).unwrap())
    });
}

fn bench_encode_group(c: &mut Criterion) {
    let sink = sink();
    let mut group = c.benchmark_group("encode");
    for (name, case) in [LOG_CASES[2], LOG_CASES[4]] {
        let block = capture_block(&sink.log_blocks, flush_log_buffer, case, ENCODE_LOG_EVENTS);
        bench_encode(
            &mut group,
            &format!("log_{name}"),
            &block,
            ENCODE_LOG_EVENTS,
        );
    }
    for (name, case) in SPAN_CASES {
        let block = capture_block(&sink.thread_blocks, flush_thread_buffer, case, ENCODE_SPANS);
        bench_encode(&mut group, &format!("span_{name}"), &block, ENCODE_SPANS);
    }
    group.finish();
}

/// Encoding of one block holding `ENCODE_METRIC_EVENTS / n` batches, reported per batch.
fn bench_metric_batch_export(c: &mut Criterion) {
    let sink = sink();
    let process_info = sink.process_info.get().unwrap();
    let mut group = c.benchmark_group("metric_batch");
    for (n, case) in metric_batch_export_cases() {
        let nb_batches = ENCODE_METRIC_EVENTS / n;
        let block = capture_block(&sink.metrics_blocks, flush_metrics_buffer, case, nb_batches);
        let wire = block.encode_bin(process_info).unwrap().len();
        print_bytes(
            &format!("metric_batch/{n}/micromegas"),
            nb_batches,
            raw_bytes(&block),
            wire,
        );
        group.throughput(Throughput::Elements(nb_batches as u64));
        group.bench_function(BenchmarkId::new(format!("export/{n}"), "micromegas"), |b| {
            b.iter(|| block.encode_bin(process_info).unwrap())
        });
    }
    group.finish();
}

fn criterion_benchmark(c: &mut Criterion) {
    let process_info = Arc::new(OnceLock::new());
    let queue = Arc::new(WorkerQueue {
        items: Mutex::new(VecDeque::new()),
        wake: Condvar::new(),
        queued_bytes: AtomicUsize::new(0),
        pending: AtomicUsize::new(0),
        idle: Condvar::new(),
        idle_lock: Mutex::new(()),
        dropped_events: AtomicU64::new(0),
    });
    {
        let (queue, process_info) = (queue.clone(), process_info.clone());
        std::thread::spawn(move || queue.run_worker(&process_info));
    }
    let sink = Arc::new(BenchSink {
        capture: AtomicBool::new(false),
        log_blocks: Mutex::new(vec![]),
        metrics_blocks: Mutex::new(vec![]),
        thread_blocks: Mutex::new(vec![]),
        process_info,
        queue,
    });
    SINK.set(sink.clone()).ok().expect("single init");
    let _tracing_guard = TracingSystemGuard::new(
        10 * 1024 * 1024,
        1024 * 1024,
        10 * 1024 * 1024,
        sink,
        HashMap::new(),
        true,
    )
    .expect("init telemetry");
    set_max_level(LevelFilter::Info);
    init_thread_stream();
    install_tracing_interop(None);
    series_sets();

    bench_cases(c, "log", "micromegas", &LOG_CASES, drain, Some(after_case));
    bench_cases(
        c,
        "log",
        "micromegas_interop",
        &TRACING_LOG_CASES,
        drain,
        Some(after_case),
    );
    bench_cases(
        c,
        "metric_batch",
        "micromegas",
        &metric_batch_cases(),
        drain,
        Some(after_case),
    );
    bench_cases(
        c,
        "span",
        "micromegas",
        &SPAN_CASES,
        drain,
        Some(after_case),
    );
    bench_contention(
        c,
        "micromegas",
        &CONTENTION_CASES,
        init_thread_stream,
        unregister_thread_stream,
        drain,
        Some(after_case),
    );
    bench_encode_group(c);
    bench_metric_batch_export(c);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
