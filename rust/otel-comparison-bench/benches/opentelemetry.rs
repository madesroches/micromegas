//! OpenTelemetry side of the instrumentation overhead comparison, on SDK defaults with the
//! network exporters replaced by ones that still encode OTLP protobuf. The micromegas side is
//! `benches/micromegas.rs`; both use the same group and case ids.

use std::{
    collections::HashMap,
    io::Write,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use flate2::{Compression, write::GzEncoder};
use micromegas_otel_comparison_bench::{
    BATCH, Case, bench_cases, bench_contention, print_bytes, print_metric_drops,
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
        PeriodicReader, SdkMeterProvider, Temporality,
        data::{AggregatedMetrics, MetricData, ResourceMetrics},
        exporter::PushMetricExporter,
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

/// Data points received per instrument name, across every export.
static EXPORTED_POINTS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

fn count_points<T>(data: &MetricData<T>) -> u64 {
    match data {
        MetricData::Gauge(g) => g.data_points().count() as u64,
        MetricData::Sum(s) => s.data_points().count() as u64,
        MetricData::Histogram(h) => h.data_points().count() as u64,
        MetricData::ExponentialHistogram(h) => h.data_points().count() as u64,
    }
}

/// Metrics are aggregated in-process; the export interval is long enough that the exporter is
/// never on the measured path, so it only counts the data points it receives.
#[derive(Debug)]
struct CountingMetricExporter;

impl PushMetricExporter for CountingMetricExporter {
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        let mut exported = EXPORTED_POINTS.lock().unwrap();
        let exported = exported.get_or_insert_with(HashMap::new);
        for metric in metrics.scope_metrics().flat_map(|s| s.metrics()) {
            let points = match metric.data() {
                AggregatedMetrics::F64(d) => count_points(d),
                AggregatedMetrics::U64(d) => count_points(d),
                AggregatedMetrics::I64(d) => count_points(d),
            };
            *exported.entry(metric.name().to_owned()).or_default() += points;
        }
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
    meter_provider: SdkMeterProvider,
    resource: ResourceAttributesWithSchema,
}

static PROVIDERS: OnceLock<Providers> = OnceLock::new();
static COUNTER: OnceLock<Counter<u64>> = OnceLock::new();
// One instrument per metric case, so the points the exporter sees under an instrument's name
// belong to that case alone.
static CASE_COUNTERS: [OnceLock<Counter<u64>>; 5] = [const { OnceLock::new() }; 5];
static CASE_GAUGES: [OnceLock<Gauge<u64>>; 5] = [const { OnceLock::new() }; 5];
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
    ($instruments:ident, $record:ident) => {
        [
            ("plain", |_| $instruments[0].get().unwrap().$record(1, &[])),
            ("static_attrs", |_| {
                $instruments[1]
                    .get()
                    .unwrap()
                    .$record(1, &[KeyValue::new("tier", "ram")])
            }),
            ("set", |_| {
                $instruments[2].get().unwrap().$record(1, shared_set())
            }),
            ("dyn_low_card", |i| {
                $instruments[3]
                    .get()
                    .unwrap()
                    .$record(1, &[KeyValue::new("l", LABELS[i % 4])])
            }),
            // Exceeds the default 2000-series cardinality limit, so most samples land in the
            // overflow series.
            ("dyn_high_card", |i| {
                $instruments[4]
                    .get()
                    .unwrap()
                    .$record(1, &[KeyValue::new("id", i.to_string())])
            }),
        ]
    };
}

const COUNTER_CASES: [(&str, Case); 5] = metric_cases!(CASE_COUNTERS, add);
const GAUGE_CASES: [(&str, Case); 5] = metric_cases!(CASE_GAUGES, record);

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

const COUNTER_CONTENTION_CASES: [(&str, Case); 1] = [("metric/static_attrs", |_| {
    counter().add(1, &[KeyValue::new("tier", "ram")])
})];

static LAST_LOG_EXPORTED: AtomicU64 = AtomicU64::new(0);
static LAST_SPAN_EXPORTED: AtomicU64 = AtomicU64::new(0);

fn drain() {
    providers().logger.force_flush().unwrap();
    providers().tracer_provider.force_flush().unwrap();
}

/// Fails the bench if the exporter saw fewer records than the case emitted.
fn after_case(id: &str, emitted: u64) {
    let mut parts = id.split('/');
    let mut kind = parts.next().unwrap();
    if kind == "metric" {
        report_metric_drops(id, emitted);
        return;
    }
    if kind == "contention" {
        kind = parts.next().unwrap();
    }
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

/// Prints how many of the case's samples reached the exporter as individual points; `report.py`
/// parses the line. Unlike logs and spans, metric samples are aggregated away by design, so this
/// reports instead of asserting.
fn report_metric_drops(id: &str, emitted: u64) {
    providers().meter_provider.force_flush().unwrap();
    let mut parts = id.split('/').skip(1);
    let case = parts.next().unwrap();
    let kind = parts.next().unwrap().strip_prefix("otel_").unwrap();
    let kept = EXPORTED_POINTS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&format!("{kind}_{case}")).copied())
        .unwrap_or(0);
    print_metric_drops(id, kept, emitted);
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
        .with_reader(PeriodicReader::builder(CountingMetricExporter).build())
        .build();
    let tracer = tracer_provider.tracer("bench");
    let meter = meter_provider.meter("bench");
    COUNTER.set(meter.u64_counter("counter").build()).unwrap();
    // Created up front so instrument creation is never inside a timed region.
    for (i, (name, _)) in COUNTER_CASES.iter().enumerate() {
        CASE_COUNTERS[i]
            .set(meter.u64_counter(format!("counter_{name}")).build())
            .unwrap();
        CASE_GAUGES[i]
            .set(meter.u64_gauge(format!("gauge_{name}")).build())
            .unwrap();
    }

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
                meter_provider,
                resource: (&resource).into(),
            })
            .is_ok()
    );

    bench_cases(c, "log", "otel", &LOG_CASES, drain, Some(after_case));
    bench_cases(
        c,
        "metric",
        "otel_counter",
        &COUNTER_CASES,
        drain,
        Some(after_case),
    );
    bench_cases(
        c,
        "metric",
        "otel_gauge",
        &GAUGE_CASES,
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
    bench_contention(
        c,
        "otel",
        &CONTENTION_CASES,
        || {},
        || {},
        drain,
        Some(after_case),
    );
    bench_contention(
        c,
        "otel_counter",
        &COUNTER_CONTENTION_CASES,
        || {},
        || {},
        drain,
        Some(after_case),
    );
    bench_encode_group(c);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
