//! Integration tests for OTLP/JSON parsing.
//!
//! Verifies that the JSON encoding path produces the same blocks as protobuf,
//! that canonical OTLP/JSON fixtures parse correctly, and that the documented
//! string-encoded-timestamp constraint is enforced.

mod fixtures;

use fixtures::*;
use micromegas_otel_ingestion::block::{split_logs, split_metrics, split_traces};
use micromegas_otel_ingestion::proto::{
    ExportLogsServiceRequest, ExportMetricsServiceRequest, ExportTraceServiceRequest,
};

/// Serializing a proto-built request to JSON then back must produce identical blocks.
#[test]
fn logs_json_round_trip_matches_proto() {
    let req = make_logs_request(
        "svc",
        "host1",
        1,
        vec![
            log_record(1_700_000_000_000_000_000, 9, "hello"),
            log_record(1_700_000_005_000_000_000, 17, "boom"),
        ],
    );
    let json_bytes = serde_json::to_vec(&req).expect("serialize to JSON");
    let proto_blocks = split_logs(req).unwrap();

    let req_from_json: ExportLogsServiceRequest =
        serde_json::from_slice(&json_bytes).expect("deserialize from JSON");
    let json_blocks = split_logs(req_from_json).unwrap();

    assert_eq!(proto_blocks.len(), json_blocks.len());
    assert_eq!(
        proto_blocks[0].block.block_id,
        json_blocks[0].block.block_id
    );
    assert_eq!(proto_blocks[0].nb_records, json_blocks[0].nb_records);
    assert_eq!(proto_blocks[0].begin_time, json_blocks[0].begin_time);
    assert_eq!(proto_blocks[0].end_time, json_blocks[0].end_time);
}

/// Canonical OTLP/JSON log fixture (string-quoted timestamps, camelCase fields) parses.
#[test]
fn logs_canonical_json_fixture_parses() {
    let json = r#"{
        "resourceLogs": [{
            "resource": {
                "attributes": [{"key": "service.name", "value": {"stringValue": "test-svc"}}]
            },
            "scopeLogs": [{
                "scope": {"name": "test-scope"},
                "logRecords": [{
                    "timeUnixNano": "1700000000000000000",
                    "severityNumber": 9,
                    "body": {"stringValue": "hello world"}
                }]
            }]
        }]
    }"#;
    let req: ExportLogsServiceRequest = serde_json::from_str(json).unwrap();
    let blocks = split_logs(req).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].nb_records, 1);
    assert_eq!(
        blocks[0].begin_time.timestamp_nanos_opt().unwrap(),
        1_700_000_000_000_000_000
    );
    assert_eq!(
        blocks[0].end_time.timestamp_nanos_opt().unwrap(),
        1_700_000_000_000_000_000
    );
}

/// Canonical OTLP/JSON traces fixture (string-quoted timestamps, hex trace/span ids) parses.
#[test]
fn traces_canonical_json_fixture_parses() {
    let json = r#"{
        "resourceSpans": [{
            "resource": {
                "attributes": [{"key": "service.name", "value": {"stringValue": "test-svc"}}]
            },
            "scopeSpans": [{
                "scope": {"name": "test-scope"},
                "spans": [{
                    "traceId": "11111111111111111111111111111111",
                    "spanId": "2222222222222222",
                    "name": "handle_request",
                    "kind": 1,
                    "startTimeUnixNano": "1700000000000000000",
                    "endTimeUnixNano": "1700000000500000000"
                }]
            }]
        }]
    }"#;
    let req: ExportTraceServiceRequest = serde_json::from_str(json).unwrap();
    let blocks = split_traces(req).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(
        blocks[0].begin_time.timestamp_nanos_opt().unwrap(),
        1_700_000_000_000_000_000
    );
    assert_eq!(
        blocks[0].end_time.timestamp_nanos_opt().unwrap(),
        1_700_000_000_500_000_000
    );
}

/// String-quoted `timeUnixNano` (the OTLP/JSON spec mandated form) is accepted.
#[test]
fn string_encoded_timestamp_accepted() {
    let json = r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"timeUnixNano":"1700000000000000000","severityNumber":9,"body":{"stringValue":"ts-test"}}]}]}]}"#;
    let req: ExportLogsServiceRequest = serde_json::from_str(json).unwrap();
    let blocks = split_logs(req).unwrap();
    assert_eq!(
        blocks[0].begin_time.timestamp_nanos_opt().unwrap(),
        1_700_000_000_000_000_000
    );
}

/// Bare-number `timeUnixNano` (lenient proto3 JSON, not mandated by spec) is rejected.
/// This locks in the documented limitation so a future dependency change is noticed.
#[test]
fn bare_number_timestamp_rejected() {
    let json = r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"timeUnixNano":1700000000000000000,"severityNumber":9,"body":{"stringValue":"ts-test"}}]}]}]}"#;
    let result: Result<ExportLogsServiceRequest, _> = serde_json::from_str(json);
    assert!(result.is_err(), "bare-number timeUnixNano must be rejected");
}

/// Empty `resourceLogs` / `resourceMetrics` / `resourceSpans` → no blocks, no error.
#[test]
fn empty_json_requests_yield_no_blocks() {
    let req: ExportLogsServiceRequest = serde_json::from_str(r#"{"resourceLogs":[]}"#).unwrap();
    assert!(split_logs(req).unwrap().is_empty());

    let req: ExportMetricsServiceRequest =
        serde_json::from_str(r#"{"resourceMetrics":[]}"#).unwrap();
    assert!(split_metrics(req).unwrap().is_empty());

    let req: ExportTraceServiceRequest = serde_json::from_str(r#"{"resourceSpans":[]}"#).unwrap();
    assert!(split_traces(req).unwrap().is_empty());
}
