//! Tests for OTel resource → micromegas identity synthesis.

use micromegas_otel_ingestion::identity::{
    SignalKey, attr_to_string, block_id_from_payload, is_degenerate_resource,
    process_id_from_resource, process_owner_string, process_start_string,
    stream_id_from_process_signal,
};
use micromegas_otel_ingestion::proto::any_value::Value as AvValue;
use micromegas_otel_ingestion::proto::{AnyValue, KeyValue, Resource};
use uuid::Uuid;

fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        key_strindex: 0,
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
fn process_id_differs_per_owner() {
    let a = resource_with(&[
        ("host.name", "h"),
        ("process.pid", "1"),
        ("process.owner", "alice"),
    ]);
    let b = resource_with(&[
        ("host.name", "h"),
        ("process.pid", "1"),
        ("process.owner", "bob"),
    ]);
    assert_ne!(
        process_id_from_resource(Some(&a)),
        process_id_from_resource(Some(&b))
    );
}

#[test]
fn process_id_owner_uses_user_name_fallback() {
    // `process.owner` and `user.name` resolve to the same owner string, so they must
    // produce the same process_id.
    let canonical = resource_with(&[("host.name", "h"), ("process.owner", "alice")]);
    let fallback = resource_with(&[("host.name", "h"), ("user.name", "alice")]);
    assert_eq!(
        process_id_from_resource(Some(&canonical)),
        process_id_from_resource(Some(&fallback))
    );
}

#[test]
fn process_owner_prefers_keys_in_priority_order() {
    // process.owner > process.user.name > process.real_user.name > user.name.
    let all = resource_with(&[
        ("process.owner", "owner"),
        ("process.user.name", "euser"),
        ("process.real_user.name", "ruser"),
        ("user.name", "generic"),
    ]);
    assert_eq!(process_owner_string(&all.attributes), "owner");

    let no_owner = resource_with(&[
        ("process.user.name", "euser"),
        ("process.real_user.name", "ruser"),
        ("user.name", "generic"),
    ]);
    assert_eq!(process_owner_string(&no_owner.attributes), "euser");

    let real_only = resource_with(&[
        ("process.real_user.name", "ruser"),
        ("user.name", "generic"),
    ]);
    assert_eq!(process_owner_string(&real_only.attributes), "ruser");

    let generic_only = resource_with(&[("user.name", "generic")]);
    assert_eq!(process_owner_string(&generic_only.attributes), "generic");

    let none = resource_with(&[("host.name", "h")]);
    assert_eq!(process_owner_string(&none.attributes), "");
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

#[test]
fn attr_to_string_ignores_profiling_strindex() {
    // Profiling-only string-table reference — no dictionary here, so it must render empty,
    // never the index stringified ("5").
    let v = AnyValue {
        value: Some(AvValue::StringValueStrindex(5)),
    };
    assert_eq!(attr_to_string(&v), "");
}

#[test]
fn strindex_value_on_identity_key_hashes_as_absent() {
    // `service.name` carried as a profiling-only string-table reference is meaningless for this
    // signal; it must hash identically to omitting `service.name` entirely. Guards against
    // contaminating the load-bearing process_id with a dictionary index.
    let strindex_service = KeyValue {
        key: "service.name".to_string(),
        key_strindex: 0,
        value: Some(AnyValue {
            value: Some(AvValue::StringValueStrindex(3)),
        }),
    };
    let with_strindex = Resource {
        attributes: vec![kv("host.name", "h"), strindex_service],
        dropped_attributes_count: 0,
        entity_refs: vec![],
    };
    let without = resource_with(&[("host.name", "h")]);
    assert_eq!(
        process_id_from_resource(Some(&with_strindex)),
        process_id_from_resource(Some(&without))
    );
}

#[test]
fn windows_and_wsl_differ() {
    // Motivating case: Windows + WSL sibling processes on the same machine sharing all base
    // V1 fields but differing in os.type must not collapse onto the same process_id.
    let windows = resource_with(&[
        ("host.id", "machine-1"),
        ("host.name", "my-machine"),
        ("service.name", "claude-code"),
        ("os.type", "windows"),
    ]);
    let wsl = resource_with(&[
        ("host.id", "machine-1"),
        ("host.name", "my-machine"),
        ("service.name", "claude-code"),
        ("os.type", "linux"),
    ]);
    assert_ne!(
        process_id_from_resource(Some(&windows)),
        process_id_from_resource(Some(&wsl))
    );
}

#[test]
fn process_id_is_stable_with_new_fields() {
    // Regression lock: a canonical resource with known values must always hash to this UUID.
    // Update this constant only if the formula is intentionally changed.
    let r = resource_with(&[
        ("host.name", "my-host"),
        ("service.name", "my-service"),
        ("os.type", "linux"),
        ("os.version", "5.15.0"),
        ("host.arch", "x86_64"),
        ("service.version", "2.3.1"),
        ("telemetry.sdk.name", "opentelemetry"),
        ("telemetry.sdk.language", "rust"),
        ("telemetry.sdk.version", "0.28.0"),
        ("process.runtime.name", "rustc"),
        ("process.runtime.version", "1.88.0"),
    ]);
    let id = process_id_from_resource(Some(&r));
    assert_eq!(id.to_string(), "92267645-021b-5d0f-960b-c74719552658");
}

#[test]
fn interned_key_is_ignored_in_identity() {
    // `service.name` provided only via `key_strindex` (empty `key`) is not found by `attr`, so
    // it hashes the same as a resource lacking `service.name`.
    let interned_key = KeyValue {
        key: String::new(),
        key_strindex: 4,
        value: Some(AnyValue {
            value: Some(AvValue::StringValue("svc".to_string())),
        }),
    };
    let with_interned = Resource {
        attributes: vec![kv("host.name", "h"), interned_key],
        dropped_attributes_count: 0,
        entity_refs: vec![],
    };
    let without = resource_with(&[("host.name", "h")]);
    assert_eq!(
        process_id_from_resource(Some(&with_interned)),
        process_id_from_resource(Some(&without))
    );
}
