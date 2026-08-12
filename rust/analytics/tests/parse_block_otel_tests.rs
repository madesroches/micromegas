//! Unit tests for the OTLP `BlockObjectDecoder`s used by `parse_block`
//! (`otlp/v1/logs`, `otlp/v1/metrics`, `otlp/v1/traces`). No services, no DB —
//! decoders are driven directly through a `Vec<(String, Vec<u8>)>`-collecting
//! `ObjectVisitor`, matching the existing `parse_block_tests.rs` style.

use micromegas_analytics::lakehouse::block_object_decoder::{BlockObjectDecoder, ObjectVisitor};
use micromegas_analytics::lakehouse::otel::block_decoders::{
    OtelLogsBlockDecoder, OtelMetricsBlockDecoder, OtelTracesBlockDecoder,
};
use micromegas_analytics::metadata::StreamMetadata;
use micromegas_telemetry::block_wire_format::BlockPayload;
use opentelemetry_proto::tonic::common::v1::{
    AnyValue, InstrumentationScope, KeyValue, any_value::Value as Av,
};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    ExponentialHistogram, ExponentialHistogramDataPoint, Gauge, Histogram, HistogramDataPoint,
    Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum, Summary, SummaryDataPoint, metric,
    number_data_point,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, span};
use prost::Message;
use std::sync::Arc;

/// Test double: collects every visited `(type_name, value)` pair, never stopping.
#[derive(Default)]
struct CollectingVisitor {
    rows: Vec<(String, Vec<u8>)>,
}

impl ObjectVisitor for CollectingVisitor {
    fn visit(&mut self, type_name: &str, value: &[u8]) -> anyhow::Result<bool> {
        self.rows.push((type_name.to_string(), value.to_vec()));
        Ok(true)
    }

    fn skip(&mut self) -> anyhow::Result<bool> {
        Ok(true)
    }
}

/// Test double: stops after `limit` visited rows.
struct StoppingVisitor {
    limit: usize,
    rows: Vec<(String, Vec<u8>)>,
}

impl ObjectVisitor for StoppingVisitor {
    fn visit(&mut self, type_name: &str, value: &[u8]) -> anyhow::Result<bool> {
        self.rows.push((type_name.to_string(), value.to_vec()));
        Ok(self.rows.len() < self.limit)
    }

    fn skip(&mut self) -> anyhow::Result<bool> {
        Ok(true)
    }
}

/// OTLP decoders never look at `StreamMetadata` (it's transit-specific), so a
/// dummy value satisfies the `BlockObjectDecoder::decode` signature.
fn dummy_stream_metadata() -> StreamMetadata {
    StreamMetadata {
        process_id: uuid::Uuid::nil(),
        stream_id: uuid::Uuid::nil(),
        dependencies_metadata: vec![],
        objects_metadata: vec![],
        tags: vec![],
        properties: Arc::new(vec![]),
    }
}

fn payload_of(objects: Vec<u8>) -> BlockPayload {
    BlockPayload {
        dependencies: vec![],
        objects,
    }
}

fn s_kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        key_strindex: 0,
        value: Some(AnyValue {
            value: Some(Av::StringValue(value.to_string())),
        }),
    }
}

fn scope(name: &str) -> InstrumentationScope {
    InstrumentationScope {
        name: name.to_string(),
        version: String::new(),
        attributes: vec![],
        dropped_attributes_count: 0,
    }
}

/// Decodes JSONB bytes and returns the field for `key`, panicking with a
/// helpful message if `bytes` doesn't decode to an object containing it.
fn field<'a>(bytes: &'a [u8], key: &str) -> jsonb::Value<'a> {
    match jsonb::from_slice(bytes).expect("decoding row JSONB bytes") {
        jsonb::Value::Object(map) => map
            .get(key)
            .unwrap_or_else(|| panic!("missing key '{key}'"))
            .clone(),
        other => panic!("expected a JSONB object, got {other:?}"),
    }
}

