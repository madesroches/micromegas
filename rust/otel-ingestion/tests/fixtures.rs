//! Helpers for building OTLP proto fixtures without pulling in the full SDK.
//!
//! `opentelemetry-proto` alone is enough for tests — adding `opentelemetry-sdk`
//! and the `opentelemetry-otlp` exporter just to materialize bytes would drag
//! tokio/tower/reqwest in transitively for no test-coverage gain.
#![allow(dead_code)]

use micromegas_otel_ingestion::proto::{
    AnyValue, ExportLogsServiceRequest, ExportMetricsServiceRequest, ExportTraceServiceRequest,
    KeyValue, LogRecord, Metric, ResourceLogs, ResourceMetrics, ResourceSpans, ScopeLogs,
    ScopeMetrics, ScopeSpans, Span, any_value::Value as AnyVal, metric,
};
use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::metrics::v1::{Gauge, NumberDataPoint, Sum, number_data_point};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::span;

pub fn s_kv(k: &str, v: &str) -> KeyValue {
    KeyValue {
        key: k.into(),
        value: Some(AnyValue {
            value: Some(AnyVal::StringValue(v.into())),
        }),
    }
}

pub fn i_kv(k: &str, v: i64) -> KeyValue {
    KeyValue {
        key: k.into(),
        value: Some(AnyValue {
            value: Some(AnyVal::IntValue(v)),
        }),
    }
}

pub fn scope(name: &str) -> InstrumentationScope {
    InstrumentationScope {
        name: name.into(),
        version: "0.0.0".into(),
        attributes: vec![],
        dropped_attributes_count: 0,
    }
}

/// Builds an `ExportLogsServiceRequest` carrying `records` under one resource and one scope.
pub fn make_logs_request(
    service_name: &str,
    host: &str,
    pid: i64,
    records: Vec<LogRecord>,
) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    s_kv("service.name", service_name),
                    s_kv("host.name", host),
                    i_kv("process.pid", pid),
                ],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(scope("test.fixture")),
                log_records: records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

/// `LogRecord` with both timestamps set to zero — models an SDK that omits all timestamps.
pub fn log_record_no_timestamp(severity: i32, body: &str) -> LogRecord {
    LogRecord {
        time_unix_nano: 0,
        observed_time_unix_nano: 0,
        severity_number: severity,
        severity_text: String::new(),
        body: Some(AnyValue {
            value: Some(AnyVal::StringValue(body.into())),
        }),
        attributes: vec![],
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: vec![],
        span_id: vec![],
        event_name: String::new(),
    }
}

/// `LogRecord` with `time_unix_nano = 0` and a non-zero `observed_time_unix_nano`.
pub fn log_record_observed_only(observed_nanos: u64, severity: i32, body: &str) -> LogRecord {
    LogRecord {
        time_unix_nano: 0,
        observed_time_unix_nano: observed_nanos,
        severity_number: severity,
        severity_text: String::new(),
        body: Some(AnyValue {
            value: Some(AnyVal::StringValue(body.into())),
        }),
        attributes: vec![],
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: vec![],
        span_id: vec![],
        event_name: String::new(),
    }
}

pub fn log_record(time_unix_nano: u64, severity: i32, body: &str) -> LogRecord {
    LogRecord {
        time_unix_nano,
        observed_time_unix_nano: 0,
        severity_number: severity,
        severity_text: String::new(),
        body: Some(AnyValue {
            value: Some(AnyVal::StringValue(body.into())),
        }),
        attributes: vec![],
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: vec![],
        span_id: vec![],
        event_name: String::new(),
    }
}

pub fn make_metrics_request(
    service_name: &str,
    host: &str,
    pid: i64,
    metrics: Vec<Metric>,
) -> ExportMetricsServiceRequest {
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![
                    s_kv("service.name", service_name),
                    s_kv("host.name", host),
                    i_kv("process.pid", pid),
                ],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_metrics: vec![ScopeMetrics {
                scope: Some(scope("test.metrics")),
                metrics,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

pub fn sum_metric(name: &str, unit: &str, time: u64, value: f64) -> Metric {
    Metric {
        name: name.into(),
        description: String::new(),
        unit: unit.into(),
        metadata: vec![],
        data: Some(metric::Data::Sum(Sum {
            data_points: vec![NumberDataPoint {
                attributes: vec![],
                start_time_unix_nano: time,
                time_unix_nano: time,
                exemplars: vec![],
                flags: 0,
                value: Some(number_data_point::Value::AsDouble(value)),
            }],
            aggregation_temporality: 2, // CUMULATIVE
            is_monotonic: true,
        })),
    }
}

pub fn gauge_metric(name: &str, unit: &str, time: u64, value: i64) -> Metric {
    Metric {
        name: name.into(),
        description: String::new(),
        unit: unit.into(),
        metadata: vec![],
        data: Some(metric::Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                attributes: vec![],
                start_time_unix_nano: 0,
                time_unix_nano: time,
                exemplars: vec![],
                flags: 0,
                value: Some(number_data_point::Value::AsInt(value)),
            }],
        })),
    }
}

pub fn make_traces_request(
    service_name: &str,
    host: &str,
    pid: i64,
    spans: Vec<Span>,
) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![
                    s_kv("service.name", service_name),
                    s_kv("host.name", host),
                    i_kv("process.pid", pid),
                ],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_spans: vec![ScopeSpans {
                scope: Some(scope("test.spans")),
                spans,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

pub fn root_span(name: &str, trace_id: [u8; 16], span_id: [u8; 8], start: u64, end: u64) -> Span {
    Span {
        trace_id: trace_id.to_vec(),
        span_id: span_id.to_vec(),
        trace_state: String::new(),
        parent_span_id: vec![],
        flags: 0,
        name: name.into(),
        kind: span::SpanKind::Internal as i32,
        start_time_unix_nano: start,
        end_time_unix_nano: end,
        attributes: vec![],
        dropped_attributes_count: 0,
        events: vec![],
        dropped_events_count: 0,
        links: vec![],
        dropped_links_count: 0,
        status: None,
    }
}
