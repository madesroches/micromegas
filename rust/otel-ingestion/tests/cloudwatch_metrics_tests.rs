//! Unit tests for the CloudWatch Metric Streams resource rewrite.
//!
//! No database: pure shape assertions on `is_cloudwatch_metric_stream_resource` /
//! `metric_namespace` / `rewrite_cloudwatch_metric_streams`, plus round-trips through
//! `split_metrics` / `ProcessFromResource::build` to prove the rewritten request produces
//! the per-namespace processes this plan exists to create.
//!
//! CloudWatch-shaped fixtures are kept local to this file (not the shared `fixtures.rs`),
//! following the `cloudwatch_logs_tests.rs` precedent of self-contained test files.

use micromegas_otel_ingestion::block::{ProcessFromResource, split_metrics};
use micromegas_otel_ingestion::cloudwatch_metrics::{
    UNKNOWN_NAMESPACE, is_cloudwatch_metric_stream_resource, metric_namespace,
    rewrite_cloudwatch_metric_streams,
};
use micromegas_otel_ingestion::identity::{IdentityContext, process_id_from_resource};
use micromegas_otel_ingestion::proto::{
    AnyValue, ExportMetricsServiceRequest, KeyValue, Metric, ResourceMetrics, ScopeMetrics,
    any_value::Value as AnyVal, metric,
};
use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::metrics::v1::{
    ExponentialHistogram, ExponentialHistogramDataPoint, Gauge, Histogram, HistogramDataPoint,
    NumberDataPoint, Sum, Summary, SummaryDataPoint,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

fn s_kv(k: &str, v: &str) -> KeyValue {
    KeyValue {
        key: k.into(),
        key_strindex: 0,
        value: Some(AnyValue {
            value: Some(AnyVal::StringValue(v.into())),
        }),
    }
}

fn scope(name: &str) -> InstrumentationScope {
    InstrumentationScope {
        name: name.into(),
        version: "0.0.0".into(),
        attributes: vec![],
        dropped_attributes_count: 0,
    }
}

/// Resource attrs for a CloudWatch Metric Stream: only `cloud.*` + `aws.exporter.arn`, no
/// `service.*`/`host.*`/`process.*` — the exact degenerate shape from the issue's dev data.
fn cw_resource_attrs(arn: &str) -> Vec<KeyValue> {
    vec![
        s_kv("cloud.account.id", "123456789012"),
        s_kv("cloud.provider", "aws"),
        s_kv("cloud.region", "us-east-1"),
        s_kv("aws.exporter.arn", arn),
    ]
}

/// A `Summary` metric — the shape CloudWatch Metric Streams' `opentelemetry1.0` output
/// actually produces — with one data point carrying `Namespace`/`MetricName`/`Dimensions`
/// datapoint attributes, mirroring the issue's dev data.
fn cw_summary_metric(namespace: &str, metric_name: &str, time: u64) -> Metric {
    let mut attributes = vec![s_kv("MetricName", metric_name)];
    if !namespace.is_empty() {
        attributes.insert(0, s_kv("Namespace", namespace));
    }
    attributes.push(s_kv(
        "Dimensions",
        r#"{"InstanceId":"i-0123456789abcdef0"}"#,
    ));
    Metric {
        name: format!("amazonaws.com/{namespace}/{metric_name}"),
        description: String::new(),
        unit: "Percent".into(),
        metadata: vec![],
        data: Some(metric::Data::Summary(Summary {
            data_points: vec![SummaryDataPoint {
                attributes,
                start_time_unix_nano: 0,
                time_unix_nano: time,
                count: 1,
                sum: 42.0,
                quantile_values: vec![],
                flags: 0,
            }],
        })),
    }
}

/// A metric with a `Summary` data point that has no `Namespace` attribute at all — models a
/// future data shape where CloudWatch omits it.
fn cw_summary_metric_no_namespace(metric_name: &str, time: u64) -> Metric {
    Metric {
        name: format!("amazonaws.com/{metric_name}"),
        description: String::new(),
        unit: "Percent".into(),
        metadata: vec![],
        data: Some(metric::Data::Summary(Summary {
            data_points: vec![SummaryDataPoint {
                attributes: vec![s_kv("MetricName", metric_name)],
                start_time_unix_nano: 0,
                time_unix_nano: time,
                count: 1,
                sum: 1.0,
                quantile_values: vec![],
                flags: 0,
            }],
        })),
    }
}

/// `Gauge`-shaped metric carrying a `Namespace` datapoint attribute — covers
/// `metric_namespace`'s other (non-`Summary`) match arms.
fn cw_gauge_metric(namespace: &str, metric_name: &str, time: u64) -> Metric {
    Metric {
        name: format!("amazonaws.com/{namespace}/{metric_name}"),
        description: String::new(),
        unit: "Count".into(),
        metadata: vec![],
        data: Some(metric::Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                attributes: vec![
                    s_kv("Namespace", namespace),
                    s_kv("MetricName", metric_name),
                ],
                start_time_unix_nano: 0,
                time_unix_nano: time,
                exemplars: vec![],
                flags: 0,
                value: Some(
                    opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(1),
                ),
            }],
        })),
    }
}