fn as_str(v: &jsonb::Value<'_>) -> String {
    match v {
        jsonb::Value::String(s) => s.to_string(),
        other => panic!("expected a JSONB string, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

fn resource_logs_with_two_scopes(records_per_scope: usize) -> ResourceLogs {
    let mut scope_logs = Vec::new();
    for scope_idx in 0..2 {
        let mut log_records = Vec::new();
        for i in 0..records_per_scope {
            log_records.push(LogRecord {
                time_unix_nano: 1_700_000_000_000_000_000 + i as u64,
                observed_time_unix_nano: 0,
                severity_number: 9,
                severity_text: String::new(),
                body: Some(AnyValue {
                    value: Some(Av::StringValue(format!("message {scope_idx}-{i}"))),
                }),
                attributes: vec![s_kv("event.id", &format!("evt-{scope_idx}-{i}"))],
                dropped_attributes_count: 0,
                flags: 0,
                trace_id: vec![0xAB; 16],
                span_id: vec![0xCD; 8],
                event_name: String::new(),
            });
        }
        scope_logs.push(ScopeLogs {
            scope: Some(scope(&format!("scope-{scope_idx}"))),
            log_records,
            schema_url: String::new(),
        });
    }
    ResourceLogs {
        resource: Some(Resource {
            attributes: vec![s_kv("service.name", "my-service")],
            dropped_attributes_count: 0,
            entity_refs: vec![],
        }),
        scope_logs,
        schema_url: String::new(),
    }
}

#[test]
fn logs_decoder_emits_one_row_per_record_with_full_envelope() {
    let resource_logs = resource_logs_with_two_scopes(3);
    let payload = payload_of(resource_logs.encode_to_vec());
    let stream = dummy_stream_metadata();
    let mut visitor = CollectingVisitor::default();
    OtelLogsBlockDecoder
        .decode(&stream, &payload, &mut visitor)
        .expect("decoding a valid ResourceLogs payload");

    assert_eq!(visitor.rows.len(), 6, "2 scopes x 3 records");
    for (type_name, _) in &visitor.rows {
        assert_eq!(type_name, "otlp.LogRecord");
    }

    let (_, bytes) = &visitor.rows[0];
    assert_eq!(as_str(&field(bytes, "__type")), "otlp.LogRecord");

    let attrs = field(bytes, "__attributes");
    match attrs {
        jsonb::Value::Object(m) => {
            assert_eq!(
                as_str(m.get("event.id").expect("record attribute present")),
                "evt-0-0"
            );
        }
        other => panic!("expected __attributes to be an object, got {other:?}"),
    }

    let resource = field(bytes, "__resource");
    match resource {
        jsonb::Value::Object(m) => {
            assert_eq!(
                as_str(m.get("service.name").expect("resource attribute present")),
                "my-service"
            );
        }
        other => panic!("expected __resource to be an object, got {other:?}"),
    }

    let scope_obj = field(bytes, "__scope");
    match scope_obj {
        jsonb::Value::Object(m) => {
            assert_eq!(
                as_str(m.get("otel.scope.name").expect("scope name present")),
                "scope-0"
            );
        }
        other => panic!("expected __scope to be an object, got {other:?}"),
    }

    // OTLP/JSON encodes 64-bit nanos as a quoted string.
    assert_eq!(as_str(&field(bytes, "timeUnixNano")), "1700000000000000000");

    // trace_id is hex-encoded per OTLP/JSON.
    assert_eq!(as_str(&field(bytes, "traceId")), "ab".repeat(16));
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

fn metric_extras<'a>(bytes: &'a [u8]) -> jsonb::Value<'a> {
    field(bytes, "__metric")
}

fn metric_extra_str(bytes: &[u8], key: &str) -> String {
    match metric_extras(bytes) {
        jsonb::Value::Object(m) => as_str(m.get(key).unwrap_or_else(|| panic!("missing {key}"))),
        other => panic!("expected __metric to be an object, got {other:?}"),
    }
}

fn resource_metrics_with_one_of_each() -> ResourceMetrics {
    let attrs = vec![s_kv("k", "v")];
    let sum_metric = Metric {
        name: "sum_metric".to_string(),
        description: String::new(),
        unit: "1".to_string(),
        metadata: vec![],
        data: Some(metric::Data::Sum(Sum {
            data_points: vec![NumberDataPoint {
                attributes: attrs.clone(),
                start_time_unix_nano: 0,
                time_unix_nano: 1,
                exemplars: vec![],
                flags: 0,
                value: Some(number_data_point::Value::AsDouble(1.0)),
            }],
            aggregation_temporality: 2,
            is_monotonic: true,
        })),
    };
    let gauge_metric = Metric {
        name: "gauge_metric".to_string(),
        description: String::new(),
        unit: "1".to_string(),
        metadata: vec![],
        data: Some(metric::Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                attributes: attrs.clone(),
                start_time_unix_nano: 0,
                time_unix_nano: 1,
                exemplars: vec![],
                flags: 0,
                value: Some(number_data_point::Value::AsInt(2)),
            }],
        })),
    };
    let summary_metric = Metric {
        name: "summary_metric".to_string(),
        description: String::new(),
        unit: "ms".to_string(),
        metadata: vec![],
        data: Some(metric::Data::Summary(Summary {
            data_points: vec![SummaryDataPoint {
                attributes: attrs.clone(),
                start_time_unix_nano: 0,
                time_unix_nano: 1,
                count: 10,
                sum: 100.0,
                quantile_values: vec![],
                flags: 0,
            }],
        })),
    };
    let histogram_metric = Metric {
        name: "histogram_metric".to_string(),
        description: String::new(),
        unit: "ms".to_string(),
        metadata: vec![],
        data: Some(metric::Data::Histogram(Histogram {
            data_points: vec![HistogramDataPoint {
                attributes: attrs.clone(),
                start_time_unix_nano: 0,
                time_unix_nano: 1,
                count: 5,
                sum: Some(50.0),
                bucket_counts: vec![1, 2, 2],
                explicit_bounds: vec![1.0, 2.0],
                exemplars: vec![],
                flags: 0,
                min: Some(0.1),
                max: Some(9.9),
            }],
            aggregation_temporality: 2,
        })),
    };
    let exp_histogram_metric = Metric {
        name: "exp_histogram_metric".to_string(),
        description: String::new(),
        unit: "ms".to_string(),
        metadata: vec![],
        data: Some(metric::Data::ExponentialHistogram(ExponentialHistogram {
            data_points: vec![ExponentialHistogramDataPoint {
                attributes: attrs.clone(),
                start_time_unix_nano: 0,
                time_unix_nano: 1,
                count: 5,
                sum: Some(50.0),
                scale: 1,
                zero_count: 0,
                positive: None,
                negative: None,
                flags: 0,
                exemplars: vec![],
                min: Some(0.1),
                max: Some(9.9),
                zero_threshold: 0.0,
            }],
            aggregation_temporality: 2,
        })),
    };

    ResourceMetrics {
        resource: Some(Resource {
            attributes: vec![s_kv("service.name", "metrics-service")],
            dropped_attributes_count: 0,
            entity_refs: vec![],
        }),
        scope_metrics: vec![ScopeMetrics {
            scope: Some(scope("test-scope")),
            metrics: vec![
                sum_metric,
                gauge_metric,
                summary_metric,
                histogram_metric,
                exp_histogram_metric,
            ],
            schema_url: String::new(),
        }],
        schema_url: String::new(),
    }
}

