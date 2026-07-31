//! CloudWatch Metric Streams resource rewrite.
//!
//! Metrics delivered via `POST /ingestion/otlp/v1/metrics/firehose` from a CloudWatch Metric
//! Stream carry a `Resource` with no `service.*`/`host.*`/`process.*` identity at all — only
//! `cloud.account.id`, `cloud.provider`, `cloud.region`, and `aws.exporter.arn`. Every stream
//! from every AWS account/region therefore hashes to the same degenerate `process_id`
//! (`identity::is_degenerate_resource` already flags this shape, but nothing corrects it).
//!
//! This module detects that fingerprint and partitions each matching `ResourceMetrics` into
//! one synthetic `ResourceMetrics` per CloudWatch **namespace** (`AWS/RDS`, `AWS/ECS`,
//! `ECS/ContainerInsights`, `AWS/S3`, …), read off the `Namespace` attribute CloudWatch
//! attaches to every datapoint — giving `exe` a bounded, meaningful value while folding the
//! exporter ARN into `service.instance.id` so different accounts/regions never collide onto
//! the same process. Mirrors what `cloudwatch_logs.rs` does for the Logs Firehose route, but
//! unlike that route this producer's resource carries no per-stream identity of its own to
//! reuse — the identity has to be synthesized from datapoint attributes instead.
//!
//! Pure, unit-testable, no HTTP/framework dependency. Called from
//! `handler::ingest_firehose_metrics` only — the shared `/v1/metrics` OTLP endpoint is left
//! untouched (see the design plan for the rationale).

use crate::identity;
use crate::proto::{
    AnyValue, ExportMetricsServiceRequest, KeyValue, Metric, ResourceMetrics, ScopeMetrics,
    any_value,
};
use std::collections::BTreeMap;

/// `service.name` fallback for a metric with no usable `Namespace` datapoint attribute, so
/// `exe` is never empty on this route. Public (rather than private) so
/// `tests/cloudwatch_metrics_tests.rs` can assert against it directly, matching the
/// `cloudwatch_logs.rs` precedent (`cloudwatch_logs.rs:21-22`).
pub const UNKNOWN_NAMESPACE: &str = "AWS/Unknown";

/// True when `attrs` looks like a CloudWatch Metric Streams resource: a non-empty
/// `aws.exporter.arn` marker, no `service.name`/`service.namespace`, and otherwise degenerate
/// (`identity::is_degenerate_resource`).
///
/// All three conjuncts are gated on emptiness via `identity::attr_norm` (trim + lowercase),
/// not `Option::is_some()`/`is_none()`, to match the same-strength check
/// `is_degenerate_resource` already applies to its own fields — a present-but-empty
/// `StringValue("")` must not be treated as "has a value" for any of the three: an empty
/// `aws.exporter.arn` is not a real marker (and would otherwise still leave the resource
/// degenerate), and a present-but-empty `service.name`/`service.namespace` must not be
/// treated as "has a service name" either.
///
/// Public so `tests/cloudwatch_metrics_tests.rs` can call it directly.
pub fn is_cloudwatch_metric_stream_resource(attrs: &[KeyValue]) -> bool {
    !identity::attr_norm(attrs, "aws.exporter.arn").is_empty()
        && identity::attr_norm(attrs, "service.name").is_empty()
        && identity::attr_norm(attrs, "service.namespace").is_empty()
        && identity::is_degenerate_resource(attrs)
}

