//! `BlockObjectDecoder` implementations for OTLP `ResourceLogs`/`ResourceMetrics`/
//! `ResourceSpans` block payloads, used by `parse_block` to give a debug-level view
//! into OTLP blocks (see the design plan for the row-shape rationale).
//!
//! Each decoder walks `scope_* → leaf records` and emits one row per leaf (log
//! record / span / metric data point), in payload order. The row's `value` is a
//! faithful OTLP/JSON dump of the leaf (via `serde_json` + `jsonb::Value::from`),
//! plus a synthesized `__`-prefixed envelope (`__type`, `__attributes`,
//! `__resource`, `__scope`, and — metrics only — `__metric`).

use super::attrs::{attrs_to_jsonb_value, scope_extras};
use crate::lakehouse::block_object_decoder::{BlockObjectDecoder, ObjectVisitor};
use crate::metadata::StreamMetadata;
use anyhow::{Context, Result};
use jsonb::Value as JsonbValue;
use micromegas_telemetry::block_wire_format::BlockPayload;
use micromegas_tracing::prelude::*;
use opentelemetry_proto::tonic::common::v1::KeyValue;
use opentelemetry_proto::tonic::logs::v1::ResourceLogs;
use opentelemetry_proto::tonic::metrics::v1::{
    ExponentialHistogramDataPoint, HistogramDataPoint, NumberDataPoint, ResourceMetrics,
    SummaryDataPoint, metric::Data,
};
use opentelemetry_proto::tonic::trace::v1::ResourceSpans;
use prost::Message;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::BTreeMap;

