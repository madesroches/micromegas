//! Integration tests for the webhook → synthetic-OTLP-logs-request path.
//!
//! Mirrors `split_tests.rs`: no database, just shape assertions on
//! `build_webhook_request` and the `split_logs` output it feeds.

mod fixtures;

use fixtures::s_kv;
use micromegas_otel_ingestion::block::split_logs;
use micromegas_otel_ingestion::handler::build_webhook_request;
use micromegas_otel_ingestion::identity::{IdentityContext, process_id_from_resource};
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
    // Left at 0 and stored as-is — the arrival-time fallback lives in the block's
    // begin_time/end_time (see split_logs's doc comment), not in the record.
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
fn split_logs_on_webhook_request_yields_one_block_with_arrival_time_bounds_and_matching_identity() {
    let attrs = vec![
        s_kv("service.name", "gitlab"),
        s_kv("service.namespace", "ci"),
    ];
    let req = build_webhook_request(attrs.clone(), "push-events".to_string(), b"{}");
    let blocks = split_logs(req, IdentityContext::default()).unwrap();
    assert_eq!(blocks.len(), 1);
    let b = &blocks[0];
    assert_eq!(b.nb_records, 1);

    // Block bounds fall back to arrival time (sentinel: 2024-01-01) via
    // logs_bounds/build_prepared_block, even though no record carries a timestamp.
    let sentinel_ns: i64 = 1_704_067_200_000_000_000;
    assert!(b.begin_time.timestamp_nanos_opt().unwrap() > sentinel_ns);
    assert!(b.end_time.timestamp_nanos_opt().unwrap() > sentinel_ns);

    let resource = Resource {
        attributes: attrs,
        dropped_attributes_count: 0,
        entity_refs: vec![],
    };
    assert_eq!(
        b.process_id,
        process_id_from_resource(Some(&resource), IdentityContext::default())
    );
}

#[test]
fn identical_webhook_deliveries_dedup_distinct_bodies_dont() {
    let attrs = vec![s_kv("service.name", "gitlab")];
    let req1 = build_webhook_request(attrs.clone(), "push-events".to_string(), b"same body");
    let req2 = build_webhook_request(attrs.clone(), "push-events".to_string(), b"same body");
    let req_diff = build_webhook_request(attrs, "push-events".to_string(), b"different body");

    let a = split_logs(req1, IdentityContext::default()).unwrap();
    let b = split_logs(req2, IdentityContext::default()).unwrap();
    let c = split_logs(req_diff, IdentityContext::default()).unwrap();

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

    let plain = split_logs(req_plain, IdentityContext::default()).unwrap();
    let empty_extra_ctx = IdentityContext {
        audience: None,
        extra_hash_input: &[],
    };
    let with_extra_ctx = IdentityContext {
        audience: None,
        extra_hash_input: b"x-gitlab-event-uuid:abc",
    };
    let with_other_extra_ctx = IdentityContext {
        audience: None,
        extra_hash_input: b"x-gitlab-event-uuid:def",
    };
    let empty_extra = split_logs(req_empty_extra, empty_extra_ctx).unwrap();
    let with_extra = split_logs(req_with_extra, with_extra_ctx).unwrap();
    let with_other_extra = split_logs(req_with_other_extra, with_other_extra_ctx).unwrap();

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

#[test]
fn extra_hash_input_still_influences_block_id_alongside_an_audience() {
    // The webhook path's extra_hash_input and the audience prefix are two independent
    // inputs into the same hash -- both must keep mattering together.
    let attrs = vec![s_kv("service.name", "gitlab")];
    let req_a = build_webhook_request(attrs.clone(), "push-events".to_string(), b"same body");
    let req_b = build_webhook_request(attrs, "push-events".to_string(), b"same body");

    let ctx_same_extra_diff_header = IdentityContext {
        audience: Some("team-a"),
        extra_hash_input: b"x-gitlab-event-uuid:abc",
    };
    let ctx_same_extra_other_header = IdentityContext {
        audience: Some("team-a"),
        extra_hash_input: b"x-gitlab-event-uuid:def",
    };
    let a = split_logs(req_a, ctx_same_extra_diff_header).unwrap();
    let b = split_logs(req_b, ctx_same_extra_other_header).unwrap();
    assert_ne!(
        a[0].block.block_id, b[0].block.block_id,
        "extra_hash_input must still influence block_id when an audience is also present"
    );
}
