//! OpenTelemetry side of the instrumentation overhead comparison, on SDK defaults with the
//! network exporters replaced by ones that still encode OTLP protobuf. The micromegas side is
//! `benches/micromegas.rs`; both use the same group and case ids.

use std::{
    io::Write,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use flate2::{Compression, write::GzEncoder};
use micromegas_otel_comparison_bench::{
    Case, bench_cases, bench_contention, print_bytes, print_drops,
};
use opentelemetry::{
    InstrumentationScope, KeyValue,
    metrics::{Counter, Gauge, MeterProvider},
    trace::{Tracer, TracerProvider},
};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_proto::{
    tonic::collector::{logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest},
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
        PeriodicReader, SdkMeterProvider, Temporality, data::ResourceMetrics,
        exporter::PushMetricExporter,
    },
    trace::{SdkTracer, SdkTracerProvider, SpanData, SpanExporter},
};
use prost::Message;
use tracing_subscriber::{filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt};

const ENCODE_LOG_EVENTS: usize = 10_000;
const ENCODE_SPANS: usize = 10_000;
// The default batch queue holds 2048 records and drops on full.
const FLUSH_EVERY: usize = 2000;

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

/// Metrics are aggregated in-process; the export interval is long enough that the exporter is
/// never on the measured path, so it does nothing.
#[derive(Debug)]
struct NoopMetricExporter;

impl PushMetricExporter for NoopMetricExporter {
    async fn export(&self, _metrics: &ResourceMetrics) -> OTelSdkResult {
        Ok(())
    }
    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
    fn shutdown_with_timeout(&self, _timeout: std::time::Duration) -> OTelSdkResult {
        Ok(())
    }
    fn temporality(&self) -> Temporality {
        Temporality::Cumulative
    }
}

struct Providers {
    logger: SdkLoggerProvider,
    tracer_provider: SdkTracerProvider,
    tracer: SdkTracer,
    // Kept alive for the reader thread.
    _meter_provider: SdkMeterProvider,
    resource: ResourceAttributesWithSchema,
}

static PROVIDERS: OnceLock<Providers> = OnceLock::new();
static COUNTER: OnceLock<Counter<u64>> = OnceLock::new();
static GAUGE: OnceLock<Gauge<u64>> = OnceLock::new();
static SET: OnceLock<[KeyValue; 3]> = OnceLock::new();

fn providers() -> &'static Providers {
    PROVIDERS
        .get()
        .expect("providers are initialized before any case runs")
}

fn tracer() -> &'static SdkTracer {
    &providers().tracer
}

fn counter() -> &'static Counter<u64> {
    COUNTER.get().unwrap()
}

fn gauge() -> &'static Gauge<u64> {
    GAUGE.get().unwrap()
}

fn shared_set() -> &'static [KeyValue] {
    SET.get_or_init(|| {
        [
            KeyValue::new("x", "1"),
            KeyValue::new("y", "2"),
            KeyValue::new("z", "3"),
        ]
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

macro_rules! metric_cases {
    ($instrument:ident, $record:ident) => {
        [
            ("plain", |_| $instrument().$record(1, &[])),
            ("static_attrs", |_| {
                $instrument().$record(1, &[KeyValue::new("tier", "ram")])
            }),
            ("set", |_| $instrument().$record(1, shared_set())),
            ("dyn_low_card", |i| {
                $instrument().$record(1, &[KeyValue::new("l", LABELS[i % 4])])
            }),
            // Exceeds the default 2000-series cardinality limit, so most samples land in the
            // overflow series.
            ("dyn_high_card", |i| {
                $instrument().$record(1, &[KeyValue::new("id", i.to_string())])
            }),
        ]
    };
}

const COUNTER_CASES: [(&str, Case); 5] = metric_cases!(counter, add);
const GAUGE_CASES: [(&str, Case); 5] = metric_cases!(gauge, record);

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

const CONTENTION_CASES: [(&str, Case); 3] = [
    ("log/static", LOG_CASES[0].1),
    ("log/dyn_high_card", LOG_CASES[4].1),
    ("span/static", SPAN_CASES[0].1),
];

const COUNTER_CONTENTION_CASES: [(&str, Case); 1] = [("metric/static_attrs", COUNTER_CASES[1].1)];

static LAST_LOG_EXPORTED: AtomicU64 = AtomicU64::new(0);
static LAST_SPAN_EXPORTED: AtomicU64 = AtomicU64::new(0);

/// Flushes the batch processors so the exporter has seen everything the case emitted, then
/// reports how much of it never got there.
fn after_case(id: &str, emitted: u64) {
    let mut parts = id.split('/');
    let mut kind = parts.next().unwrap();
    if kind == "contention" {
        kind = parts.next().unwrap();
    }
    let (exported, last) = match kind {
        "log" => {
            providers().logger.force_flush().unwrap();
            (&LOG_EXPORTED, &LAST_LOG_EXPORTED)
        }
        "span" => {
            providers().tracer_provider.force_flush().unwrap();
            (&SPAN_EXPORTED, &LAST_SPAN_EXPORTED)
        }
        _ => return,
    };
    let total = exported.load(Ordering::Relaxed);
    let delivered = total - last.swap(total, Ordering::Relaxed);
    print_drops(id, emitted.saturating_sub(delivered), emitted);
}

fn capture_logs(case: Case, nb_events: usize) -> Vec<(SdkLogRecord, InstrumentationScope)> {
    providers().logger.force_flush().unwrap();
    CAPTURED_LOGS.lock().unwrap().clear();
    CAPTURE_LOGS.store(true, Ordering::Relaxed);
    for i in 0..nb_events {
        case(i);
        if (i + 1) % FLUSH_EVERY == 0 {
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
        if (i + 1) % FLUSH_EVERY == 0 {
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
    for (name, case) in [SPAN_CASES[0], SPAN_CASES[2]] {
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
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_reader(PeriodicReader::builder(NoopMetricExporter).build())
        .build();
    let tracer = tracer_provider.tracer("bench");
    let meter = meter_provider.meter("bench");
    COUNTER.set(meter.u64_counter("counter").build()).unwrap();
    GAUGE.set(meter.u64_gauge("gauge").build()).unwrap();

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
                _meter_provider: meter_provider,
                resource: (&resource).into(),
            })
            .is_ok()
    );

    bench_cases(c, "log", "otel", &LOG_CASES, Some(after_case));
    bench_cases(c, "metric", "otel_counter", &COUNTER_CASES, None);
    bench_cases(c, "metric", "otel_gauge", &GAUGE_CASES, None);
    bench_cases(c, "span", "otel", &SPAN_CASES, Some(after_case));
    bench_cases(
        c,
        "span",
        "otel_tracing",
        &TRACING_SPAN_CASES,
        Some(after_case),
    );
    bench_contention(c, "otel", &CONTENTION_CASES, || {}, || {}, Some(after_case));
    bench_contention(
        c,
        "otel_counter",
        &COUNTER_CONTENTION_CASES,
        || {},
        || {},
        Some(after_case),
    );
    bench_encode_group(c);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