/// `Sum`-shaped metric carrying a `Namespace` datapoint attribute — covers
/// `metric_namespace`'s `Sum` match arm.
fn cw_sum_metric(namespace: &str, metric_name: &str, time: u64) -> Metric {
    Metric {
        name: format!("amazonaws.com/{namespace}/{metric_name}"),
        description: String::new(),
        unit: "Bytes".into(),
        metadata: vec![],
        data: Some(metric::Data::Sum(Sum {
            data_points: vec![NumberDataPoint {
                attributes: vec![
                    s_kv("Namespace", namespace),
                    s_kv("MetricName", metric_name),
                ],
                start_time_unix_nano: 0,
                time_unix_nano: time,
                exemplars: vec![],
                flags: 0,
                value: Some(
                    opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(1),
                ),
            }],
            aggregation_temporality: 0,
            is_monotonic: true,
        })),
    }
}

/// `Histogram`-shaped metric carrying a `Namespace` datapoint attribute — covers
/// `metric_namespace`'s `Histogram` match arm.
fn cw_histogram_metric(namespace: &str, metric_name: &str, time: u64) -> Metric {
    Metric {
        name: format!("amazonaws.com/{namespace}/{metric_name}"),
        description: String::new(),
        unit: "Milliseconds".into(),
        metadata: vec![],
        data: Some(metric::Data::Histogram(Histogram {
            data_points: vec![HistogramDataPoint {
                attributes: vec![
                    s_kv("Namespace", namespace),
                    s_kv("MetricName", metric_name),
                ],
                start_time_unix_nano: 0,
                time_unix_nano: time,
                count: 1,
                sum: Some(1.0),
                bucket_counts: vec![],
                explicit_bounds: vec![],
                exemplars: vec![],
                flags: 0,
                min: None,
                max: None,
            }],
            aggregation_temporality: 0,
        })),
    }
}

/// `ExponentialHistogram`-shaped metric carrying a `Namespace` datapoint attribute — covers
/// `metric_namespace`'s `ExponentialHistogram` match arm.
fn cw_exponential_histogram_metric(namespace: &str, metric_name: &str, time: u64) -> Metric {
    Metric {
        name: format!("amazonaws.com/{namespace}/{metric_name}"),
        description: String::new(),
        unit: "Milliseconds".into(),
        metadata: vec![],
        data: Some(metric::Data::ExponentialHistogram(ExponentialHistogram {
            data_points: vec![ExponentialHistogramDataPoint {
                attributes: vec![
                    s_kv("Namespace", namespace),
                    s_kv("MetricName", metric_name),
                ],
                start_time_unix_nano: 0,
                time_unix_nano: time,
                count: 1,
                sum: Some(1.0),
                scale: 0,
                zero_count: 0,
                positive: None,
                negative: None,
                flags: 0,
                exemplars: vec![],
                min: None,
                max: None,
                zero_threshold: 0.0,
            }],
            aggregation_temporality: 0,
        })),
    }
}

