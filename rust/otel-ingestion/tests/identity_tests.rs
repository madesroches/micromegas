//! Tests for OTel resource → micromegas identity synthesis.

use micromegas_otel_ingestion::identity::{
    SignalKey, block_id_from_payload, is_degenerate_resource, process_id_from_resource,
    process_start_string, stream_id_from_process_signal,
};
use micromegas_otel_ingestion::proto::any_value::Value as AvValue;
use micromegas_otel_ingestion::proto::{AnyValue, KeyValue, Resource};
use uuid::Uuid;

fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(AvValue::StringValue(value.to_string())),
        }),
    }
}

fn resource_with(pairs: &[(&str, &str)]) -> Resource {
    Resource {
        attributes: pairs.iter().map(|(k, v)| kv(k, v)).collect(),
        dropped_attributes_count: 0,
        entity_refs: vec![],
    }
}

#[test]
fn process_id_is_stable() {
    let r1 = resource_with(&[
        ("service.name", "claude-code"),
        ("host.name", "macbook-mad"),
        ("process.pid", "1234"),
        ("process.start_time", "2026-04-01T12:00:00Z"),
    ]);
    let r2 = resource_with(&[
        // permuted order
        ("process.start_time", "2026-04-01T12:00:00Z"),
        ("host.name", "macbook-mad"),
        ("process.pid", "1234"),
        ("service.name", "claude-code"),
    ]);
    assert_eq!(
        process_id_from_resource(Some(&r1)),
        process_id_from_resource(Some(&r2))
    );
}

#[test]
fn process_id_differs_per_pid() {
    let a = resource_with(&[("service.name", "claude-code"), ("process.pid", "1")]);
    let b = resource_with(&[("service.name", "claude-code"), ("process.pid", "2")]);
    assert_ne!(
        process_id_from_resource(Some(&a)),
        process_id_from_resource(Some(&b))
    );
}

#[test]
fn process_id_normalizes_host_case() {
    let a = resource_with(&[("host.name", "Foo"), ("service.name", "svc")]);
    let b = resource_with(&[("host.name", "FOO"), ("service.name", "svc")]);
    assert_eq!(
        process_id_from_resource(Some(&a)),
        process_id_from_resource(Some(&b))
    );
}

#[test]
fn process_start_resolves_either_attribute() {
    let canonical = resource_with(&[("process.creation.time", "abc")]);
    let legacy = resource_with(&[("process.start_time", "abc")]);
    assert_eq!(process_start_string(&canonical.attributes), "abc");
    assert_eq!(process_start_string(&legacy.attributes), "abc");
}

#[test]
fn process_start_prefers_creation_time_when_both_present() {
    let both = resource_with(&[
        ("process.creation.time", "canonical"),
        ("process.start_time", "legacy"),
    ]);
    assert_eq!(process_start_string(&both.attributes), "canonical");
}

#[test]
fn stream_id_differs_per_signal() {
    let p = Uuid::new_v4();
    assert_ne!(
        stream_id_from_process_signal(p, SignalKey::Logs),
        stream_id_from_process_signal(p, SignalKey::Metrics)
    );
    assert_ne!(
        stream_id_from_process_signal(p, SignalKey::Logs),
        stream_id_from_process_signal(p, SignalKey::Traces)
    );
}

#[test]
fn block_id_is_content_addressed() {
    let id1 = block_id_from_payload(b"hello");
    let id2 = block_id_from_payload(b"hello");
    let id3 = block_id_from_payload(b"world");
    assert_eq!(id1, id2);
    assert_ne!(id1, id3);
}

#[test]
fn degenerate_resource_detected() {
    let empty = resource_with(&[("service.name", "svc")]);
    assert!(is_degenerate_resource(&empty.attributes));
    let with_host = resource_with(&[("service.name", "svc"), ("host.name", "h")]);
    assert!(!is_degenerate_resource(&with_host.attributes));
}
