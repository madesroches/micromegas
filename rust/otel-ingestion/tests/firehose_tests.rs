//! Unit tests for the Kinesis Firehose HTTP Endpoint Delivery envelope adapter.
//!
//! No database: pure shape assertions on `decode_firehose_envelope`, plus a
//! round-trip through `split_metrics` to prove a decoded record is a real
//! `ExportMetricsServiceRequest` protobuf.

mod fixtures;

use base64::Engine as _;
use fixtures::{gauge_metric, make_metrics_request};
use micromegas_otel_ingestion::block::split_metrics;
use micromegas_otel_ingestion::error::{OtelError, Signal};
use micromegas_otel_ingestion::handler::{decode_firehose_envelope, decode_next_length_delimited};
use micromegas_otel_ingestion::proto::ExportMetricsServiceRequest;
use prost::Message;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn envelope_json(request_id: Option<&str>, records: &[&[u8]]) -> String {
    let records_json: Vec<String> = records
        .iter()
        .map(|r| format!(r#"{{"data":"{}"}}"#, b64(r)))
        .collect();
    let request_id_field = match request_id {
        Some(id) => format!(r#""requestId":"{id}","#),
        None => String::new(),
    };
    format!(
        r#"{{{request_id_field}"timestamp":1578090901599,"records":[{}]}}"#,
        records_json.join(",")
    )
}

#[test]
fn single_record_round_trips_a_real_otlp_metrics_protobuf() {
    let req = make_metrics_request(
        "firehose-e2e",
        "firehose-host",
        1,
        vec![gauge_metric("cpu.usage", "percent", 1_000, 42)],
    );
    let payload = req.encode_to_vec();
    let body = envelope_json(Some("req-1"), &[&payload]);

    let envelope =
        decode_firehose_envelope(body.as_bytes(), Signal::Metrics).expect("decode envelope");
    assert_eq!(envelope.request_id, "req-1");
    assert_eq!(envelope.records.len(), 1);

    let decoded = ExportMetricsServiceRequest::decode(envelope.records[0].as_slice())
        .expect("decode record as ExportMetricsServiceRequest");
    assert_eq!(decoded, req);

    let blocks = split_metrics(decoded).expect("split_metrics");
    assert_eq!(blocks.len(), 1);
}

#[test]
fn multi_record_batch_preserves_order() {
    let req1 = make_metrics_request(
        "svc-1",
        "host-1",
        1,
        vec![gauge_metric("metric.one", "1", 100, 1)],
    );
    let req2 = make_metrics_request(
        "svc-2",
        "host-2",
        2,
        vec![gauge_metric("metric.two", "1", 200, 2)],
    );
    let p1 = req1.encode_to_vec();
    let p2 = req2.encode_to_vec();
    let body = envelope_json(Some("req-multi"), &[&p1, &p2]);

    let envelope =
        decode_firehose_envelope(body.as_bytes(), Signal::Metrics).expect("decode envelope");
    assert_eq!(envelope.records.len(), 2);
    assert_eq!(envelope.records[0], p1);
    assert_eq!(envelope.records[1], p2);
}

#[test]
fn malformed_json_is_parse_error() {
    let err = decode_firehose_envelope(b"not json at all", Signal::Metrics)
        .expect_err("expected parse error");
    assert!(matches!(err, OtelError::Parse { .. }));
}

#[test]
fn malformed_base64_in_a_record_is_parse_error() {
    let body =
        r#"{"requestId":"req-bad","timestamp":1,"records":[{"data":"not-valid-base64!!!"}]}"#;
    let err = decode_firehose_envelope(body.as_bytes(), Signal::Metrics)
        .expect_err("expected parse error");
    assert!(matches!(err, OtelError::Parse { .. }));
}

#[test]
fn empty_records_yields_ok_with_zero_records() {
    let body = envelope_json(Some("req-empty"), &[]);
    let envelope =
        decode_firehose_envelope(body.as_bytes(), Signal::Metrics).expect("decode envelope");
    assert_eq!(envelope.request_id, "req-empty");
    assert!(envelope.records.is_empty());
}

#[test]
fn absent_records_field_yields_ok_with_zero_records() {
    let body = r#"{"requestId":"req-absent","timestamp":1}"#;
    let envelope =
        decode_firehose_envelope(body.as_bytes(), Signal::Metrics).expect("decode envelope");
    assert_eq!(envelope.request_id, "req-absent");
    assert!(envelope.records.is_empty());
}

#[test]
fn missing_request_id_defaults_to_empty_string() {
    let body = envelope_json(None, &[]);
    let envelope =
        decode_firehose_envelope(body.as_bytes(), Signal::Metrics).expect("decode envelope");
    assert_eq!(envelope.request_id, "");
}

// ---------------------------------------------------------------------------
// `decode_next_length_delimited` — CloudWatch Metric Streams packs one-or-more
// `[varint32 length][message bytes]` entries per record; these tests cover that framing
// directly (issue #1381: a fresh `M::decode` per record misreads the second message's
// length-prefix byte as a protobuf tag once a record carries more than one message).
// ---------------------------------------------------------------------------

#[test]
fn single_length_delimited_message_decodes_then_exhausts() {
    let req = make_metrics_request(
        "svc-single",
        "host-single",
        1,
        vec![gauge_metric("cpu.usage", "percent", 1_000, 42)],
    );
    let record = req.encode_length_delimited_to_vec();

    let mut buf: &[u8] = &record;
    let decoded =
        decode_next_length_delimited::<ExportMetricsServiceRequest>(&mut buf, Signal::Metrics)
            .expect("decode first message")
            .expect("first message present");
    assert_eq!(decoded, req);

    let second =
        decode_next_length_delimited::<ExportMetricsServiceRequest>(&mut buf, Signal::Metrics)
            .expect("second call must not error");
    assert!(second.is_none(), "record has exactly one message");
}

#[test]
fn two_concatenated_length_delimited_messages_decode_in_order() {
    let req1 = make_metrics_request(
        "svc-1",
        "host-1",
        1,
        vec![gauge_metric("metric.one", "1", 100, 1)],
    );
    let req2 = make_metrics_request(
        "svc-2",
        "host-2",
        2,
        vec![gauge_metric("metric.two", "1", 200, 2)],
    );
    let mut record = req1.encode_length_delimited_to_vec();
    record.extend(req2.encode_length_delimited_to_vec());

    let mut buf: &[u8] = &record;
    let first =
        decode_next_length_delimited::<ExportMetricsServiceRequest>(&mut buf, Signal::Metrics)
            .expect("decode first message")
            .expect("first message present");
    assert_eq!(first, req1);

    let second =
        decode_next_length_delimited::<ExportMetricsServiceRequest>(&mut buf, Signal::Metrics)
            .expect("decode second message")
            .expect("second message present");
    assert_eq!(second, req2);

    let third =
        decode_next_length_delimited::<ExportMetricsServiceRequest>(&mut buf, Signal::Metrics)
            .expect("third call must not error");
    assert!(third.is_none(), "record has exactly two messages");
}

#[test]
fn zero_length_record_yields_none_on_first_call() {
    let record: Vec<u8> = vec![];
    let mut buf: &[u8] = &record;
    let result =
        decode_next_length_delimited::<ExportMetricsServiceRequest>(&mut buf, Signal::Metrics)
            .expect("empty record is not an error");
    assert!(result.is_none());
}

#[test]
fn valid_message_followed_by_malformed_framing_keeps_the_valid_message() {
    let req = make_metrics_request(
        "svc-valid",
        "host-valid",
        1,
        vec![gauge_metric("metric.valid", "1", 100, 1)],
    );
    let mut record = req.encode_length_delimited_to_vec();
    // Append a length prefix (varint 50) claiming 50 bytes of message body that don't exist.
    record.push(50);

    let mut buf: &[u8] = &record;
    let first =
        decode_next_length_delimited::<ExportMetricsServiceRequest>(&mut buf, Signal::Metrics)
            .expect("decode first (valid) message")
            .expect("first message present");
    assert_eq!(first, req);

    let second =
        decode_next_length_delimited::<ExportMetricsServiceRequest>(&mut buf, Signal::Metrics);
    assert!(
        matches!(second, Err(OtelError::Parse { .. })),
        "truncated second message must error, not silently discard the first: {second:?}"
    );
}

#[test]
fn malformed_framing_on_first_message_is_parse_error() {
    // Length prefix (varint 100) claiming far more bytes than are actually present.
    let record: Vec<u8> = vec![100, 1, 2, 3];
    let mut buf: &[u8] = &record;
    let result =
        decode_next_length_delimited::<ExportMetricsServiceRequest>(&mut buf, Signal::Metrics);
    assert!(matches!(result, Err(OtelError::Parse { .. })));
}
