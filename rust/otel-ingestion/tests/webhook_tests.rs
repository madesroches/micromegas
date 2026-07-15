//! Integration tests for the webhook → synthetic-OTLP-logs-request path.
//!
//! Mirrors `split_tests.rs`: no database, just shape assertions on
//! `build_webhook_request` and the `split_logs` output it feeds.

mod fixtures;

use fixtures::s_kv;
use micromegas_otel_ingestion::block::{split_logs, split_logs_with_extra_hash_input};
use micromegas_otel_ingestion::handler::build_webhook_request;
use micromegas_otel_ingestion::identity::process_id_from_resource;
use micromegas_otel_ingestion::proto::{Resource, SeverityNumber, any_value};

#[test]
fn build_webhook_request_shape() {
    let attrs = vec![
        s_kv("service.name", "gitlab"),
        s_kv("service.namespace", "ci"),
    ];
    let req = build_webhook_request(attrs, "push-events".to_string(), b"{\"a\":1}");

    assert_eq!(req.resource_logs.len(), 1);
    let rl = &req.resource_logs[0];
    assert_eq!(rl.scope_logs.len(), 1);
    let scope_logs = &rl.scope_logs[0];
    assert_eq!(scope_logs.scope.as_ref().unwrap().name, "push-events");
    assert_eq!(scope_logs.log_records.len(), 1);

    let record = &scope_logs.log_records[0];
    assert_eq!(record.severity_number, SeverityNumber::Info as i32);
    match record.body.as_ref().and_then(|b| b.value.as_ref()) {
        Some(any_value::Value::StringValue(s)) => assert_eq!(s, "{\"a\":1}"),
        other => panic!("expected StringValue body, got {other:?}"),
    }
    // Left at 0 so split_logs's existing backfill stamps ingestion time.
    assert_eq!(record.time_unix_nano, 0);
    assert_eq!(record.observed_time_unix_nano, 0);
}

#[test]
fn build_webhook_request_lossy_converts_non_utf8_body() {
    let attrs = vec![s_kv("service.name", "gitlab")];
    // 0xFF is never valid UTF-8 on its own; from_utf8_lossy replaces it with U+FFFD.
    let non_utf8_body: &[u8] = b"\xff\xfe\x00binary";
    let req = build_webhook_request(attrs, "push-events".to_string(), non_utf8_body);

    let record = &req.resource_logs[0].scope_logs[0].log_records[0];
    match record.body.as_ref().and_then(|b| b.value.as_ref()) {
        Some(any_value::Value::StringValue(s)) => assert!(s.contains('\u{FFFD}')),
        other => panic!("expected StringValue body, got {other:?}"),
    }
}

#[test]
fn split_logs_on_webhook_request_yields_one_backfilled_block_with_matching_identity() {
    let attrs = vec![
        s_kv("service.name", "gitlab"),
        s_kv("service.namespace", "ci"),
    ];
    let req = build_webhook_request(attrs.clone(), "push-events".to_string(), b"{}");
    let blocks = split_logs(req).unwrap();
    assert_eq!(blocks.len(), 1);
    let b = &blocks[0];
    assert_eq!(b.nb_records, 1);

    // Backfilled timestamp is well past epoch (sentinel: 2024-01-01).
    let sentinel_ns: i64 = 1_704_067_200_000_000_000;
    assert!(b.begin_time.timestamp_nanos_opt().unwrap() > sentinel_ns);
    assert!(b.end_time.timestamp_nanos_opt().unwrap() > sentinel_ns);

    let resource = Resource {
        attributes: attrs,
        dropped_attributes_count: 0,
        entity_refs: vec![],
    };
    assert_eq!(b.process_id, process_id_from_resource(Some(&resource)));
}

#[test]
fn identical_webhook_deliveries_dedup_distinct_bodies_dont() {
    let attrs = vec![s_kv("service.name", "gitlab")];
    let req1 = build_webhook_request(attrs.clone(), "push-events".to_string(), b"same body");
    let req2 = build_webhook_request(attrs.clone(), "push-events".to_string(), b"same body");
    let req_diff = build_webhook_request(attrs, "push-events".to_string(), b"different body");

    let a = split_logs(req1).unwrap();
    let b = split_logs(req2).unwrap();
    let c = split_logs(req_diff).unwrap();

    assert_eq!(a[0].block.block_id, b[0].block.block_id);
    assert_ne!(a[0].block.block_id, c[0].block.block_id);
}

#[test]
fn extra_hash_input_changes_block_id_but_empty_matches_plain_split_logs() {
    let attrs = vec![s_kv("service.name", "gitlab")];
    let req_plain = build_webhook_request(attrs.clone(), "push-events".to_string(), b"same body");
    let req_empty_extra =
        build_webhook_request(attrs.clone(), "push-events".to_string(), b"same body");
    let req_with_extra =
        build_webhook_request(attrs.clone(), "push-events".to_string(), b"same body");
    let req_with_other_extra =
        build_webhook_request(attrs, "push-events".to_string(), b"same body");

    let plain = split_logs(req_plain).unwrap();
    let empty_extra = split_logs_with_extra_hash_input(req_empty_extra, &[]).unwrap();
    let with_extra =
        split_logs_with_extra_hash_input(req_with_extra, b"x-gitlab-event-uuid:abc").unwrap();
    let with_other_extra =
        split_logs_with_extra_hash_input(req_with_other_extra, b"x-gitlab-event-uuid:def").unwrap();

    // &[] reproduces split_logs's OTLP-only behavior exactly.
    assert_eq!(plain[0].block.block_id, empty_extra[0].block.block_id);
    // A non-empty extra_hash_input changes block_id even though the request is identical...
    assert_ne!(plain[0].block.block_id, with_extra[0].block.block_id);
    // ...and different extra_hash_input values (e.g. distinct unrecognized headers) produce
    // distinct block_ids for an otherwise byte-identical webhook body.
    assert_ne!(
        with_extra[0].block.block_id,
        with_other_extra[0].block.block_id
    );
}
