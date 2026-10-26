//! Tests for extracting the claimed resource identity from a rejected OTLP body prefix.

use chrono::Utc;
use micromegas_otel_ingestion::Encoding;
use micromegas_otel_ingestion::block::ProcessFromResource;
use micromegas_otel_ingestion::proto::any_value::Value as AvValue;
use micromegas_otel_ingestion::proto::{
    AnyValue, ExportLogsServiceRequest, ExportMetricsServiceRequest, ExportTraceServiceRequest,
    KeyValue, KeyValueList, Resource, ResourceLogs, ResourceMetrics, ResourceSpans,
};
use micromegas_otel_ingestion::rejected::{ClaimedResource, claimed_resource_from_prefix};
use prost::Message;

fn value_kv(key: &str, value: AvValue) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        key_strindex: 0,
        value: Some(AnyValue { value: Some(value) }),
    }
}

fn kv(key: &str, value: &str) -> KeyValue {
    value_kv(key, AvValue::StringValue(value.to_string()))
}

fn resource(attrs: Vec<KeyValue>) -> Resource {
    Resource {
        attributes: attrs,
        ..Default::default()
    }
}

fn logs_request(attrs: Vec<KeyValue>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource(attrs)),
            ..Default::default()
        }],
    }
}

fn full_attrs() -> Vec<KeyValue> {
    vec![
        kv("service.namespace", "shop"),
        kv("service.name", "checkout"),
        kv("host.name", "node-7"),
        kv("telemetry.sdk.language", "python"),
        kv("telemetry.sdk.name", "opentelemetry"),
        kv("telemetry.sdk.version", "1.2.3"),
        kv("service.version", "9.9"),
        kv("service.instance.id", "inst-1"),
        kv("deployment.environment.name", "prod"),
        kv("k8s.namespace.name", "ns"),
        kv("k8s.pod.name", "pod-1"),
        kv("k8s.deployment.name", "dep"),
        kv("k8s.cluster.name", "cluster"),
        kv("container.name", "ctr"),
        kv("cloud.region", "eu-west-1"),
        value_kv("process.pid", AvValue::IntValue(4242)),
        kv("process.runtime.name", "CPython"),
        kv("process.runtime.version", "3.12"),
        kv("process.owner", "alice"),
        kv("process.command_line", "--token=secret"),
        kv("custom.secret", "hunter2"),
    ]
}

fn expected_full() -> Vec<(&'static str, &'static str)> {
    vec![
        ("claimed_exe", "shop/checkout"),
        ("claimed_computer", "node-7"),
        ("claimed_sdk_language", "python"),
        ("claimed_sdk_name", "opentelemetry"),
        ("claimed_sdk_version", "1.2.3"),
        ("claimed_service_version", "9.9"),
        ("claimed_service_instance_id", "inst-1"),
        ("claimed_deployment_environment", "prod"),
        ("claimed_k8s_namespace", "ns"),
        ("claimed_k8s_pod", "pod-1"),
        ("claimed_k8s_deployment", "dep"),
        ("claimed_k8s_cluster", "cluster"),
        ("claimed_container", "ctr"),
        ("claimed_cloud_region", "eu-west-1"),
        ("claimed_pid", "4242"),
        ("claimed_runtime", "CPython 3.12"),
        ("claimed_username", "alice"),
    ]
}

fn assert_full(claimed: &ClaimedResource) {
    for (name, value) in expected_full() {
        assert_eq!(claimed.get(name), value, "{name}");
    }
    assert_eq!(claimed.properties().len(), expected_full().len());
    for (_, value) in claimed.properties() {
        assert!(!value.contains("secret") && !value.contains("hunter2"));
    }
}

fn proto_bytes(attrs: Vec<KeyValue>) -> Vec<u8> {
    logs_request(attrs).encode_to_vec()
}

fn json_bytes(attrs: Vec<KeyValue>) -> Vec<u8> {
    serde_json::to_vec(&logs_request(attrs)).expect("encode json")
}

fn extract(bytes: &[u8], encoding: Encoding) -> Option<ClaimedResource> {
    claimed_resource_from_prefix(bytes, encoding)
}

#[test]
fn protobuf_full_matrix() {
    let claimed = extract(&proto_bytes(full_attrs()), Encoding::Protobuf).expect("claimed");
    assert_full(&claimed);
}

#[test]
fn json_full_matrix() {
    let claimed = extract(&json_bytes(full_attrs()), Encoding::Json).expect("claimed");
    assert_full(&claimed);
}