fn make_cw_request(
    resource_attrs: Vec<KeyValue>,
    scope_metrics: Vec<ScopeMetrics>,
) -> ExportMetricsServiceRequest {
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: resource_attrs,
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_metrics,
            schema_url: String::new(),
        }],
    }
}

fn one_scope(metrics: Vec<Metric>) -> Vec<ScopeMetrics> {
    vec![ScopeMetrics {
        scope: Some(scope("cloudwatch-metric-streams")),
        metrics,
        schema_url: String::new(),
    }]
}

fn find_attr<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a str> {
    attrs.iter().find(|kv| kv.key == key).and_then(|kv| {
        match kv.value.as_ref()?.value.as_ref()? {
            AnyVal::StringValue(s) => Some(s.as_str()),
            _ => None,
        }
    })
}

// ---------------------------------------------------------------------------
// is_cloudwatch_metric_stream_resource
// ---------------------------------------------------------------------------

#[test]
fn fingerprint_true_for_aws_exporter_arn_only_resource() {
    let attrs =
        cw_resource_attrs("arn:aws:firehose:us-east-1:123456789012:deliverystream/cw-metrics");
    assert!(is_cloudwatch_metric_stream_resource(&attrs));
}

#[test]
fn fingerprint_false_when_service_name_present() {
    let mut attrs = cw_resource_attrs("arn:aws:firehose:us-east-1:123456789012:deliverystream/x");
    attrs.push(s_kv("service.name", "my-service"));
    assert!(!is_cloudwatch_metric_stream_resource(&attrs));
}

#[test]
fn fingerprint_false_when_service_namespace_present() {
    let mut attrs = cw_resource_attrs("arn:aws:firehose:us-east-1:123456789012:deliverystream/x");
    attrs.push(s_kv("service.namespace", "my-ns"));
    assert!(!is_cloudwatch_metric_stream_resource(&attrs));
}

#[test]
fn fingerprint_false_when_resource_is_not_actually_degenerate() {
    // aws.exporter.arn alongside a real host.name/host.id/process.pid/service.instance.id
    // must not be rewritten — a non-degenerate resource that happens to carry
    // aws.exporter.arn is not this producer's shape.
    for (key, value) in [
        ("host.name", "real-host"),
        ("host.id", "real-host-id"),
        ("process.pid", "1234"),
        ("service.instance.id", "real-instance"),
    ] {
        let mut attrs =
            cw_resource_attrs("arn:aws:firehose:us-east-1:123456789012:deliverystream/x");
        attrs.push(s_kv(key, value));
        assert!(
            !is_cloudwatch_metric_stream_resource(&attrs),
            "must not match once {key} is set"
        );
    }
}

#[test]
fn fingerprint_true_when_service_name_and_namespace_present_but_empty_or_whitespace() {
    // A present-but-empty/whitespace-only value must not be treated as "has a service
    // name" — pins the attr_norm-over-is_none() design decision.
    for (name_value, ns_value) in [("", ""), ("   ", "  "), ("", "   ")] {
        let mut attrs =
            cw_resource_attrs("arn:aws:firehose:us-east-1:123456789012:deliverystream/x");
        attrs.push(s_kv("service.name", name_value));
        attrs.push(s_kv("service.namespace", ns_value));
        assert!(
            is_cloudwatch_metric_stream_resource(&attrs),
            "empty/whitespace service.name={name_value:?} service.namespace={ns_value:?} must still match"
        );
    }
}

#[test]
fn fingerprint_false_when_aws_exporter_arn_is_empty_or_whitespace() {
    // An empty ARN is not a real marker — must fall through untouched rather than match a
    // still-degenerate resource.
    for arn in ["", "   "] {
        let attrs = cw_resource_attrs(arn);
        assert!(
            !is_cloudwatch_metric_stream_resource(&attrs),
            "empty/whitespace aws.exporter.arn={arn:?} must not match"
        );
    }
}

