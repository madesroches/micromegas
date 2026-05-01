//! Splitting an OTLP `Export*ServiceRequest` into per-Resource micromegas blocks.
//!
//! An `ExportRequest` may carry multiple resources (different services). We split at
//! the Resource boundary so each block has an unambiguous `process_id`.

use crate::identity::{
    SignalKey, attr_to_string, block_id_from_payload, is_degenerate_resource,
    process_id_from_resource, stream_id_from_process_signal,
};
use crate::proto::{KeyValue, ResourceLogs, ResourceMetrics, ResourceSpans};
use anyhow::Result;
use chrono::{DateTime, Utc};
use micromegas_telemetry::block_wire_format::{Block, BlockPayload};
use micromegas_telemetry::property::Property;
use micromegas_tracing::prelude::*;
use prost::Message;
use uuid::Uuid;

/// A single per-resource block ready to be written. Carries everything the ingestion
/// service needs to register the process + stream + block.
pub struct PreparedBlock {
    pub process_id: Uuid,
    pub stream_id: Uuid,
    pub block: Block,
    pub signal: SignalKey,
    pub resource_attrs: Vec<KeyValue>,
    /// Number of records in the resource submessage (logs / metric data points / spans).
    pub nb_records: i32,
    /// Bounds derived from per-record timestamps. Used both for the `Block` envelope
    /// and for the `processes.start_time` fallback when synthesizing a brand new process.
    pub begin_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
}

/// Walks `ResourceLogs` to find min/max `time_unix_nano`. Falls back to
/// `observed_time_unix_nano` when the per-record `time` is 0.
fn logs_bounds(rl: &ResourceLogs) -> Option<(i64, i64, i32)> {
    let mut min = i64::MAX;
    let mut max = i64::MIN;
    let mut count = 0i32;
    for scope in &rl.scope_logs {
        for record in &scope.log_records {
            count += 1;
            let t = if record.time_unix_nano != 0 {
                record.time_unix_nano as i64
            } else if record.observed_time_unix_nano != 0 {
                record.observed_time_unix_nano as i64
            } else {
                continue;
            };
            min = min.min(t);
            max = max.max(t);
        }
    }
    if min == i64::MAX {
        if count == 0 {
            None
        } else {
            // All records have zero timestamps — fall back to wall clock at handler.
            Some((0, 0, count))
        }
    } else {
        Some((min, max, count))
    }
}

/// Walks `ResourceSpans` for min(start_time) / max(end_time).
fn spans_bounds(rs: &ResourceSpans) -> Option<(i64, i64, i32)> {
    let mut min = i64::MAX;
    let mut max = i64::MIN;
    let mut count = 0i32;
    for scope in &rs.scope_spans {
        for span in &scope.spans {
            count += 1;
            if span.start_time_unix_nano != 0 {
                min = min.min(span.start_time_unix_nano as i64);
            }
            if span.end_time_unix_nano != 0 {
                max = max.max(span.end_time_unix_nano as i64);
            }
        }
    }
    if count == 0 {
        None
    } else if min == i64::MAX || max == i64::MIN {
        Some((0, 0, count))
    } else {
        Some((min, max, count))
    }
}

