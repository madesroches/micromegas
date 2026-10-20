//! OpenTelemetry side of the instrumentation overhead comparison, on SDK defaults with the
//! network exporters replaced by ones that still encode OTLP protobuf. The micromegas side is
//! `benches/micromegas.rs`; both use the same group and case ids.

use std::{
    io::Write,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use flate2::{Compression, write::GzEncoder};
use micromegas_otel_comparison_bench::{BATCH, Case, SERIES_COUNTS, bench_cases, print_bytes};
use opentelemetry::{
    InstrumentationScope, KeyValue,
    metrics::{Gauge, MeterProvider},
    trace::{Tracer, TracerProvider},
};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_proto::{
    tonic::collector::{
        logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
        trace::v1::ExportTraceServiceRequest,
    },
    transform::{
        common::tonic::ResourceAttributesWithSchema, logs::tonic::group_logs_by_resource_and_scope,
        trace::tonic::group_spans_by_resource_and_scope,
    },
};
use opentelemetry_sdk::{
    Resource,
    error::OTelSdkResult,
    logs::{LogBatch, LogExporter, SdkLogRecord, SdkLoggerProvider},
    metrics::{
        InstrumentKind, ManualReader, Pipeline, SdkMeterProvider, Temporality,
        data::{AggregatedMetrics, MetricData, ResourceMetrics},
        reader::MetricReader,
    },
    trace::{SdkTracer, SdkTracerProvider, SpanData, SpanExporter},
};
use prost::Message;
use tracing_subscriber::{filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt};

const ENCODE_LOG_EVENTS: usize = 10_000;
const ENCODE_SPANS: usize = 10_000;

static LABELS: [&str; 4] = ["alpha", "beta", "gamma", "delta"];

static LOG_EXPORTED: AtomicU64 = AtomicU64::new(0);
static SPAN_EXPORTED: AtomicU64 = AtomicU64::new(0);
static CAPTURE_LOGS: AtomicBool = AtomicBool::new(false);
static CAPTURE_SPANS: AtomicBool = AtomicBool::new(false);
static CAPTURED_LOGS: Mutex<Vec<(SdkLogRecord, InstrumentationScope)>> = Mutex::new(Vec::new());
static CAPTURED_SPANS: Mutex<Vec<SpanData>> = Mutex::new(Vec::new());

fn encode_logs(batch: &LogBatch<'_>, resource: &ResourceAttributesWithSchema) -> Vec<u8> {
    ExportLogsServiceRequest {
        resource_logs: group_logs_by_resource_and_scope(batch, resource),
    }
    .encode_to_vec()
}

fn encode_spans(spans: Vec<SpanData>, resource: &ResourceAttributesWithSchema) -> Vec<u8> {
    ExportTraceServiceRequest {
        resource_spans: group_spans_by_resource_and_scope(spans, resource),
    }
    .encode_to_vec()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// Does the conversion work of an OTLP exporter on the batch thread, without the network.
#[derive(Debug, Default)]
struct EncodingLogExporter {
    resource: ResourceAttributesWithSchema,
}

impl LogExporter for EncodingLogExporter {
    async fn export(&self, batch: LogBatch<'_>) -> OTelSdkResult {
        let nb_records = batch.iter().count() as u64;
        if CAPTURE_LOGS.load(Ordering::Relaxed) {
            CAPTURED_LOGS
                .lock()
                .unwrap()
                .extend(batch.iter().map(|(r, s)| (r.clone(), s.clone())));
        }
        std::hint::black_box(encode_logs(&batch, &self.resource));
        LOG_EXPORTED.fetch_add(nb_records, Ordering::Relaxed);
        Ok(())
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.resource = resource.into();
    }
}

#[derive(Debug, Default)]
struct EncodingSpanExporter {
    resource: ResourceAttributesWithSchema,
}

impl SpanExporter for EncodingSpanExporter {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let nb_spans = batch.len() as u64;
        if CAPTURE_SPANS.load(Ordering::Relaxed) {
            CAPTURED_SPANS.lock().unwrap().extend(batch.iter().cloned());
        }
        std::hint::black_box(encode_spans(batch, &self.resource));
        SPAN_EXPORTED.fetch_add(nb_spans, Ordering::Relaxed);
        Ok(())
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.resource = resource.into();
    }
}

/// The provider owns its reader, so the bench keeps a second handle to collect on demand.
#[derive(Debug, Clone)]
struct SharedReader(Arc<ManualReader>);

impl MetricReader for SharedReader {
    fn register_pipeline(&self, pipeline: std::sync::Weak<Pipeline>) {
        self.0.register_pipeline(pipeline)
    }
    fn collect(&self, rm: &mut ResourceMetrics) -> OTelSdkResult {
        self.0.collect(rm)
    }
    fn force_flush(&self) -> OTelSdkResult {
        self.0.force_flush()
    }
    fn shutdown_with_timeout(&self, timeout: std::time::Duration) -> OTelSdkResult {
        self.0.shutdown_with_timeout(timeout)
    }
    fn temporality(&self, kind: InstrumentKind) -> Temporality {
        self.0.temporality(kind)
    }
}

/// One meter provider per batch size, so a cumulative collect holds exactly that many series.
struct BatchMeter {
    n: usize,
    _provider: SdkMeterProvider,
    reader: SharedReader,
    gauge: Gauge<u64>,
}

fn batch_meter(n: usize) -> &'static BatchMeter {
    providers().batch_meters.iter().find(|m| m.n == n).unwrap()
}