// ---------------------------------------------------------------------------
// metric_namespace
// ---------------------------------------------------------------------------

#[test]
fn metric_namespace_extracts_from_summary_first_data_point() {
    let metric = cw_summary_metric("AWS/RDS", "CPUUtilization", 1_000);
    assert_eq!(metric_namespace(&metric).as_deref(), Some("AWS/RDS"));
}

#[test]
fn metric_namespace_extracts_from_gauge() {
    let metric = cw_gauge_metric("AWS/ECS", "RunningTaskCount", 1_000);
    assert_eq!(metric_namespace(&metric).as_deref(), Some("AWS/ECS"));
}

#[test]
fn metric_namespace_extracts_from_sum() {
    let metric = cw_sum_metric("AWS/S3", "BucketSizeBytes", 1_000);
    assert_eq!(metric_namespace(&metric).as_deref(), Some("AWS/S3"));
}

#[test]
fn metric_namespace_extracts_from_histogram() {
    let metric = cw_histogram_metric("AWS/Lambda", "Duration", 1_000);
    assert_eq!(metric_namespace(&metric).as_deref(), Some("AWS/Lambda"));
}

#[test]
fn metric_namespace_extracts_from_exponential_histogram() {
    let metric = cw_exponential_histogram_metric("AWS/ApiGateway", "Latency", 1_000);
    assert_eq!(metric_namespace(&metric).as_deref(), Some("AWS/ApiGateway"));
}

#[test]
fn metric_namespace_none_when_no_data_points() {
    let metric = Metric {
        name: "amazonaws.com/AWS/RDS/Empty".into(),
        description: String::new(),
        unit: String::new(),
        metadata: vec![],
        data: Some(metric::Data::Summary(Summary {
            data_points: vec![],
        })),
    };
    assert_eq!(metric_namespace(&metric), None);
}

#[test]
fn metric_namespace_none_when_attribute_absent() {
    let metric = cw_summary_metric_no_namespace("SomeMetric", 1_000);
    assert_eq!(metric_namespace(&metric), None);
}

#[test]
fn metric_namespace_none_when_value_empty_or_whitespace_only() {
    for ns in ["", "   "] {
        let metric = cw_summary_metric(ns, "SomeMetric", 1_000);
        assert_eq!(metric_namespace(&metric), None, "namespace={ns:?}");
    }
}

// ---------------------------------------------------------------------------
// rewrite_cloudwatch_metric_streams
// ---------------------------------------------------------------------------

#[test]
fn rewrite_partitions_two_namespaces_and_falls_back_for_missing_namespace() {
    let arn = "arn:aws:firehose:us-east-1:123456789012:deliverystream/cw-metrics";
    let req = make_cw_request(
        cw_resource_attrs(arn),
        one_scope(vec![
            cw_summary_metric("AWS/RDS", "CPUUtilization", 1_000),
            cw_summary_metric("AWS/ECS", "MemoryUtilization", 1_000),
            cw_summary_metric_no_namespace("MysteryMetric", 1_000),
        ]),
    );

    let rewritten = rewrite_cloudwatch_metric_streams(req);
    assert_eq!(rewritten.resource_metrics.len(), 3);

    let mut by_service_name = std::collections::BTreeMap::new();
    for rm in &rewritten.resource_metrics {
        let attrs = &rm.resource.as_ref().expect("resource").attributes;
        let service_name = find_attr(attrs, "service.name").expect("service.name set");
        let instance_id = find_attr(attrs, "service.instance.id").expect("service.instance.id set");
        assert_eq!(instance_id, arn);
        let metric_names: Vec<_> = rm.scope_metrics[0]
            .metrics
            .iter()
            .map(|m| m.name.clone())
            .collect();
        by_service_name.insert(service_name.to_string(), metric_names);
    }

    assert_eq!(
        by_service_name.get("AWS/RDS").expect("AWS/RDS bucket"),
        &vec!["amazonaws.com/AWS/RDS/CPUUtilization".to_string()]
    );
    assert_eq!(
        by_service_name.get("AWS/ECS").expect("AWS/ECS bucket"),
        &vec!["amazonaws.com/AWS/ECS/MemoryUtilization".to_string()]
    );
    let fallback = by_service_name
        .get(UNKNOWN_NAMESPACE)
        .expect("AWS/Unknown fallback bucket");
    assert_eq!(fallback, &vec!["amazonaws.com/MysteryMetric".to_string()]);

    // exe is never empty on this route, including the fallback bucket.
    for rm in &rewritten.resource_metrics {
        let attrs = &rm.resource.as_ref().expect("resource").attributes;
        let proc = ProcessFromResource::build(attrs, chrono::Utc::now());
        assert!(!proc.exe.is_empty(), "exe must never be empty: {attrs:?}");
    }
}

