//! Unit tests for the CloudWatch Logs subscription-filter decoder.
//!
//! No database: pure shape assertions on `decode_cloudwatch_logs_record` /
//! `build_export_logs_request`, plus a round-trip through `split_logs` to prove the
//! synthesized request produces the same blocks the OTLP logs pipeline already handles.

use flate2::Compression;
use flate2::write::GzEncoder;
use micromegas_otel_ingestion::block::split_logs;
use micromegas_otel_ingestion::cloudwatch_logs::{
    build_export_logs_request, decode_cloudwatch_logs_record,
};
use micromegas_otel_ingestion::error::OtelError;
use micromegas_otel_ingestion::identity::process_id_from_resource;
use std::io::Write;

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).expect("writing to gzip encoder");
    encoder.finish().expect("finishing gzip stream")
}

fn data_message(log_group: &str, log_stream: &str, owner: &str, events_json: &str) -> Vec<u8> {
    let json = format!(
        r#"{{"messageType":"DATA_MESSAGE","owner":"{owner}","logGroup":"{log_group}","logStream":"{log_stream}","subscriptionFilters":["my-filter"],"logEvents":[{events_json}]}}"#
    );
    gzip(json.as_bytes())
}

fn control_message() -> Vec<u8> {
    let json = r#"{"messageType":"CONTROL_MESSAGE","owner":"CloudwatchLogs","logGroup":"","logStream":"","subscriptionFilters":[],"logEvents":[{"id":"1","timestamp":1510109208016,"message":"CWL CONTROL MESSAGE: Checking health of destination Firehose."}]}"#;
    gzip(json.as_bytes())
}

#[test]
fn data_message_with_multiple_events_decodes() {
    let events = r#"{"id":"evt-1","timestamp":1510109208016,"message":"first line"},{"id":"evt-2","timestamp":1510109208100,"message":"second line"}"#;
    let raw = data_message(
        "/ecs/my-service",
        "ecs/my-service/abcd1234",
        "123456789012",
        events,
    );

    let msg = decode_cloudwatch_logs_record(&raw, 0)
        .expect("decode ok")
        .expect("Some for DATA_MESSAGE");
    assert_eq!(msg.log_group, "/ecs/my-service");
    assert_eq!(msg.log_stream, "ecs/my-service/abcd1234");
    assert_eq!(msg.owner, "123456789012");
    assert_eq!(msg.log_events.len(), 2);
    assert_eq!(msg.log_events[0].message, "first line");
    assert_eq!(msg.log_events[1].message, "second line");
}

#[test]
fn control_message_is_dropped_not_an_error() {
    let raw = control_message();
    let result = decode_cloudwatch_logs_record(&raw, 0).expect("decode ok");
    assert!(result.is_none(), "CONTROL_MESSAGE must be silently skipped");
}

#[test]
fn data_message_with_empty_events_is_dropped() {
    let json = r#"{"messageType":"DATA_MESSAGE","owner":"123456789012","logGroup":"/ecs/my-service","logStream":"stream-1","subscriptionFilters":["f"],"logEvents":[]}"#;
    let raw = gzip(json.as_bytes());
    let result = decode_cloudwatch_logs_record(&raw, 0).expect("decode ok");
    assert!(result.is_none(), "empty logEvents must be dropped");
}

#[test]
fn decompressed_size_over_cap_is_parse_error() {
    // Highly redundant data compresses to a tiny gzip stream but decompresses past the
    // 64 MiB cap, proving the cap is enforced on decompressed bytes, not compressed input size.
    const OVER_CAP: usize = 64 * 1024 * 1024 + 1024;
    let huge_message = "a".repeat(OVER_CAP);
    let json = format!(
        r#"{{"messageType":"DATA_MESSAGE","owner":"1","logGroup":"/g","logStream":"s","subscriptionFilters":["f"],"logEvents":[{{"id":"1","timestamp":1,"message":"{huge_message}"}}]}}"#
    );
    let raw = gzip(json.as_bytes());
    let err = decode_cloudwatch_logs_record(&raw, 0).expect_err("expected parse error");
    assert!(matches!(err, OtelError::Parse { .. }));
}

#[test]
fn malformed_gzip_is_parse_error() {
    let err =
        decode_cloudwatch_logs_record(b"not gzip at all", 0).expect_err("expected parse error");
    assert!(matches!(err, OtelError::Parse { .. }));
}

#[test]
fn valid_gzip_malformed_json_is_parse_error() {
    let raw = gzip(b"not json at all");
    let err = decode_cloudwatch_logs_record(&raw, 0).expect_err("expected parse error");
    assert!(matches!(err, OtelError::Parse { .. }));
}