struct Providers {
    logger: SdkLoggerProvider,
    tracer_provider: SdkTracerProvider,
    tracer: SdkTracer,
    batch_meters: Vec<BatchMeter>,
    resource: ResourceAttributesWithSchema,
}

static PROVIDERS: OnceLock<Providers> = OnceLock::new();
static SERIES: OnceLock<Vec<&'static [KeyValue]>> = OnceLock::new();

fn providers() -> &'static Providers {
    PROVIDERS
        .get()
        .expect("providers are initialized before any case runs")
}

fn tracer() -> &'static SdkTracer {
    &providers().tracer
}

/// One static attribute slice per series: three fixed pairs and the series identifier.
fn series_attrs() -> &'static [&'static [KeyValue]] {
    SERIES.get_or_init(|| {
        (0..*SERIES_COUNTS.iter().max().unwrap())
            .map(|k| {
                Box::leak(Box::new([
                    KeyValue::new("x", "1"),
                    KeyValue::new("y", "2"),
                    KeyValue::new("z", "3"),
                    KeyValue::new("series", k.to_string()),
                ])) as &'static [KeyValue]
            })
            .collect()
    })
}

const LOG_CASES: [(&str, Case); 5] = [
    ("static", |_| tracing::info!("static")),
    ("formatted", |i| tracing::info!("value {}", i)),
    ("static_attrs", |_| {
        tracing::info!(a = "1", b = "2", c = "3", "static")
    }),
    ("dyn_low_card", |i| {
        tracing::info!(l = LABELS[i % 4], "static")
    }),
    ("dyn_high_card", |i| tracing::info!(id = %i, "static")),
];

/// One batch: a value for each of `N` series, from the persistent counter.
fn record_batch<const N: usize>(i: usize) {
    let gauge = &batch_meter(N).gauge;
    for attrs in &series_attrs()[..N] {
        gauge.record(i as u64, attrs);
    }
}