/// Walks `ResourceMetrics` for min/max `time_unix_nano` across every Sum/Gauge/Histogram point.
/// Histogram/ExponentialHistogram/Summary points still count toward bounds even though
/// the v1 processor skips them — keeps block insert-time predicates consistent with
/// payload contents.
fn metrics_bounds(rm: &ResourceMetrics) -> Option<(i64, i64, i32)> {
    use crate::proto::metric::Data;
    let mut min = i64::MAX;
    let mut max = i64::MIN;
    let mut count = 0i32;
    for scope in &rm.scope_metrics {
        for metric in &scope.metrics {
            match metric.data.as_ref() {
                Some(Data::Sum(s)) => {
                    for dp in &s.data_points {
                        count += 1;
                        let t = dp.time_unix_nano as i64;
                        if t != 0 {
                            min = min.min(t);
                            max = max.max(t);
                        }
                    }
                }
                Some(Data::Gauge(g)) => {
                    for dp in &g.data_points {
                        count += 1;
                        let t = dp.time_unix_nano as i64;
                        if t != 0 {
                            min = min.min(t);
                            max = max.max(t);
                        }
                    }
                }
                Some(Data::Histogram(h)) => {
                    for dp in &h.data_points {
                        count += 1;
                        let t = dp.time_unix_nano as i64;
                        if t != 0 {
                            min = min.min(t);
                            max = max.max(t);
                        }
                    }
                }
                Some(Data::ExponentialHistogram(h)) => {
                    for dp in &h.data_points {
                        count += 1;
                        let t = dp.time_unix_nano as i64;
                        if t != 0 {
                            min = min.min(t);
                            max = max.max(t);
                        }
                    }
                }
                Some(Data::Summary(s)) => {
                    for dp in &s.data_points {
                        count += 1;
                        let t = dp.time_unix_nano as i64;
                        if t != 0 {
                            min = min.min(t);
                            max = max.max(t);
                        }
                    }
                }
                None => {}
            }
        }
    }
    if count == 0 {
        None
    } else if min == i64::MAX || max == i64::MIN {
        Some((0, 0, count))
    } else {
        Some((min, max, count))
    }
}

fn nanos_to_datetime(nanos: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_nanos(nanos)
}

/// Builds a single `PreparedBlock` from a per-resource payload.
fn build_prepared_block(
    payload_bytes: Vec<u8>,
    resource_attrs: Vec<KeyValue>,
    signal: SignalKey,
    bounds: (i64, i64, i32),
) -> PreparedBlock {
    let (min_nanos, max_nanos, nb_records) = bounds;
    let (begin_time, end_time) = if min_nanos == 0 && max_nanos == 0 {
        let now = Utc::now();
        (now, now)
    } else {
        (nanos_to_datetime(min_nanos), nanos_to_datetime(max_nanos))
    };

    let resource = crate::proto::Resource {
        attributes: resource_attrs.clone(),
        dropped_attributes_count: 0,
        entity_refs: vec![],
    };
    let process_id = process_id_from_resource(Some(&resource));
    let stream_id = stream_id_from_process_signal(process_id, signal);
    let block_id = block_id_from_payload(&payload_bytes);

    if is_degenerate_resource(&resource_attrs) {
        warn!(
            "OTLP resource without host.id/host.name/process.pid/service.instance.id — \
             multiple processes may collapse onto process_id={}",
            process_id
        );
    }

    let nb_objects = nb_records;
    let begin_ticks = begin_time.timestamp_nanos_opt().unwrap_or(0);
    let end_ticks = end_time.timestamp_nanos_opt().unwrap_or(0);

    let block = Block {
        block_id,
        stream_id,
        process_id,
        begin_time: begin_time.to_rfc3339(),
        begin_ticks,
        end_time: end_time.to_rfc3339(),
        end_ticks,
        payload: BlockPayload {
            dependencies: Vec::new(),
            objects: payload_bytes,
        },
        object_offset: 0,
        nb_objects,
    };

    PreparedBlock {
        process_id,
        stream_id,
        block,
        signal,
        resource_attrs,
        nb_records,
        begin_time,
        end_time,
    }
}

/// Splits a logs request into per-resource blocks.
pub fn split_logs(req: crate::proto::ExportLogsServiceRequest) -> Result<Vec<PreparedBlock>> {
    let mut out = Vec::with_capacity(req.resource_logs.len());
    for rl in req.resource_logs {
        let Some(bounds) = logs_bounds(&rl) else {
            continue;
        };
        let resource_attrs = rl
            .resource
            .as_ref()
            .map(|r| r.attributes.clone())
            .unwrap_or_default();
        let payload_bytes = rl.encode_to_vec();
        out.push(build_prepared_block(
            payload_bytes,
            resource_attrs,
            SignalKey::Logs,
            bounds,
        ));
    }
    Ok(out)
}