#[test]
fn rewrite_clears_whitespace_only_service_namespace_so_exe_is_exactly_the_namespace() {
    let arn = "arn:aws:firehose:us-east-1:123456789012:deliverystream/cw-metrics";
    let mut resource_attrs = cw_resource_attrs(arn);
    // A present-but-whitespace-only service.namespace satisfies the fingerprint's emptiness
    // gate (attr_norm trims) but is not itself empty for ProcessFromResource::build's raw
    // attr_to_string read.
    resource_attrs.push(s_kv("service.namespace", "  "));
    assert!(is_cloudwatch_metric_stream_resource(&resource_attrs));

    let req = make_cw_request(
        resource_attrs,
        one_scope(vec![cw_summary_metric("AWS/RDS", "CPUUtilization", 1_000)]),
    );
    let rewritten = rewrite_cloudwatch_metric_streams(req);
    assert_eq!(rewritten.resource_metrics.len(), 1);
    let attrs = &rewritten.resource_metrics[0]
        .resource
        .as_ref()
        .expect("resource")
        .attributes;
    assert_eq!(find_attr(attrs, "service.namespace"), Some(""));

    let proc = ProcessFromResource::build(attrs, chrono::Utc::now());
    assert_eq!(proc.exe, "AWS/RDS", "exe must not be \"  /AWS/RDS\"");
}

#[test]
fn rewrite_passes_through_non_cloudwatch_resource_unchanged() {
    // Already has service.name — not this producer's shape.
    let req1 = make_cw_request(
        vec![s_kv("service.name", "regular-otlp-service")],
        one_scope(vec![cw_summary_metric("AWS/RDS", "CPUUtilization", 1_000)]),
    );
    let rewritten1 = rewrite_cloudwatch_metric_streams(req1.clone());
    assert_eq!(rewritten1, req1);

    // No aws.exporter.arn at all.
    let req2 = make_cw_request(
        vec![s_kv("host.name", "some-host")],
        one_scope(vec![cw_summary_metric("AWS/RDS", "CPUUtilization", 1_000)]),
    );
    let rewritten2 = rewrite_cloudwatch_metric_streams(req2.clone());
    assert_eq!(rewritten2, req2);
}

