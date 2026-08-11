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
use opentelemetry_proto::tonic::common::v1::{InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::ResourceLogs;
use opentelemetry_proto::tonic::metrics::v1::{ResourceMetrics, metric::Data};
use opentelemetry_proto::tonic::resource::v1::Resource;
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
#[allow(clippy::too_many_arguments)]
fn leaf_jsonb<T: Serialize>(
    type_name: &str,
    leaf: &T,
    leaf_attrs: &[KeyValue],
    resource: Option<&Resource>,
    scope: Option<&InstrumentationScope>,
    schema_url: &str,
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
    let resource_attrs: &[KeyValue] = resource.map_or(&[], |r| r.attributes.as_slice());
    map.insert(
        "__resource".to_string(),
        attrs_to_jsonb_value(resource_attrs, &[]),
    );
    map.insert(
        "__scope".to_string(),
        JsonbValue::Object(scope_extras(scope, schema_url).into_iter().collect()),
    );
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
        for scope_logs in &resource_logs.scope_logs {
            let scope = scope_logs.scope.as_ref();
            for record in &scope_logs.log_records {
                let bytes = leaf_jsonb(
                    "otlp.LogRecord",
                    record,
                    &record.attributes,
                    resource,
                    scope,
                    &scope_logs.schema_url,
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
        for scope_spans in &resource_spans.scope_spans {
            let scope = scope_spans.scope.as_ref();
            for span in &scope_spans.spans {
                let bytes = leaf_jsonb(
                    "otlp.Span",
                    span,
                    &span.attributes,
                    resource,
                    scope,
                    &scope_spans.schema_url,
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
        for scope_metrics in &resource_metrics.scope_metrics {
            let scope = scope_metrics.scope.as_ref();
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
                        for dp in &sum.data_points {
                            let bytes = leaf_jsonb(
                                "otlp.NumberDataPoint",
                                dp,
                                &dp.attributes,
                                resource,
                                scope,
                                &scope_metrics.schema_url,
                                &extras,
                            )?;
                            if !visitor.visit("otlp.NumberDataPoint", &bytes)? {
                                return Ok(());
                            }
                        }
                    }
                    Data::Gauge(gauge) => {
                        extras.push((
                            "otel.metric.kind".to_string(),
                            JsonbValue::String(Cow::Borrowed("gauge")),
                        ));
                        for dp in &gauge.data_points {
                            let bytes = leaf_jsonb(
                                "otlp.NumberDataPoint",
                                dp,
                                &dp.attributes,
                                resource,
                                scope,
                                &scope_metrics.schema_url,
                                &extras,
                            )?;
                            if !visitor.visit("otlp.NumberDataPoint", &bytes)? {
                                return Ok(());
                            }
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
                        for dp in &h.data_points {
                            let bytes = leaf_jsonb(
                                "otlp.HistogramDataPoint",
                                dp,
                                &dp.attributes,
                                resource,
                                scope,
                                &scope_metrics.schema_url,
                                &extras,
                            )?;
                            if !visitor.visit("otlp.HistogramDataPoint", &bytes)? {
                                return Ok(());
                            }
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
                        for dp in &h.data_points {
                            let bytes = leaf_jsonb(
                                "otlp.ExponentialHistogramDataPoint",
                                dp,
                                &dp.attributes,
                                resource,
                                scope,
                                &scope_metrics.schema_url,
                                &extras,
                            )?;
                            if !visitor.visit("otlp.ExponentialHistogramDataPoint", &bytes)? {
                                return Ok(());
                            }
                        }
                    }
                    Data::Summary(s) => {
                        extras.push((
                            "otel.metric.kind".to_string(),
                            JsonbValue::String(Cow::Borrowed("summary")),
                        ));
                        for dp in &s.data_points {
                            let bytes = leaf_jsonb(
                                "otlp.SummaryDataPoint",
                                dp,
                                &dp.attributes,
                                resource,
                                scope,
                                &scope_metrics.schema_url,
                                &extras,
                            )?;
                            if !visitor.visit("otlp.SummaryDataPoint", &bytes)? {
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