/// Splits a metrics request into per-resource blocks.
pub fn split_metrics(req: crate::proto::ExportMetricsServiceRequest) -> Result<Vec<PreparedBlock>> {
    let mut out = Vec::with_capacity(req.resource_metrics.len());
    for rm in req.resource_metrics {
        let Some(bounds) = metrics_bounds(&rm) else {
            continue;
        };
        let resource_attrs = rm
            .resource
            .as_ref()
            .map(|r| r.attributes.clone())
            .unwrap_or_default();
        let payload_bytes = rm.encode_to_vec();
        out.push(build_prepared_block(
            payload_bytes,
            resource_attrs,
            SignalKey::Metrics,
            bounds,
        ));
    }
    Ok(out)
}

/// Splits a trace request into per-resource blocks.
pub fn split_traces(req: crate::proto::ExportTraceServiceRequest) -> Result<Vec<PreparedBlock>> {
    let mut out = Vec::with_capacity(req.resource_spans.len());
    for rs in req.resource_spans {
        let Some(bounds) = spans_bounds(&rs) else {
            continue;
        };
        let resource_attrs = rs
            .resource
            .as_ref()
            .map(|r| r.attributes.clone())
            .unwrap_or_default();
        let payload_bytes = rs.encode_to_vec();
        out.push(build_prepared_block(
            payload_bytes,
            resource_attrs,
            SignalKey::Traces,
            bounds,
        ));
    }
    Ok(out)
}

/// Pulls the well-known process attributes off a resource to build the row that goes
/// in the `processes` table. Everything else gets folded into `properties` under
/// `otel.resource.*` so it's queryable but doesn't bloat the typed columns.
pub struct ProcessFromResource {
    pub exe: String,
    pub username: String,
    pub computer: String,
    pub distro: String,
    pub cpu_brand: String,
    pub start_time: DateTime<Utc>,
    pub start_ticks: i64,
    pub properties: Vec<Property>,
}