#[test]
fn rewrite_merges_same_namespace_across_two_scope_metrics_into_one_resource_metrics() {
    let arn = "arn:aws:firehose:us-east-1:123456789012:deliverystream/cw-metrics";
    let req = make_cw_request(
        cw_resource_attrs(arn),
        vec![
            ScopeMetrics {
                scope: Some(scope("scope-a")),
                metrics: vec![cw_summary_metric("AWS/RDS", "CPUUtilization", 1_000)],
                schema_url: String::new(),
            },
            ScopeMetrics {
                scope: Some(scope("scope-b")),
                metrics: vec![cw_summary_metric("AWS/RDS", "FreeStorageSpace", 2_000)],
                schema_url: String::new(),
            },
        ],
    );

    let rewritten = rewrite_cloudwatch_metric_streams(req);
    // One namespace present → exactly one ResourceMetrics, not two.
    assert_eq!(rewritten.resource_metrics.len(), 1);
    let rm = &rewritten.resource_metrics[0];
    assert_eq!(rm.scope_metrics.len(), 2, "both original scopes preserved");
    let all_metric_names: Vec<_> = rm
        .scope_metrics
        .iter()
        .flat_map(|s| s.metrics.iter().map(|m| m.name.clone()))
        .collect();
    assert_eq!(
        all_metric_names,
        vec![
            "amazonaws.com/AWS/RDS/CPUUtilization".to_string(),
            "amazonaws.com/AWS/RDS/FreeStorageSpace".to_string(),
        ]
    );

    let blocks = split_metrics(rewritten, IdentityContext::default()).expect("split_metrics");
    assert_eq!(
        blocks.len(),
        1,
        "one PreparedBlock for the merged namespace"
    );
}

#[test]
fn rewrite_merges_case_variant_namespaces_into_one_resource_metrics_with_one_exe() {
    // "MyApp/Prod" and "myapp/prod" must collapse onto the same bucket: process_id derivation
    // (identity::attr_norm) lower-cases service.name, so two case-preserving buckets would
    // otherwise produce two ResourceMetrics/exe values sharing one process_id.
    let arn = "arn:aws:firehose:us-east-1:123456789012:deliverystream/cw-metrics";
    let req = make_cw_request(
        cw_resource_attrs(arn),
        one_scope(vec![
            cw_summary_metric("MyApp/Prod", "CPUUtilization", 1_000),
            cw_summary_metric("myapp/prod", "MemoryUtilization", 2_000),
        ]),
    );

    let rewritten = rewrite_cloudwatch_metric_streams(req);
    assert_eq!(
        rewritten.resource_metrics.len(),
        1,
        "case variants of the same namespace must merge into one ResourceMetrics"
    );

    let attrs = &rewritten.resource_metrics[0]
        .resource
        .as_ref()
        .expect("resource")
        .attributes;
    // First-seen raw (case-preserving) namespace string wins as service.name/exe.
    assert_eq!(find_attr(attrs, "service.name"), Some("MyApp/Prod"));

    let all_metric_names: Vec<_> = rewritten.resource_metrics[0]
        .scope_metrics
        .iter()
        .flat_map(|s| s.metrics.iter().map(|m| m.name.clone()))
        .collect();
    assert_eq!(
        all_metric_names,
        vec![
            "amazonaws.com/MyApp/Prod/CPUUtilization".to_string(),
            "amazonaws.com/myapp/prod/MemoryUtilization".to_string(),
        ],
        "both metrics land under the merged bucket"
    );

    let blocks = split_metrics(rewritten, IdentityContext::default()).expect("split_metrics");
    assert_eq!(
        blocks.len(),
        1,
        "one PreparedBlock (one process_id) for the merged case-variant namespace"
    );
}

#[test]
fn full_pipeline_yields_one_block_per_namespace_with_distinct_process_ids_and_expected_exe() {
    let arn = "arn:aws:firehose:us-east-1:123456789012:deliverystream/cw-metrics";
    let req = make_cw_request(
        cw_resource_attrs(arn),
        one_scope(vec![
            cw_summary_metric("AWS/RDS", "CPUUtilization", 1_000),
            cw_summary_metric("AWS/ECS", "MemoryUtilization", 2_000),
        ]),
    );
    let rewritten = rewrite_cloudwatch_metric_streams(req);
    let blocks = split_metrics(rewritten, IdentityContext::default()).expect("split_metrics");
    assert_eq!(blocks.len(), 2);
    assert_ne!(blocks[0].process_id, blocks[1].process_id);

    let mut exes: Vec<_> = blocks
        .iter()
        .map(|b| ProcessFromResource::build(&b.resource_attrs, chrono::Utc::now()).exe)
        .collect();
    exes.sort();
    assert_eq!(exes, vec!["AWS/ECS".to_string(), "AWS/RDS".to_string()]);
}