/// Builds the JSONB `value` for one OTLP leaf record (a `LogRecord`, `Span`, or
/// metric data point): a faithful OTLP/JSON dump of `leaf` (via `serde_json` +
/// `jsonb::Value::from`), with a synthesized `__`-prefixed envelope layered on top
/// (`__type`, `__attributes`, `__resource`, `__scope`, and — when `metric_extras`
/// is non-empty — `__metric`). See the design plan §"JSONB value shape" for the
/// full rationale, including the known lossy cases (non-finite `f64` → `null`,
/// `asInt`/`Exemplar.as_int` as bare numbers rather than quoted strings).
///
/// `resource` and `scope` are the already-built `__resource`/`__scope` envelope
/// values for the enclosing resource/scope block — callers compute these once
/// per resource/scope (via `attrs_to_jsonb_value`/`scope_extras`) rather than
/// once per leaf, since they're invariant across every leaf sharing that block.
fn leaf_jsonb<T: Serialize>(
    type_name: &str,
    leaf: &T,
    leaf_attrs: &[KeyValue],
    resource: &JsonbValue<'static>,
    scope: &JsonbValue<'static>,
    metric_extras: &[(String, JsonbValue<'static>)],
) -> Result<Vec<u8>> {
    let json = serde_json::to_value(leaf).context("serializing OTLP leaf to JSON")?;
    let mut map: BTreeMap<String, JsonbValue<'static>> = match JsonbValue::from(&json) {
        JsonbValue::Object(map) => map,
        // Every OTLP leaf message (LogRecord/Span/*DataPoint) is a struct, so
        // `serde_json::to_value` always yields a JSON object. Defensive fallback
        // rather than a panic if that ever stops being true upstream.
        other => {
            let mut map = BTreeMap::new();
            map.insert("__value".to_string(), other);
            map
        }
    };

    map.insert(
        "__type".to_string(),
        JsonbValue::String(Cow::Owned(type_name.to_string())),
    );
    map.insert(
        "__attributes".to_string(),
        attrs_to_jsonb_value(leaf_attrs, &[]),
    );
    map.insert("__resource".to_string(), resource.clone());
    map.insert("__scope".to_string(), scope.clone());
    if !metric_extras.is_empty() {
        map.insert(
            "__metric".to_string(),
            JsonbValue::Object(metric_extras.iter().cloned().collect()),
        );
    }

    let mut buf = Vec::new();
    JsonbValue::Object(map).write_to_vec(&mut buf);
    Ok(buf)
}

/// Decodes OTLP `ResourceLogs` block payloads (`streams.format = "otlp/v1/logs"`)
/// into one row per `LogRecord`.
#[derive(Debug)]
pub struct OtelLogsBlockDecoder;

impl BlockObjectDecoder for OtelLogsBlockDecoder {
    fn decode(
        &self,
        _stream: &StreamMetadata,
        payload: &BlockPayload,
        visitor: &mut dyn ObjectVisitor,
    ) -> Result<()> {
        // Block payload format: BlockPayload { dependencies: [], objects: <ResourceLogs proto> }.
        // Not compressed, unlike the transit path — see `otel-ingestion/src/block.rs`.
        let resource_logs = ResourceLogs::decode(payload.objects.as_slice())
            .inspect_err(|e| error!("corrupt OTLP logs block payload: {e:?}"))
            .context("decoding ResourceLogs proto")?;

        let resource = resource_logs.resource.as_ref();
        let resource_attrs: &[KeyValue] = resource.map_or(&[], |r| r.attributes.as_slice());
        let resource_jsonb = attrs_to_jsonb_value(resource_attrs, &[]);
        for scope_logs in &resource_logs.scope_logs {
            let scope = scope_logs.scope.as_ref();
            let scope_jsonb = JsonbValue::Object(
                scope_extras(scope, &scope_logs.schema_url)
                    .into_iter()
                    .collect(),
            );
            for record in &scope_logs.log_records {
                let bytes = leaf_jsonb(
                    "otlp.LogRecord",
                    record,
                    &record.attributes,
                    &resource_jsonb,
                    &scope_jsonb,
                    &[],
                )?;
                if !visitor.visit("otlp.LogRecord", &bytes)? {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

/// Decodes OTLP `ResourceSpans` block payloads (`streams.format = "otlp/v1/traces"`)
/// into one row per `Span` (with its `events`/`links` nested in the JSONB value).
#[derive(Debug)]
pub struct OtelTracesBlockDecoder;

impl BlockObjectDecoder for OtelTracesBlockDecoder {
    fn decode(
        &self,
        _stream: &StreamMetadata,
        payload: &BlockPayload,
        visitor: &mut dyn ObjectVisitor,
    ) -> Result<()> {
        let resource_spans = ResourceSpans::decode(payload.objects.as_slice())
            .inspect_err(|e| error!("corrupt OTLP traces block payload: {e:?}"))
            .context("decoding ResourceSpans proto")?;

        let resource = resource_spans.resource.as_ref();
        let resource_attrs: &[KeyValue] = resource.map_or(&[], |r| r.attributes.as_slice());
        let resource_jsonb = attrs_to_jsonb_value(resource_attrs, &[]);
        for scope_spans in &resource_spans.scope_spans {
            let scope = scope_spans.scope.as_ref();
            let scope_jsonb = JsonbValue::Object(
                scope_extras(scope, &scope_spans.schema_url)
                    .into_iter()
                    .collect(),
            );
            for span in &scope_spans.spans {
                let bytes = leaf_jsonb(
                    "otlp.Span",
                    span,
                    &span.attributes,
                    &resource_jsonb,
                    &scope_jsonb,
                    &[],
                )?;
                if !visitor.visit("otlp.Span", &bytes)? {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

/// Gives generic access to the `attributes` field shared by all four OTLP metric
/// data point message types, so `emit_data_points` can build `leaf_jsonb`'s
/// `leaf_attrs` argument without matching on the concrete data point type.
trait DataPointAttrs {
    fn attributes(&self) -> &[KeyValue];
}

impl DataPointAttrs for NumberDataPoint {
    fn attributes(&self) -> &[KeyValue] {
        &self.attributes
    }
}

impl DataPointAttrs for HistogramDataPoint {
    fn attributes(&self) -> &[KeyValue] {
        &self.attributes
    }
}

impl DataPointAttrs for ExponentialHistogramDataPoint {
    fn attributes(&self) -> &[KeyValue] {
        &self.attributes
    }
}

impl DataPointAttrs for SummaryDataPoint {
    fn attributes(&self) -> &[KeyValue] {
        &self.attributes
    }
}

/// Shared emit loop for one `metric::Data` variant's data points: builds each
/// leaf's JSONB `value` via `leaf_jsonb` and feeds it to `visitor`, stopping
/// early if the visitor returns `false`. Factors out the logic that was
/// previously duplicated across the Sum/Gauge/Histogram/ExponentialHistogram/
/// Summary match arms in `OtelMetricsBlockDecoder::decode` — only the
/// kind-specific `metric_extras` (aggregation_temporality, is_monotonic, ...)
/// stay inline in each arm.
///
/// Returns `Ok(false)` if the visitor asked to stop; callers should return
/// `Ok(())` from `decode` in that case.
fn emit_data_points<T: Serialize + DataPointAttrs>(
    type_name: &str,
    data_points: &[T],
    resource: &JsonbValue<'static>,
    scope: &JsonbValue<'static>,
    metric_extras: &[(String, JsonbValue<'static>)],
    visitor: &mut dyn ObjectVisitor,
) -> Result<bool> {
    for dp in data_points {
        let bytes = leaf_jsonb(
            type_name,
            dp,
            dp.attributes(),
            resource,
            scope,
            metric_extras,
        )?;
        if !visitor.visit(type_name, &bytes)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Decodes OTLP `ResourceMetrics` block payloads (`streams.format = "otlp/v1/metrics"`)
/// into one row per data point (Sum/Gauge → `otlp.NumberDataPoint`, Histogram →
/// `otlp.HistogramDataPoint`, ExponentialHistogram → `otlp.ExponentialHistogramDataPoint`,
/// Summary → `otlp.SummaryDataPoint`). Per-data-point granularity lines up with
/// `measures` and with `metrics_bounds`'s `nb_objects` counting basis (see the
/// design plan §"Per-data-point vs. per-`Metric` rows for metrics").
///
/// The parent `Metric`'s `name`/`unit`/`description`, plus kind-specific fields
/// unreachable from the data point itself (`aggregation_temporality`,
/// `is_monotonic`, `metadata`), ride along in every row's `__metric` object —
/// see `leaf_jsonb`.
#[derive(Debug)]
pub struct OtelMetricsBlockDecoder;

impl BlockObjectDecoder for OtelMetricsBlockDecoder {
    fn decode(
        &self,
        _stream: &StreamMetadata,
        payload: &BlockPayload,
        visitor: &mut dyn ObjectVisitor,
    ) -> Result<()> {
        let resource_metrics = ResourceMetrics::decode(payload.objects.as_slice())
            .inspect_err(|e| error!("corrupt OTLP metrics block payload: {e:?}"))
            .context("decoding ResourceMetrics proto")?;

        let resource = resource_metrics.resource.as_ref();
        let resource_attrs: &[KeyValue] = resource.map_or(&[], |r| r.attributes.as_slice());
        let resource_jsonb = attrs_to_jsonb_value(resource_attrs, &[]);
        for scope_metrics in &resource_metrics.scope_metrics {
            let scope = scope_metrics.scope.as_ref();
            let scope_jsonb = JsonbValue::Object(
                scope_extras(scope, &scope_metrics.schema_url)
                    .into_iter()
                    .collect(),
            );
            for metric in &scope_metrics.metrics {
                let Some(data) = metric.data.as_ref() else {
                    // A `Metric` with `data: None` emits nothing and consumes no
                    // ordinal: `object_index` is purely positional over emitted
                    // leaves for OTLP blocks, so calling `visitor.skip()` here
                    // would invent a gap rather than preserve one.
                    debug!(
                        "OTel metric '{}' has no data, skipping (no row emitted, no ordinal consumed)",
                        metric.name
                    );
                    continue;
                };

                // Fields the parent `Metric` carries that no data point can express
                // on its own: name/unit/description (identity), plus `metadata`
                // (tag 12) when non-empty. Common to every leaf kind below.
                let mut extras: Vec<(String, JsonbValue<'static>)> = vec![
                    (
                        "name".to_string(),
                        JsonbValue::String(Cow::Owned(metric.name.clone())),
                    ),
                    (
                        "unit".to_string(),
                        JsonbValue::String(Cow::Owned(metric.unit.clone())),
                    ),
                    (
                        "description".to_string(),
                        JsonbValue::String(Cow::Owned(metric.description.clone())),
                    ),
                ];
                if !metric.metadata.is_empty() {
                    extras.push((
                        "otel.metric.metadata".to_string(),
                        attrs_to_jsonb_value(&metric.metadata, &[]),
                    ));
                }

                match data {
                    Data::Sum(sum) => {
                        extras.push((
                            "otel.metric.kind".to_string(),
                            JsonbValue::String(Cow::Borrowed("sum")),
                        ));
                        extras.push((
                            "otel.metric.aggregation_temporality".to_string(),
                            JsonbValue::Number(jsonb::Number::Int64(
                                sum.aggregation_temporality as i64,
                            )),
                        ));
                        extras.push((
                            "otel.metric.is_monotonic".to_string(),
                            JsonbValue::Bool(sum.is_monotonic),
                        ));
                        if !emit_data_points(
                            "otlp.NumberDataPoint",
                            &sum.data_points,
                            &resource_jsonb,
                            &scope_jsonb,
                            &extras,
                            visitor,
                        )? {
                            return Ok(());
                        }
                    }
                    Data::Gauge(gauge) => {
                        extras.push((
                            "otel.metric.kind".to_string(),
                            JsonbValue::String(Cow::Borrowed("gauge")),
                        ));
                        if !emit_data_points(
                            "otlp.NumberDataPoint",
                            &gauge.data_points,
                            &resource_jsonb,
                            &scope_jsonb,
                            &extras,
                            visitor,
                        )? {
                            return Ok(());
                        }
                    }
                    Data::Histogram(h) => {
                        extras.push((
                            "otel.metric.kind".to_string(),
                            JsonbValue::String(Cow::Borrowed("histogram")),
                        ));
                        extras.push((
                            "otel.metric.aggregation_temporality".to_string(),
                            JsonbValue::Number(jsonb::Number::Int64(
                                h.aggregation_temporality as i64,
                            )),
                        ));
                        if !emit_data_points(
                            "otlp.HistogramDataPoint",
                            &h.data_points,
                            &resource_jsonb,
                            &scope_jsonb,
                            &extras,
                            visitor,
                        )? {
                            return Ok(());
                        }
                    }
                    Data::ExponentialHistogram(h) => {
                        extras.push((
                            "otel.metric.kind".to_string(),
                            JsonbValue::String(Cow::Borrowed("exponential_histogram")),
                        ));
                        extras.push((
                            "otel.metric.aggregation_temporality".to_string(),
                            JsonbValue::Number(jsonb::Number::Int64(
                                h.aggregation_temporality as i64,
                            )),
                        ));
                        if !emit_data_points(
                            "otlp.ExponentialHistogramDataPoint",
                            &h.data_points,
                            &resource_jsonb,
                            &scope_jsonb,
                            &extras,
                            visitor,
                        )? {
                            return Ok(());
                        }
                    }
                    Data::Summary(s) => {
                        extras.push((
                            "otel.metric.kind".to_string(),
                            JsonbValue::String(Cow::Borrowed("summary")),
                        ));
                        if !emit_data_points(
                            "otlp.SummaryDataPoint",
                            &s.data_points,
                            &resource_jsonb,
                            &scope_jsonb,
                            &extras,
                            visitor,
                        )? {
                            return Ok(());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
