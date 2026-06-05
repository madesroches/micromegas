//! Integration tests for the per-resource block splitter.
//!
//! These tests don't touch the database — they only check that we shape the
//! output `PreparedBlock` set correctly given an `Export*ServiceRequest`. The
//! handler-level tests that round-trip through the in-memory ingestion service
//! belong in a separate suite (deferred — would need a mock `WebIngestionService`
//! or a real Postgres + object store; running these as part of the ingestion
//! end-to-end harness instead).

mod fixtures;

use fixtures::*;
use micromegas_otel_ingestion::FORMAT_OTLP_LOGS;
use micromegas_otel_ingestion::block::{split_logs, split_metrics, split_traces};
use micromegas_otel_ingestion::identity::SignalKey;
use micromegas_otel_ingestion::proto::{ExportLogsServiceRequest, ResourceLogs};
use prost::Message;

#[test]
fn logs_split_one_block_per_resource_with_correct_bounds() {
    let req = make_logs_request(
        "claude-code",
        "macbook",
        1234,
        vec![
            log_record(1_700_000_000_000_000_000, 9, "hello"),
            log_record(1_700_000_005_000_000_000, 17, "boom"),
        ],
    );
    let blocks = split_logs(req).unwrap();
    assert_eq!(blocks.len(), 1);
    let b = &blocks[0];
    assert_eq!(b.nb_records, 2);
    assert!(matches!(b.signal, SignalKey::Logs));
    // Bounds reflect min/max time_unix_nano across records.
    assert_eq!(
        b.begin_time.timestamp_nanos_opt().unwrap(),
        1_700_000_000_000_000_000
    );
    assert_eq!(
        b.end_time.timestamp_nanos_opt().unwrap(),
        1_700_000_005_000_000_000
    );
    // The block envelope reflects what's ingested.
    assert_eq!(b.block.process_id, b.process_id);
    assert_eq!(b.block.stream_id, b.stream_id);
    assert_eq!(b.block.nb_objects, 2);
}

#[test]
fn logs_block_id_is_content_addressed_and_idempotent() {
    let req = make_logs_request(
        "svc",
        "h1",
        1,
        vec![log_record(1_700_000_000_000_000_000, 9, "a")],
    );
    let req2 = make_logs_request(
        "svc",
        "h1",
        1,
        vec![log_record(1_700_000_000_000_000_000, 9, "a")],
    );
    let req_diff = make_logs_request(
        "svc",
        "h1",
        1,
        vec![log_record(1_700_000_000_000_000_000, 9, "b")],
    );
    let a = split_logs(req).unwrap();
    let b = split_logs(req2).unwrap();
    let c = split_logs(req_diff).unwrap();
    assert_eq!(a[0].block.block_id, b[0].block.block_id);
    assert_eq!(a[0].process_id, b[0].process_id);
    assert_ne!(a[0].block.block_id, c[0].block.block_id);
    // Process_id stable across body changes — same identifying tuple.
    assert_eq!(a[0].process_id, c[0].process_id);
}

#[test]
fn metrics_split_emits_one_block_for_a_mixed_kind_resource() {
    let req = make_metrics_request(
        "claude-code",
        "macbook",
        1234,
        vec![
            sum_metric(
                "claude_code.token.usage",
                "tokens",
                1_700_000_000_000_000_000,
                42.0,
            ),
            gauge_metric(
                "claude_code.session.count",
                "1",
                1_700_000_001_000_000_000,
                3,
            ),
        ],
    );
    let blocks = split_metrics(req).unwrap();
    assert_eq!(blocks.len(), 1);
    let b = &blocks[0];
    assert_eq!(b.nb_records, 2);
    assert!(matches!(b.signal, SignalKey::Metrics));
    assert_eq!(
        b.begin_time.timestamp_nanos_opt().unwrap(),
        1_700_000_000_000_000_000
    );
    assert_eq!(
        b.end_time.timestamp_nanos_opt().unwrap(),
        1_700_000_001_000_000_000
    );
}

#[test]
fn traces_split_carries_proto_in_payload() {
    let trace_id = [0x11u8; 16];
    let span_id = [0x22u8; 8];
    let req = make_traces_request(
        "svc",
        "h1",
        1,
        vec![root_span(
            "handle_request",
            trace_id,
            span_id,
            1_700_000_000_000_000_000,
            1_700_000_000_500_000_000,
        )],
    );
    let blocks = split_traces(req).unwrap();
    assert_eq!(blocks.len(), 1);
    let b = &blocks[0];
    assert!(matches!(b.signal, SignalKey::Traces));
    // Re-decode the payload — the block.payload.objects must be a valid ResourceSpans proto.
    let decoded =
        opentelemetry_proto::tonic::trace::v1::ResourceSpans::decode(&*b.block.payload.objects)
            .unwrap();
    assert_eq!(decoded.scope_spans.len(), 1);
    assert_eq!(decoded.scope_spans[0].spans.len(), 1);
    assert_eq!(decoded.scope_spans[0].spans[0].trace_id, trace_id.to_vec());
}