#[test]
fn metrics_decoder_emits_one_row_per_data_point_with_metric_extras() {
    let resource_metrics = resource_metrics_with_one_of_each();
    let payload = payload_of(resource_metrics.encode_to_vec());
    let stream = dummy_stream_metadata();
    let mut visitor = CollectingVisitor::default();
    OtelMetricsBlockDecoder
        .decode(&stream, &payload, &mut visitor)
        .expect("decoding a valid ResourceMetrics payload");

    assert_eq!(visitor.rows.len(), 5, "one data point per metric");

    let expected_type_names = [
        "otlp.NumberDataPoint",
        "otlp.NumberDataPoint",
        "otlp.SummaryDataPoint",
        "otlp.HistogramDataPoint",
        "otlp.ExponentialHistogramDataPoint",
    ];
    for ((type_name, _), expected) in visitor.rows.iter().zip(expected_type_names) {
        assert_eq!(type_name, expected);
    }

    let names = [
        "sum_metric",
        "gauge_metric",
        "summary_metric",
        "histogram_metric",
        "exp_histogram_metric",
    ];
    for ((_, bytes), expected_name) in visitor.rows.iter().zip(names) {
        assert_eq!(metric_extra_str(bytes, "name"), expected_name);
    }

    let (_, hist_bytes) = &visitor.rows[3];
    assert_eq!(
        metric_extra_str(hist_bytes, "otel.metric.kind"),
        "histogram"
    );
    match metric_extras(hist_bytes) {
        jsonb::Value::Object(m) => {
            let temporality = m
                .get("otel.metric.aggregation_temporality")
                .expect("aggregation_temporality present");
            assert!(matches!(
                temporality,
                jsonb::Value::Number(jsonb::Number::Int64(2))
            ));
        }
        other => panic!("expected __metric to be an object, got {other:?}"),
    }

    let (_, exp_hist_bytes) = &visitor.rows[4];
    assert_eq!(
        metric_extra_str(exp_hist_bytes, "otel.metric.kind"),
        "exponential_histogram"
    );
    match metric_extras(exp_hist_bytes) {
        jsonb::Value::Object(m) => {
            let temporality = m
                .get("otel.metric.aggregation_temporality")
                .expect("aggregation_temporality present");
            assert!(matches!(
                temporality,
                jsonb::Value::Number(jsonb::Number::Int64(2))
            ));
        }
        other => panic!("expected __metric to be an object, got {other:?}"),
    }
}

