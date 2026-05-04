//! Tests for per-resource block splitting and `ProcessFromResource` construction.

use chrono::Utc;
use micromegas_otel_ingestion::block::{ProcessFromResource, split_logs};
use micromegas_otel_ingestion::proto::{
    AnyValue, ExportLogsServiceRequest, KeyValue, LogRecord, Resource, ResourceLogs, ScopeLogs,
    any_value::Value as AvValue,
};

fn s_kv(k: &str, v: &str) -> KeyValue {
    KeyValue {
        key: k.into(),
        value: Some(AnyValue {
            value: Some(AvValue::StringValue(v.into())),
        }),
    }
}

fn empty_log_record(time_unix_nano: u64) -> LogRecord {
    LogRecord {
        time_unix_nano,
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
    }
}

fn resource_logs(service_name: &str, records: Vec<LogRecord>) -> ResourceLogs {
    ResourceLogs {
        resource: Some(Resource {
            attributes: vec![s_kv("service.name", service_name)],
            dropped_attributes_count: 0,
            entity_refs: vec![],
        }),
        scope_logs: vec![ScopeLogs {
            scope: None,
            log_records: records,
            schema_url: String::new(),
        }],
        schema_url: String::new(),
    }
}

#[test]
fn split_logs_one_block_per_resource() {
    let req = ExportLogsServiceRequest {
        resource_logs: vec![
            resource_logs("svc-a", vec![empty_log_record(1_700_000_000_000_000_000)]),
            resource_logs("svc-b", vec![empty_log_record(1_700_000_001_000_000_000)]),
        ],
    };
    let blocks = split_logs(req).unwrap();
    assert_eq!(blocks.len(), 2);
    assert_ne!(blocks[0].process_id, blocks[1].process_id);
    assert_eq!(blocks[0].nb_records, 1);
}

#[test]
fn split_logs_skips_empty_resource() {
    let req = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
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
    let mk = |svc: &str| resource_logs(svc, vec![empty_log_record(1)]);
    let a = split_logs(ExportLogsServiceRequest {
        resource_logs: vec![mk("a")],
    })
    .unwrap();
    let b = split_logs(ExportLogsServiceRequest {
        resource_logs: vec![mk("b")],
    })
    .unwrap();
    assert_ne!(a[0].block.block_id, b[0].block.block_id);
}

#[test]
fn process_field_truncation_caps_at_255_chars_without_splitting_codepoints() {
    // 300 ASCII chars → truncated to 255.
    let long_ascii = "a".repeat(300);
    let svc = s_kv("os.description", &long_ascii);
    let p = ProcessFromResource::build(&[svc], Utc::now());
    assert_eq!(p.distro.chars().count(), 255);

    // 300 multi-byte chars (3 bytes each) — never panic on a codepoint boundary
    // and never produce more than 255 chars even though byte length differs.
    let long_emoji = "は".repeat(300);
    let svc = s_kv("host.name", &long_emoji);
    let p = ProcessFromResource::build(&[svc], Utc::now());
    assert_eq!(p.computer.chars().count(), 255);
}