impl ProcessFromResource {
    pub fn build(attrs: &[KeyValue], fallback_start: DateTime<Utc>) -> Self {
        let svc_name = crate::identity::attr(attrs, "service.name")
            .map(crate::identity::attr_to_string)
            .unwrap_or_default();
        let svc_ns = crate::identity::attr(attrs, "service.namespace")
            .map(crate::identity::attr_to_string)
            .unwrap_or_default();
        let exe = if svc_ns.is_empty() {
            svc_name
        } else {
            format!("{svc_ns}/{svc_name}")
        };

        let username = crate::identity::attr(attrs, "user.name")
            .map(crate::identity::attr_to_string)
            .unwrap_or_default();
        let computer = crate::identity::attr(attrs, "host.name")
            .map(crate::identity::attr_to_string)
            .unwrap_or_default();
        let distro = crate::identity::attr(attrs, "os.description")
            .map(crate::identity::attr_to_string)
            .unwrap_or_default();
        let cpu_brand = crate::identity::attr(attrs, "host.cpu.model.name")
            .map(crate::identity::attr_to_string)
            .unwrap_or_default();

        // Prefer the OTel-stable `process.creation.time`; accept legacy `process.start_time` as a fallback.
        let start_time = crate::identity::attr(attrs, "process.creation.time")
            .or_else(|| crate::identity::attr(attrs, "process.start_time"))
            .and_then(|v| DateTime::parse_from_rfc3339(&attr_to_string(v)).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(fallback_start);
        let start_ticks = start_time.timestamp_nanos_opt().unwrap_or(0);

        // Properties: everything else, plus the well-known fields under otel.resource.* for queryability.
        let mut properties = Vec::with_capacity(attrs.len());
        for kv in attrs {
            let value = kv.value.as_ref().map(attr_to_string).unwrap_or_default();
            properties.push(Property::new(
                std::sync::Arc::new(format!("otel.resource.{}", kv.key)),
                std::sync::Arc::new(value),
            ));
        }

        ProcessFromResource {
            exe,
            username,
            computer,
            distro,
            cpu_brand,
            start_time,
            start_ticks,
            properties,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{AnyValue, LogRecord, ResourceLogs, ScopeLogs, any_value::Value as AvValue};

    fn s_kv(k: &str, v: &str) -> KeyValue {
        KeyValue {
            key: k.into(),
            value: Some(AnyValue {
                value: Some(AvValue::StringValue(v.into())),
            }),
        }
    }

    #[test]
    fn split_logs_one_block_per_resource() {
        let req = crate::proto::ExportLogsServiceRequest {
            resource_logs: vec![
                ResourceLogs {
                    resource: Some(crate::proto::Resource {
                        attributes: vec![s_kv("service.name", "svc-a")],
                        dropped_attributes_count: 0,
                        entity_refs: vec![],
                    }),
                    scope_logs: vec![ScopeLogs {
                        scope: None,
                        log_records: vec![LogRecord {
                            time_unix_nano: 1_700_000_000_000_000_000,
                            observed_time_unix_nano: 0,
                            severity_number: 9,
                            severity_text: String::new(),
                            body: None,
                            attributes: vec![],
                            dropped_attributes_count: 0,
                            flags: 0,
                            trace_id: vec![],
                            span_id: vec![],
                            event_name: String::new(),
                        }],
                        schema_url: String::new(),
                    }],
                    schema_url: String::new(),
                },
                ResourceLogs {
                    resource: Some(crate::proto::Resource {
                        attributes: vec![s_kv("service.name", "svc-b")],
                        dropped_attributes_count: 0,
                        entity_refs: vec![],
                    }),
                    scope_logs: vec![ScopeLogs {
                        scope: None,
                        log_records: vec![LogRecord {
                            time_unix_nano: 1_700_000_001_000_000_000,
                            observed_time_unix_nano: 0,
                            severity_number: 9,
                            severity_text: String::new(),
                            body: None,
                            attributes: vec![],
                            dropped_attributes_count: 0,
                            flags: 0,
                            trace_id: vec![],
                            span_id: vec![],
                            event_name: String::new(),
                        }],
                        schema_url: String::new(),
                    }],
                    schema_url: String::new(),
                },
            ],
        };
        let blocks = split_logs(req).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_ne!(blocks[0].process_id, blocks[1].process_id);
        assert_eq!(blocks[0].nb_records, 1);
    }

    #[test]
    fn split_logs_skips_empty_resource() {
        let req = crate::proto::ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(crate::proto::Resource {
                    attributes: vec![s_kv("service.name", "svc")],
                    dropped_attributes_count: 0,
                    entity_refs: vec![],
                }),
                scope_logs: vec![],
                schema_url: String::new(),
            }],
        };
        let blocks = split_logs(req).unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn block_id_changes_when_payload_changes() {
        let mk = |svc: &str| ResourceLogs {
            resource: Some(crate::proto::Resource {
                attributes: vec![s_kv("service.name", svc)],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![LogRecord {
                    time_unix_nano: 1,
                    observed_time_unix_nano: 0,
                    severity_number: 9,
                    severity_text: String::new(),
                    body: None,
                    attributes: vec![],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: vec![],
                    span_id: vec![],
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        };
        let a = split_logs(crate::proto::ExportLogsServiceRequest {
            resource_logs: vec![mk("a")],
        })
        .unwrap();
        let b = split_logs(crate::proto::ExportLogsServiceRequest {
            resource_logs: vec![mk("b")],
        })
        .unwrap();
        assert_ne!(a[0].block.block_id, b[0].block.block_id);
    }
}