#[test]
fn metrics_decoder_skips_metric_with_no_data_without_consuming_an_ordinal() {
    let no_data_metric = Metric {
        name: "empty_metric".to_string(),
        description: String::new(),
        unit: String::new(),
        metadata: vec![],
        data: None,
    };
    let sum_metric = resource_metrics_with_one_of_each()
        .scope_metrics
        .remove(0)
        .metrics
        .remove(0);
    let resource_metrics = ResourceMetrics {
        resource: None,
        scope_metrics: vec![ScopeMetrics {
            scope: None,
            metrics: vec![no_data_metric, sum_metric],
            schema_url: String::new(),
        }],
        schema_url: String::new(),
    };
    let payload = payload_of(resource_metrics.encode_to_vec());
    let stream = dummy_stream_metadata();
    let mut visitor = CollectingVisitor::default();
    OtelMetricsBlockDecoder
        .decode(&stream, &payload, &mut visitor)
        .expect("decoding a valid ResourceMetrics payload");

    // Only the Sum metric's single data point is emitted; the no-data metric
    // contributes no row and — since indices are purely positional for OTLP
    // blocks — no gap either.
    assert_eq!(visitor.rows.len(), 1);
    assert_eq!(metric_extra_str(&visitor.rows[0].1, "name"), "sum_metric");
}

// ---------------------------------------------------------------------------
// Traces
// ---------------------------------------------------------------------------