/// Reads the `Namespace` attribute off a `Metric`'s **first** data point, across whichever
/// OTel data type it holds (`Sum`/`Gauge`/`Histogram`/`ExponentialHistogram`/`Summary`).
///
/// Sampling only the first data point is sufficient because the namespace is constant per
/// `Metric` — CloudWatch encodes it directly in `Metric.name`
/// (`amazonaws.com/<Namespace>/<MetricName>`), so every data point under one `Metric` carries
/// the same `Namespace` value in practice.
///
/// Returns `None` — routing the metric to the `UNKNOWN_NAMESPACE` fallback bucket rather than
/// dropping it — when the metric has no first data point, no `Namespace` attribute, or an
/// empty/whitespace-only value. The trimmed value flows untouched into both the bucket key and
/// `service.name`, so it must match what `process_id_from_resource` will fold through
/// `attr_norm` (trim + lowercase) downstream — an untrimmed `" AWS/RDS"` and a trimmed
/// `"AWS/RDS"` would otherwise land in two different `BTreeMap` buckets (two blocks, two
/// conflicting `exe` values) while still hashing to the same `process_id`.
///
/// Public so `tests/cloudwatch_metrics_tests.rs` can call it directly.
pub fn metric_namespace(metric: &Metric) -> Option<String> {
    use crate::proto::metric::Data;
    let attrs: &[KeyValue] = match metric.data.as_ref()? {
        Data::Sum(s) => s.data_points.first()?.attributes.as_slice(),
        Data::Gauge(g) => g.data_points.first()?.attributes.as_slice(),
        Data::Histogram(h) => h.data_points.first()?.attributes.as_slice(),
        Data::ExponentialHistogram(h) => h.data_points.first()?.attributes.as_slice(),
        Data::Summary(s) => s.data_points.first()?.attributes.as_slice(),
    };
    identity::attr(attrs, "Namespace")
        .map(identity::attr_to_string)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Replace-if-present `KeyValue` setter: overwrites `key`'s value in place when already
/// present, pushing only when absent — never produces a duplicate key (unlike a plain
/// `push`), matching the shape of `cloudwatch_logs.rs`'s `kv` helper.
fn set_attr(attrs: &mut Vec<KeyValue>, key: &str, value: &str) {
    let new_value = Some(AnyValue {
        value: Some(any_value::Value::StringValue(value.to_string())),
    });
    if let Some(existing) = attrs.iter_mut().find(|kv| kv.key == key) {
        existing.value = new_value;
    } else {
        attrs.push(KeyValue {
            key: key.to_string(),
            key_strindex: 0,
            value: new_value,
        });
    }
}

/// Sets an existing entry's value to `""` in place when `key` is already present; a no-op
/// otherwise (never pushes) — used to clear `service.namespace` without adding a spurious
/// empty attribute to resources that never carried one.
fn clear_attr_if_present(attrs: &mut [KeyValue], key: &str) {
    if let Some(existing) = attrs.iter_mut().find(|kv| kv.key == key) {
        existing.value = Some(AnyValue {
            value: Some(any_value::Value::StringValue(String::new())),
        });
    }
}

/// Partitions one CloudWatch-shaped `ResourceMetrics` into one synthetic `ResourceMetrics` per
/// namespace found across its metrics (`metric_namespace`'s `None` result buckets under
/// `UNKNOWN_NAMESPACE`). A namespace appearing in more than one original `ScopeMetrics` still
/// ends up as a single output resource's `scope_metrics` list — `BTreeMap` for deterministic
/// iteration order, load-bearing for reproducible `block_id`s across repeated rewrites of the
/// same input.
///
/// Every other field on the synthetic `ResourceMetrics`/`Resource` is carried over from the
/// original unchanged — only `attributes` differs.
fn partition_resource_metrics(rm: ResourceMetrics) -> Vec<ResourceMetrics> {
    let original_attrs = rm
        .resource
        .as_ref()
        .map(|r| r.attributes.clone())
        .unwrap_or_default();
    let dropped_attributes_count = rm
        .resource
        .as_ref()
        .map(|r| r.dropped_attributes_count)
        .unwrap_or(0);
    let entity_refs = rm
        .resource
        .as_ref()
        .map(|r| r.entity_refs.clone())
        .unwrap_or_default();
    let schema_url = rm.schema_url.clone();

    let mut buckets: BTreeMap<Option<String>, Vec<ScopeMetrics>> = BTreeMap::new();
    for scope in rm.scope_metrics {
        let scope_scope = scope.scope.clone();
        let scope_schema_url = scope.schema_url.clone();
        let mut per_namespace: BTreeMap<Option<String>, Vec<Metric>> = BTreeMap::new();
        for metric in scope.metrics {
            let namespace = metric_namespace(&metric);
            per_namespace.entry(namespace).or_default().push(metric);
        }
        for (namespace, metrics) in per_namespace {
            buckets.entry(namespace).or_default().push(ScopeMetrics {
                scope: scope_scope.clone(),
                metrics,
                schema_url: scope_schema_url.clone(),
            });
        }
    }

    let arn = identity::attr(&original_attrs, "aws.exporter.arn")
        .map(identity::attr_to_string)
        .unwrap_or_default();

    buckets
        .into_iter()
        .map(|(namespace, scope_metrics)| {
            let mut resource_attrs = original_attrs.clone();
            // cloud.account.id, cloud.provider, cloud.region, aws.exporter.arn carried over
            // (via original_attrs.clone() above); only these three are added/changed:
            set_attr(&mut resource_attrs, "service.instance.id", &arn);
            set_attr(
                &mut resource_attrs,
                "service.name",
                namespace.as_deref().unwrap_or(UNKNOWN_NAMESPACE),
            );
            // So exe = service.name, not "{svc_ns}/{service.name}" — matters even though the
            // fingerprint already requires attr_norm(service.namespace) to be empty: attr_norm
            // trims+lowercases before comparing, so a present-but-whitespace-only value
            // satisfies the fingerprint's emptiness gate but is not itself empty, and
            // ProcessFromResource::build reads service.namespace with raw attr_to_string (no
            // trim).
            clear_attr_if_present(&mut resource_attrs, "service.namespace");
            ResourceMetrics {
                resource: Some(crate::proto::Resource {
                    attributes: resource_attrs,
                    dropped_attributes_count,
                    entity_refs: entity_refs.clone(),
                }),
                scope_metrics,
                schema_url: schema_url.clone(),
            }
        })
        .collect()
}

/// Rewrites every `ResourceMetrics` in `req` that matches
/// `is_cloudwatch_metric_stream_resource` into its per-namespace partition (see
/// `partition_resource_metrics`); non-matching entries pass through completely untouched —
/// this rewrite is purely additive for this one AWS-specific shape, not a change to the
/// shared OTLP/metrics path.
///
/// Public entry point, called from `handler::ingest_firehose_metrics` only (not the shared
/// `ingest_parsed_metrics`/`ingest_metrics` used by the plain `/v1/metrics` OTLP endpoint).
pub fn rewrite_cloudwatch_metric_streams(
    req: ExportMetricsServiceRequest,
) -> ExportMetricsServiceRequest {
    let mut out = Vec::with_capacity(req.resource_metrics.len());
    for rm in req.resource_metrics {
        let is_match = rm
            .resource
            .as_ref()
            .map(|r| is_cloudwatch_metric_stream_resource(&r.attributes))
            .unwrap_or(false);
        if is_match {
            out.extend(partition_resource_metrics(rm));
        } else {
            out.push(rm);
        }
    }
    ExportMetricsServiceRequest {
        resource_metrics: out,
    }
}