#[test]
fn metrics_and_traces_layouts() {
    let metrics = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(resource(vec![kv("service.name", "m")])),
            ..Default::default()
        }],
    };
    let traces = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(resource(vec![kv("service.name", "t")])),
            ..Default::default()
        }],
    };
    let m = extract(&metrics.encode_to_vec(), Encoding::Protobuf).expect("metrics");
    assert_eq!(m.get("claimed_exe"), "m");
    let t = extract(&traces.encode_to_vec(), Encoding::Protobuf).expect("traces");
    assert_eq!(t.get("claimed_exe"), "t");
    let mj = serde_json::to_vec(&metrics).expect("json");
    assert_eq!(
        extract(&mj, Encoding::Json)
            .expect("metrics json")
            .get("claimed_exe"),
        "m"
    );
    let tj = serde_json::to_vec(&traces).expect("json");
    assert_eq!(
        extract(&tj, Encoding::Json)
            .expect("traces json")
            .get("claimed_exe"),
        "t"
    );
}

#[test]
fn exe_matches_process_from_resource() {
    for attrs in [
        vec![kv("service.namespace", "ns"), kv("service.name", "svc")],
        vec![kv("service.name", "svc")],
    ] {
        let expected = ProcessFromResource::build(&attrs, Utc::now()).exe;
        let claimed = extract(&proto_bytes(attrs), Encoding::Protobuf).expect("claimed");
        assert_eq!(claimed.get("claimed_exe"), expected);
    }
}

#[test]
fn absent_host_name_is_not_a_property() {
    let claimed = extract(
        &proto_bytes(vec![kv("service.name", "svc")]),
        Encoding::Protobuf,
    )
    .expect("c");
    assert!(
        claimed
            .properties()
            .iter()
            .all(|(n, _)| *n != "claimed_computer")
    );
    assert_eq!(claimed.get("claimed_computer"), "");
}

#[test]
fn deployment_environment_falls_back_to_legacy_name() {
    let claimed = extract(
        &proto_bytes(vec![kv("deployment.environment", "staging")]),
        Encoding::Protobuf,
    )
    .expect("claimed");
    assert_eq!(claimed.get("claimed_deployment_environment"), "staging");
}

#[test]
fn only_scalar_values_are_kept() {
    let kvlist = value_kv(
        "service.name",
        AvValue::KvlistValue(KeyValueList {
            values: vec![kv("a", "b")],
        }),
    );
    let attrs = vec![kvlist, kv("host.name", "h")];
    for (bytes, enc) in [
        (proto_bytes(attrs.clone()), Encoding::Protobuf),
        (json_bytes(attrs), Encoding::Json),
    ] {
        let claimed = extract(&bytes, enc).expect("claimed");
        assert_eq!(claimed.get("claimed_exe"), "");
        assert_eq!(claimed.get("claimed_computer"), "h");
    }
}

#[test]
fn non_allowlisted_attributes_never_appear() {
    let attrs = vec![
        kv("process.command_line", "--token=abc"),
        kv("process.command_args", "x"),
        kv("process.environment_variable.KEY", "v"),
        kv("host.name", "h"),
    ];
    for (bytes, enc) in [
        (proto_bytes(attrs.clone()), Encoding::Protobuf),
        (json_bytes(attrs), Encoding::Json),
    ] {
        let claimed = extract(&bytes, enc).expect("claimed");
        assert_eq!(claimed.properties().len(), 1);
    }
}

#[test]
fn control_characters_survive_extraction() {
    let claimed = extract(
        &proto_bytes(vec![kv("service.name", "a\nb\u{1b}")]),
        Encoding::Protobuf,
    )
    .expect("claimed");
    assert_eq!(claimed.get("claimed_exe"), "a\nb\u{1b}");
}

#[test]
fn garbage_and_empty_input_yield_none() {
    for enc in [Encoding::Protobuf, Encoding::Json] {
        assert!(extract(&[], enc).is_none());
        assert!(extract(b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff", enc).is_none());
        assert!(extract(b"not an otlp message", enc).is_none());
    }
}

#[test]
fn json_keys_are_case_sensitive() {
    let body = br#"{"ResourceLogs":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"x"}}]}}]}"#;
    assert!(extract(body, Encoding::Json).is_none());
}

#[test]
fn truncation_at_every_offset_is_prefix_consistent() {
    let attrs = vec![
        kv("service.namespace", "shop"),
        kv("service.name", "checkout"),
        kv("host.name", "node-7"),
        kv("telemetry.sdk.language", "python"),
    ];
    for (bytes, enc) in [
        (proto_bytes(attrs.clone()), Encoding::Protobuf),
        (json_bytes(attrs), Encoding::Json),
    ] {
        let full = extract(&bytes, enc).expect("full");
        for cut in 0..=bytes.len() {
            let Some(partial) = extract(&bytes[..cut], enc) else {
                continue;
            };
            for (name, value) in partial.properties() {
                let full_value = full.get(name);
                if *name == "claimed_exe" {
                    assert!(full_value.starts_with(value.as_str()), "cut {cut}");
                } else {
                    assert_eq!(full_value, value, "cut {cut} {name}");
                }
            }
        }
    }
}