#[test]
fn build_export_logs_request_maps_fields_and_preserves_body_verbatim() {
    // A JSON-shaped message string proves no structured parsing happens — it must be
    // stored as one opaque string, not decoded.
    let events = r#"{"id":"evt-1","timestamp":1510109208016,"message":"{\"nested\":\"json\"}"}"#;
    let raw = data_message(
        "/aws/rds/instance/mydb/postgresql",
        "mydb",
        "999988887777",
        events,
    );
    let msg = decode_cloudwatch_logs_record(&raw, 0)
        .expect("decode ok")
        .expect("Some");

    let req = build_export_logs_request(&msg);
    assert_eq!(req.resource_logs.len(), 1);
    let rl = &req.resource_logs[0];
    let resource = rl.resource.as_ref().expect("resource present");

    let attr = |key: &str| -> String {
        resource
            .attributes
            .iter()
            .find(|kv| kv.key == key)
            .and_then(|kv| kv.value.as_ref())
            .map(|v| match &v.value {
                Some(micromegas_otel_ingestion::proto::any_value::Value::StringValue(s)) => {
                    s.clone()
                }
                _ => panic!("expected string value for {key}"),
            })
            .unwrap_or_else(|| panic!("missing attribute {key}"))
    };
    assert_eq!(attr("service.name"), "/aws/rds/instance/mydb/postgresql");
    assert_eq!(attr("service.instance.id"), "mydb");
    assert_eq!(attr("cloud.account.id"), "999988887777");
    assert_eq!(
        attr("aws.log.group.name"),
        "/aws/rds/instance/mydb/postgresql"
    );
    assert_eq!(attr("aws.log.stream.name"), "mydb");

    assert_eq!(rl.scope_logs.len(), 1);
    let log_records = &rl.scope_logs[0].log_records;
    assert_eq!(log_records.len(), 1);
    let record = &log_records[0];
    assert_eq!(record.time_unix_nano, 1510109208016u64 * 1_000_000);
    match record.body.as_ref().and_then(|b| b.value.as_ref()) {
        Some(micromegas_otel_ingestion::proto::any_value::Value::StringValue(s)) => {
            assert_eq!(s, r#"{"nested":"json"}"#);
        }
        other => panic!("expected string body, got {other:?}"),
    }
}

#[test]
fn full_pipeline_decode_build_split_produces_one_block_with_matching_record_count() {
    let events = r#"{"id":"evt-1","timestamp":1510109208016,"message":"line one"},{"id":"evt-2","timestamp":1510109208100,"message":"line two"},{"id":"evt-3","timestamp":1510109208200,"message":"line three"}"#;
    let raw = data_message("/ecs/svc", "task/abc", "111122223333", events);
    let msg = decode_cloudwatch_logs_record(&raw, 0)
        .expect("decode ok")
        .expect("Some");
    let req = build_export_logs_request(&msg);
    let blocks = split_logs(req).expect("split_logs");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].nb_records, 3);
}

#[test]
fn multi_record_batch_with_distinct_log_streams_yields_distinct_process_ids() {
    let events1 = r#"{"id":"evt-1","timestamp":1510109208016,"message":"a"}"#;
    let events2 = r#"{"id":"evt-2","timestamp":1510109208016,"message":"b"}"#;
    let raw1 = data_message("/ecs/svc", "task/one", "111122223333", events1);
    let raw2 = data_message("/ecs/svc", "task/two", "111122223333", events2);

    let msg1 = decode_cloudwatch_logs_record(&raw1, 0)
        .expect("decode ok")
        .expect("Some");
    let msg2 = decode_cloudwatch_logs_record(&raw2, 1)
        .expect("decode ok")
        .expect("Some");

    let blocks1 = split_logs(build_export_logs_request(&msg1)).expect("split_logs");
    let blocks2 = split_logs(build_export_logs_request(&msg2)).expect("split_logs");
    assert_eq!(blocks1.len(), 1);
    assert_eq!(blocks2.len(), 1);
    assert_ne!(blocks1[0].process_id, blocks2[0].process_id);

    // Cross-check against the identity formula directly.
    let pid1 = process_id_from_resource(Some(&micromegas_otel_ingestion::proto::Resource {
        attributes: blocks1[0].resource_attrs.clone(),
        dropped_attributes_count: 0,
        entity_refs: vec![],
    }));
    let pid2 = process_id_from_resource(Some(&micromegas_otel_ingestion::proto::Resource {
        attributes: blocks2[0].resource_attrs.clone(),
        dropped_attributes_count: 0,
        entity_refs: vec![],
    }));
    assert_eq!(blocks1[0].process_id, pid1);
    assert_eq!(blocks2[0].process_id, pid2);
}