#[test]
fn traces_decoder_emits_one_row_per_span_with_events_and_links_nested() {
    let span = Span {
        trace_id: vec![0x11; 16],
        span_id: vec![0x22; 8],
        trace_state: String::new(),
        parent_span_id: vec![],
        flags: 0,
        name: "my-span".to_string(),
        kind: span::SpanKind::Internal as i32,
        start_time_unix_nano: 1,
        end_time_unix_nano: 2,
        attributes: vec![s_kv("span.attr", "x")],
        dropped_attributes_count: 0,
        events: vec![span::Event {
            time_unix_nano: 1,
            name: "my-event".to_string(),
            attributes: vec![],
            dropped_attributes_count: 0,
        }],
        dropped_events_count: 0,
        links: vec![span::Link {
            trace_id: vec![0x33; 16],
            span_id: vec![0x44; 8],
            trace_state: String::new(),
            attributes: vec![],
            dropped_attributes_count: 0,
            flags: 0,
        }],
        dropped_links_count: 0,
        status: None,
    };
    let resource_spans = ResourceSpans {
        resource: Some(Resource {
            attributes: vec![s_kv("service.name", "trace-service")],
            dropped_attributes_count: 0,
            entity_refs: vec![],
        }),
        scope_spans: vec![ScopeSpans {
            scope: Some(scope("trace-scope")),
            spans: vec![span],
            schema_url: String::new(),
        }],
        schema_url: String::new(),
    };
    let payload = payload_of(resource_spans.encode_to_vec());
    let stream = dummy_stream_metadata();
    let mut visitor = CollectingVisitor::default();
    OtelTracesBlockDecoder
        .decode(&stream, &payload, &mut visitor)
        .expect("decoding a valid ResourceSpans payload");

    assert_eq!(visitor.rows.len(), 1);
    let (type_name, bytes) = &visitor.rows[0];
    assert_eq!(type_name, "otlp.Span");

    match field(bytes, "events") {
        jsonb::Value::Array(events) => assert_eq!(events.len(), 1),
        other => panic!("expected events to be an array, got {other:?}"),
    }
    match field(bytes, "links") {
        jsonb::Value::Array(links) => assert_eq!(links.len(), 1),
        other => panic!("expected links to be an array, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Early limit / robustness
// ---------------------------------------------------------------------------

#[test]
fn logs_decoder_stops_at_early_limit() {
    let resource_logs = resource_logs_with_two_scopes(3); // 6 records total
    let payload = payload_of(resource_logs.encode_to_vec());
    let stream = dummy_stream_metadata();
    let mut visitor = StoppingVisitor {
        limit: 2,
        rows: Vec::new(),
    };
    OtelLogsBlockDecoder
        .decode(&stream, &payload, &mut visitor)
        .expect("decoding a valid ResourceLogs payload");
    assert_eq!(visitor.rows.len(), 2);
}

/// Tiny deterministic PRNG (splitmix64-style LCG) — no new dependency, and
/// stable across runs/platforms so failures are reproducible from the seed.
/// Mirrors the helper in `parse_corrupt_block_tests.rs`.
struct Lcg(u64);

impl Lcg {
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() as usize) % bound
        }
    }
}

#[test]
fn garbage_bytes_never_panic() {
    let stream = dummy_stream_metadata();
    let mut rng = Lcg(0x5EED_1234);
    for _ in 0..300 {
        let len = rng.next_usize(64);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
        let payload = payload_of(bytes);

        let mut v = CollectingVisitor::default();
        let _ = OtelLogsBlockDecoder.decode(&stream, &payload, &mut v);
        let mut v = CollectingVisitor::default();
        let _ = OtelMetricsBlockDecoder.decode(&stream, &payload, &mut v);
        let mut v = CollectingVisitor::default();
        let _ = OtelTracesBlockDecoder.decode(&stream, &payload, &mut v);
    }
}

#[test]
fn truncated_valid_resource_logs_returns_err() {
    let mut resource_logs = resource_logs_with_two_scopes(1);
    // A non-empty trailing field (the outermost message's last struct field)
    // so chopping the last byte reliably underflows a length-delimited read.
    resource_logs.schema_url = "https://example.com/schema/v1".to_string();
    let full = resource_logs.encode_to_vec();
    let truncated = full[..full.len() - 1].to_vec();
    let payload = payload_of(truncated);
    let stream = dummy_stream_metadata();
    let mut visitor = CollectingVisitor::default();
    let result = OtelLogsBlockDecoder.decode(&stream, &payload, &mut visitor);
    assert!(result.is_err(), "truncated payload should fail to decode");
}