fn metric_batch_cases() -> [(&'static str, Case); 2] {
    [
        ("record/10", record_batch::<10>),
        ("record/100", record_batch::<100>),
    ]
}

const SPAN_CASES: [(&str, Case); 3] = [
    ("static", |_| {
        let _span = tracer().start("static");
    }),
    ("attrs", |i| {
        let _span = tracer()
            .span_builder("static")
            .with_attributes([KeyValue::new("id", i.to_string())])
            .start(tracer());
    }),
    ("dyn_name", |i| {
        let _span = tracer().start(format!("span {i}"));
    }),
];

const TRACING_SPAN_CASES: [(&str, Case); 3] = [
    ("static", |_| {
        let _entered = tracing::info_span!("static").entered();
    }),
    ("attrs", |i| {
        let _entered = tracing::info_span!("static", id = %i).entered();
    }),
    ("dyn_name", |i| {
        let _entered =
            tracing::info_span!("static", otel.name = %format_args!("span {i}")).entered();
    }),
];

static LAST_LOG_EXPORTED: AtomicU64 = AtomicU64::new(0);
static LAST_SPAN_EXPORTED: AtomicU64 = AtomicU64::new(0);

fn drain() {
    providers().logger.force_flush().unwrap();
    providers().tracer_provider.force_flush().unwrap();
}

/// Fails the bench if the exporter saw fewer records than the case emitted.
fn after_case(id: &str, emitted: u64) {
    let kind = id.split('/').next().unwrap();
    let (exported, last) = match kind {
        "log" => (&LOG_EXPORTED, &LAST_LOG_EXPORTED),
        "span" => (&SPAN_EXPORTED, &LAST_SPAN_EXPORTED),
        _ => return,
    };
    drain();
    let total = exported.load(Ordering::Relaxed);
    let delivered = total - last.swap(total, Ordering::Relaxed);
    assert_eq!(
        delivered,
        emitted,
        "{id}: dropped {} of {emitted} records",
        emitted.saturating_sub(delivered)
    );
}

fn capture_logs(case: Case, nb_events: usize) -> Vec<(SdkLogRecord, InstrumentationScope)> {
    providers().logger.force_flush().unwrap();
    CAPTURED_LOGS.lock().unwrap().clear();
    CAPTURE_LOGS.store(true, Ordering::Relaxed);
    for i in 0..nb_events {
        case(i);
        if (i + 1) % BATCH == 0 {
            providers().logger.force_flush().unwrap();
        }
    }
    providers().logger.force_flush().unwrap();
    CAPTURE_LOGS.store(false, Ordering::Relaxed);
    let captured = std::mem::take(&mut *CAPTURED_LOGS.lock().unwrap());
    assert_eq!(
        captured.len(),
        nb_events,
        "records were dropped during capture"
    );
    captured
}

fn capture_spans(case: Case, nb_spans: usize) -> Vec<SpanData> {
    providers().tracer_provider.force_flush().unwrap();
    CAPTURED_SPANS.lock().unwrap().clear();
    CAPTURE_SPANS.store(true, Ordering::Relaxed);
    for i in 0..nb_spans {
        case(i);
        if (i + 1) % BATCH == 0 {
            providers().tracer_provider.force_flush().unwrap();
        }
    }
    providers().tracer_provider.force_flush().unwrap();
    CAPTURE_SPANS.store(false, Ordering::Relaxed);
    let captured = std::mem::take(&mut *CAPTURED_SPANS.lock().unwrap());
    assert_eq!(
        captured.len(),
        nb_spans,
        "spans were dropped during capture"
    );
    captured
}

fn bench_encode_group(c: &mut Criterion) {
    let resource = &providers().resource;
    let mut group = c.benchmark_group("encode");
    for (name, case) in [LOG_CASES[2], LOG_CASES[4]] {
        let name = format!("log_{name}");
        let records = capture_logs(case, ENCODE_LOG_EVENTS);
        let refs: Vec<_> = records.iter().map(|(r, s)| (r, s)).collect();
        let batch = LogBatch::new(&refs);
        let proto = encode_logs(&batch, resource);
        print_bytes(
            &format!("encode/{name}/otel"),
            ENCODE_LOG_EVENTS,
            proto.len(),
            gzip(&proto).len(),
        );
        group.throughput(Throughput::Elements(ENCODE_LOG_EVENTS as u64));
        group.bench_function(BenchmarkId::new(&name, "otel"), |b| {
            b.iter(|| encode_logs(&batch, resource))
        });
        group.bench_function(BenchmarkId::new(&name, "otel_gzip"), |b| {
            b.iter(|| gzip(&encode_logs(&batch, resource)))
        });
    }
    for (name, case) in SPAN_CASES {
        let name = format!("span_{name}");
        let spans = capture_spans(case, ENCODE_SPANS);
        let proto = encode_spans(spans.clone(), resource);
        print_bytes(
            &format!("encode/{name}/otel"),
            ENCODE_SPANS,
            proto.len(),
            gzip(&proto).len(),
        );
        group.throughput(Throughput::Elements(ENCODE_SPANS as u64));
        group.bench_function(BenchmarkId::new(&name, "otel"), |b| {
            b.iter_batched(
                || spans.clone(),
                |spans| encode_spans(spans, resource),
                BatchSize::LargeInput,
            )
        });
        group.bench_function(BenchmarkId::new(&name, "otel_gzip"), |b| {
            b.iter_batched(
                || spans.clone(),
                |spans| gzip(&encode_spans(spans, resource)),
                BatchSize::LargeInput,
            )
        });
    }
    group.finish();
}

fn encode_metrics(rm: &ResourceMetrics) -> Vec<u8> {
    ExportMetricsServiceRequest::from(rm).encode_to_vec()
}

/// One batch is `n` gauge records followed by one collect, converted to OTLP and encoded: what a
/// periodic exporter does once per interval, with the interval being one batch. The records are
/// untimed setup; the collect reuses one `ResourceMetrics`, as the periodic reader does.
fn bench_metric_batch_export(c: &mut Criterion) {
    let mut group = c.benchmark_group("metric_batch");
    let mut rm = ResourceMetrics::default();
    for n in SERIES_COUNTS {
        let meter = batch_meter(n);
        let record = |i: usize| {
            for attrs in &series_attrs()[..n] {
                meter.gauge.record(i as u64, attrs);
            }
        };
        record(0);
        meter.reader.collect(&mut rm).unwrap();
        let proto = encode_metrics(&rm);
        let points: usize = rm
            .scope_metrics()
            .flat_map(|s| s.metrics())
            .map(|m| match m.data() {
                AggregatedMetrics::U64(MetricData::Gauge(g)) => g.data_points().count(),
                _ => 0,
            })
            .sum();
        assert_eq!(points, n, "a batch must export one point per series");
        print_bytes(
            &format!("metric_batch/{n}/otel"),
            1,
            proto.len(),
            gzip(&proto).len(),
        );
        let mut i = 1usize;
        group.bench_function(BenchmarkId::new(format!("export/{n}"), "otel"), |b| {
            b.iter_batched(
                || {
                    i += 1;
                    record(i)
                },
                |()| {
                    meter.reader.collect(&mut rm).unwrap();
                    encode_metrics(&rm)
                },
                BatchSize::SmallInput,
            )
        });
        group.bench_function(BenchmarkId::new(format!("export/{n}"), "otel_gzip"), |b| {
            b.iter_batched(
                || {
                    i += 1;
                    record(i)
                },
                |()| {
                    meter.reader.collect(&mut rm).unwrap();
                    gzip(&encode_metrics(&rm))
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

fn criterion_benchmark(c: &mut Criterion) {
    let resource = Resource::builder().with_service_name("bench").build();
    let logger = SdkLoggerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(EncodingLogExporter::default())
        .build();
    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(EncodingSpanExporter::default())
        .build();
    let tracer = tracer_provider.tracer("bench");
    let batch_meters = SERIES_COUNTS
        .iter()
        .map(|&n| {
            let reader = SharedReader(Arc::new(ManualReader::builder().build()));
            let provider = SdkMeterProvider::builder()
                .with_resource(resource.clone())
                .with_reader(reader.clone())
                .build();
            let gauge = provider.meter("bench").u64_gauge("gauge").build();
            BatchMeter {
                n,
                _provider: provider,
                reader,
                gauge,
            }
        })
        .collect();
    series_attrs();

    tracing_subscriber::registry()
        .with(LevelFilter::INFO)
        .with(OpenTelemetryTracingBridge::new(&logger))
        .with(tracing_opentelemetry::layer().with_tracer(tracer.clone()))
        .init();
    assert!(
        PROVIDERS
            .set(Providers {
                logger,
                tracer_provider,
                tracer,
                batch_meters,
                resource: (&resource).into(),
            })
            .is_ok()
    );

    bench_cases(c, "log", "otel", &LOG_CASES, drain, Some(after_case));
    bench_cases(
        c,
        "metric_batch",
        "otel",
        &metric_batch_cases(),
        drain,
        Some(after_case),
    );
    bench_cases(c, "span", "otel", &SPAN_CASES, drain, Some(after_case));
    bench_cases(
        c,
        "span",
        "otel_tracing",
        &TRACING_SPAN_CASES,
        drain,
        Some(after_case),
    );
    bench_encode_group(c);
    bench_metric_batch_export(c);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
