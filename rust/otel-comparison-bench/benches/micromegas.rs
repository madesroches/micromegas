//! micromegas-tracing side of the instrumentation overhead comparison. The OpenTelemetry side is
//! `benches/opentelemetry.rs`; both use the same group and case ids.

use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use micromegas_otel_comparison_bench::{
    Case, bench_cases, bench_contention, print_bytes, print_drops,
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
const ENCODE_METRIC_EVENTS: usize = 5_000;

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
            self.pending.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn wait_idle(&self) {
        while self.pending.load(Ordering::Relaxed) > 0 {
            std::thread::sleep(Duration::from_millis(1));
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

/// Waits for the worker so a case's drops are counted against that case and the next one starts
/// from an idle queue.
fn after_case(id: &str, emitted: u64) {
    flush_log_buffer();
    flush_metrics_buffer();
    flush_thread_buffer();
    let queue = &sink().queue;
    queue.wait_idle();
    print_drops(id, queue.dropped_events.swap(0, Ordering::Relaxed), emitted);
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

const METRIC_CASES: [(&str, Case); 5] = [
    ("plain", |_| imetric!("name", "unit", 1)),
    ("static_attrs", |_| {
        imetric!("name", "unit", tier = "ram", 1)
    }),
    ("set", |_| imetric!("name", "unit", ..shared_set(), 1)),
    ("dyn_low_card", |i| {
        imetric!("name", "unit", l = LABELS[i % 4], 1)
    }),
    ("dyn_high_card", |i| imetric!("name", "unit", id = %i, 1)),
];

const SPAN_CASES: [(&str, Case); 2] = [
    ("static", |_| {
        span_scope!("static");
    }),
    ("dyn_name", |i| {
        span_scope_named_dyn!(format!("span {i}"));
    }),
];

const CONTENTION_CASES: [(&str, Case); 4] = [
    ("log/static", LOG_CASES[0].1),
    ("log/dyn_high_card", LOG_CASES[4].1),
    ("metric/static_attrs", METRIC_CASES[1].1),
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
    bench: bool,
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
    if bench {
        group.throughput(Throughput::Elements(nb_events as u64));
        group.bench_function(BenchmarkId::new(name, "micromegas"), |b| {
            b.iter(|| block.encode_bin(process_info).unwrap())
        });
    }
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
            true,
        );
    }
    for (name, case) in SPAN_CASES {
        let block = capture_block(&sink.thread_blocks, flush_thread_buffer, case, ENCODE_SPANS);
        bench_encode(
            &mut group,
            &format!("span_{name}"),
            &block,
            ENCODE_SPANS,
            true,
        );
    }
    // Only the size is reported: OpenTelemetry exports aggregated series, not one point per
    // sample, so there is nothing to compare the encoding cost with.
    let block = capture_block(
        &sink.metrics_blocks,
        flush_metrics_buffer,
        METRIC_CASES[1].1,
        ENCODE_METRIC_EVENTS,
    );
    bench_encode(
        &mut group,
        "metric_static_attrs",
        &block,
        ENCODE_METRIC_EVENTS,
        false,
    );
    group.finish();
}

fn criterion_benchmark(c: &mut Criterion) {
    let process_info = Arc::new(OnceLock::new());
    let queue = Arc::new(WorkerQueue {
        items: Mutex::new(VecDeque::new()),
        wake: Condvar::new(),
        queued_bytes: AtomicUsize::new(0),
        pending: AtomicUsize::new(0),
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

    bench_cases(c, "log", "micromegas", &LOG_CASES, Some(after_case));
    bench_cases(
        c,
        "log",
        "micromegas_tracing",
        &TRACING_LOG_CASES,
        Some(after_case),
    );
    bench_cases(c, "metric", "micromegas", &METRIC_CASES, Some(after_case));
    bench_cases(c, "span", "micromegas", &SPAN_CASES, Some(after_case));
    bench_contention(
        c,
        "micromegas",
        &CONTENTION_CASES,
        init_thread_stream,
        unregister_thread_stream,
        Some(after_case),
    );
    bench_encode_group(c);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