#[test]
fn same_arn_and_namespace_yields_identical_process_id_across_requests_with_different_data() {
    let arn = "arn:aws:firehose:us-east-1:123456789012:deliverystream/cw-metrics";
    let req1 = make_cw_request(
        cw_resource_attrs(arn),
        one_scope(vec![cw_summary_metric("AWS/RDS", "CPUUtilization", 1_000)]),
    );
    let req2 = make_cw_request(
        cw_resource_attrs(arn),
        one_scope(vec![cw_summary_metric(
            "AWS/RDS",
            "FreeStorageSpace",
            9_999,
        )]),
    );

    let blocks1 = split_metrics(
        rewrite_cloudwatch_metric_streams(req1),
        IdentityContext::default(),
    )
    .expect("split_metrics");
    let blocks2 = split_metrics(
        rewrite_cloudwatch_metric_streams(req2),
        IdentityContext::default(),
    )
    .expect("split_metrics");
    assert_eq!(blocks1.len(), 1);
    assert_eq!(blocks2.len(), 1);
    assert_eq!(
        blocks1[0].process_id, blocks2[0].process_id,
        "same arn + same namespace must be idempotent across records/retries"
    );

    let pid = process_id_from_resource(
        Some(&Resource {
            attributes: blocks1[0].resource_attrs.clone(),
            dropped_attributes_count: 0,
            entity_refs: vec![],
        }),
        IdentityContext::default(),
    );
    assert_eq!(blocks1[0].process_id, pid);
}

#[test]
fn different_arn_same_namespace_yields_distinct_process_ids() {
    let req1 = make_cw_request(
        cw_resource_attrs("arn:aws:firehose:us-east-1:111111111111:deliverystream/a"),
        one_scope(vec![cw_summary_metric("AWS/RDS", "CPUUtilization", 1_000)]),
    );
    let req2 = make_cw_request(
        cw_resource_attrs("arn:aws:firehose:us-west-2:222222222222:deliverystream/b"),
        one_scope(vec![cw_summary_metric("AWS/RDS", "CPUUtilization", 1_000)]),
    );

    let blocks1 = split_metrics(
        rewrite_cloudwatch_metric_streams(req1),
        IdentityContext::default(),
    )
    .expect("split_metrics");
    let blocks2 = split_metrics(
        rewrite_cloudwatch_metric_streams(req2),
        IdentityContext::default(),
    )
    .expect("split_metrics");
    assert_ne!(
        blocks1[0].process_id, blocks2[0].process_id,
        "distinct accounts/regions (distinct ARNs) must not collapse onto one process_id"
    );
}

#[test]
fn rewriting_same_input_twice_is_deterministic_byte_for_byte() {
    let arn = "arn:aws:firehose:us-east-1:123456789012:deliverystream/cw-metrics";
    let req = make_cw_request(
        cw_resource_attrs(arn),
        one_scope(vec![
            cw_summary_metric("AWS/RDS", "CPUUtilization", 1_000),
            cw_summary_metric("AWS/ECS", "MemoryUtilization", 2_000),
            cw_summary_metric_no_namespace("MysteryMetric", 3_000),
        ]),
    );

    let rewritten1 = rewrite_cloudwatch_metric_streams(req.clone());
    let rewritten2 = rewrite_cloudwatch_metric_streams(req);
    assert_eq!(rewritten1.resource_metrics.len(), 3);
    assert_eq!(rewritten1, rewritten2, "rewrite must be deterministic");

    // block_id is derived from rm.encode_to_vec(); byte-identical structs must therefore
    // encode identically.
    for (rm1, rm2) in rewritten1
        .resource_metrics
        .iter()
        .zip(rewritten2.resource_metrics.iter())
    {
        assert_eq!(rm1.encode_to_vec(), rm2.encode_to_vec());
    }
}