#[test]
fn empty_request_yields_no_blocks() {
    let req = ExportLogsServiceRequest {
        resource_logs: vec![],
    };
    let blocks = split_logs(req).unwrap();
    assert!(blocks.is_empty());
}

#[test]
fn distinct_resources_split_into_distinct_processes() {
    let req = make_logs_request(
        "svc-a",
        "h1",
        1,
        vec![log_record(1_700_000_000_000_000_000, 9, "a")],
    );
    let req_b = make_logs_request(
        "svc-b",
        "h1",
        1,
        vec![log_record(1_700_000_000_000_000_000, 9, "a")],
    );
    let a = split_logs(req).unwrap();
    let b = split_logs(req_b).unwrap();
    assert_ne!(a[0].process_id, b[0].process_id);
    assert_ne!(a[0].stream_id, b[0].stream_id);
}

#[test]
fn format_constants_match_signal_keys() {
    // Smoke: stream tag and format are set consistently across the crate.
    assert_eq!(FORMAT_OTLP_LOGS, "otlp/v1/logs");
    assert_eq!(SignalKey::Logs.as_str(), "logs");
}

#[test]
fn logs_split_backfills_observed_time_when_both_timestamps_zero() {
    let req = make_logs_request(
        "svc",
        "h1",
        1,
        vec![
            log_record_no_timestamp(9, "no-ts-a"),
            log_record_no_timestamp(9, "no-ts-b"),
        ],
    );
    let blocks = split_logs(req).unwrap();
    assert_eq!(blocks.len(), 1);
    let b = &blocks[0];
    assert_eq!(b.nb_records, 2);
    // Envelope times must be well past epoch after backfill (sentinel: 2024-01-01).
    let sentinel_ns: i64 = 1_704_067_200_000_000_000;
    assert!(b.begin_time.timestamp_nanos_opt().unwrap() > sentinel_ns);
    assert!(b.end_time.timestamp_nanos_opt().unwrap() > sentinel_ns);
    // Stored proto must have observed_time_unix_nano backfilled on every record.
    let decoded = ResourceLogs::decode(&*b.block.payload.objects).unwrap();
    for scope in &decoded.scope_logs {
        for record in &scope.log_records {
            assert_ne!(
                record.observed_time_unix_nano, 0,
                "record still has zero observed timestamp after backfill"
            );
        }
    }
}

#[test]
fn logs_split_preserves_existing_observed_time() {
    let observed: u64 = 1_700_000_000_000_000_000;
    let req = make_logs_request(
        "svc",
        "h1",
        1,
        vec![log_record_observed_only(observed, 9, "has-observed")],
    );
    let blocks = split_logs(req).unwrap();
    assert_eq!(blocks.len(), 1);
    let decoded = ResourceLogs::decode(&*blocks[0].block.payload.objects).unwrap();
    let record = &decoded.scope_logs[0].log_records[0];
    assert_eq!(
        record.observed_time_unix_nano, observed,
        "existing observed timestamp must not be overwritten by backfill"
    );
}

#[test]
fn logs_split_mixed_timestamps_all_survive() {
    let known_ts: u64 = 1_700_000_000_000_000_000; // 2023-11-14
    let req = make_logs_request(
        "svc",
        "h1",
        1,
        vec![
            log_record(known_ts, 9, "has-time"),
            log_record_no_timestamp(9, "no-ts"),
        ],
    );
    let blocks = split_logs(req).unwrap();
    assert_eq!(blocks.len(), 1);
    let b = &blocks[0];
    assert_eq!(b.nb_records, 2);
    // begin_time reflects the known minimum non-zero timestamp.
    assert_eq!(b.begin_time.timestamp_nanos_opt().unwrap(), known_ts as i64);
    // end_time is the backfilled now_nanos, which must be after 2024-01-01.
    let sentinel_ns: i64 = 1_704_067_200_000_000_000;
    assert!(b.end_time.timestamp_nanos_opt().unwrap() > sentinel_ns);
}

#[test]
fn logs_split_block_id_stable_across_retries_for_zero_timestamp_payload() {
    let req1 = make_logs_request("svc", "h1", 1, vec![log_record_no_timestamp(9, "msg")]);
    let req2 = make_logs_request("svc", "h1", 1, vec![log_record_no_timestamp(9, "msg")]);
    let blocks1 = split_logs(req1).unwrap();
    let blocks2 = split_logs(req2).unwrap();
    assert_eq!(
        blocks1[0].block.block_id, blocks2[0].block.block_id,
        "block_id must be stable across retries with identical zero-timestamp payloads"
    );
}
