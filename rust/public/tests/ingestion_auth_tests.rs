// Tests for `micromegas::servers::ingestion_auth` (the rejected `insert_process` and OTLP logger)
// and the caller details logged by `tonic_auth_interceptor::check_auth`, on a real `Router` and
// an in-memory log sink. Tests are `#[serial]` because they share the global tracing dispatch,
// and select their log lines by a per-test `X-Forwarded-For` address.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::middleware;
use axum::routing::post;
use bytes::Bytes;
use flate2::Compression;
use flate2::write::GzEncoder;
use micromegas::servers::ingestion_auth::{RejectedRequestLogger, ingestion_auth_middleware};
use micromegas::servers::tonic_auth_interceptor::check_auth;
use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas_auth::types::{AuthContext, AuthProvider, ProviderUnavailable, RequestParts};
use micromegas_tracing::levels::{LevelFilter, set_max_level};
use micromegas_tracing::process_info::ProcessInfo;
use micromegas_tracing::test_utils::{CapturedLog, collect_logs, init_in_memory_tracing};
use prost::Message;
use serial_test::serial;
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tower::ServiceExt;

const INSERT_PROCESS: &str = "/ingestion/insert_process";
const OTLP_LOGS: &str = "/ingestion/otlp/v1/logs";
const VALID_KEY: &str = "valid-key";
const TEST_USER_AGENT: &str = "otel-test/1.0";

struct Harness {
    app: Router,
    reached: Arc<AtomicUsize>,
}

fn harness() -> Harness {
    harness_with_logger(RejectedRequestLogger::new())
}

fn harness_with_logger(logger: RejectedRequestLogger) -> Harness {
    let keyring = parse_key_ring(&format!(r#"[{{"name": "t", "key": "{VALID_KEY}"}}]"#))
        .expect("parse keyring");
    harness_with_provider(Arc::new(ApiKeyAuthProvider::new(keyring)), logger)
}

fn harness_with_provider(
    provider: Arc<dyn AuthProvider>,
    logger: RejectedRequestLogger,
) -> Harness {
    let logger = Arc::new(logger);
    let reached = Arc::new(AtomicUsize::new(0));
    let handler = {
        let reached = reached.clone();
        move |body: Bytes| {
            let reached = reached.clone();
            async move {
                reached.fetch_add(1, Ordering::SeqCst);
                body.len().to_string()
            }
        }
    };
    let app = Router::new()
        .route(INSERT_PROCESS, post(handler.clone()))
        .route("/ingestion/insert_stream", post(handler.clone()))
        .route(OTLP_LOGS, post(handler.clone()))
        .route("/ingestion/otlp/v1/metrics", post(handler.clone()))
        .route("/ingestion/otlp/v1/traces", post(handler))
        .layer(middleware::from_fn(move |req, next| {
            ingestion_auth_middleware(provider.clone(), logger.clone(), req, next)
        }));
    Harness { app, reached }
}

fn enable_logs() -> micromegas_tracing::test_utils::InMemoryTracingGuard {
    let guard = init_in_memory_tracing();
    set_max_level(LevelFilter::Trace);
    guard
}

fn cbor<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut out = vec![];
    ciborium::into_writer(value, &mut out).expect("encode cbor");
    out
}

fn process_info(process_id: uuid::Uuid) -> ProcessInfo {
    ProcessInfo {
        process_id,
        exe: "/usr/bin/game".into(),
        username: "alice".into(),
        realname: "Alice".into(),
        computer: "workstation-1".into(),
        distro: "linux".into(),
        cpu_brand: "cpu".into(),
        tsc_frequency: 1,
        start_time: chrono::Utc::now(),
        start_ticks: 0,
        parent_process_id: Some(uuid::Uuid::new_v4()),
        properties: HashMap::from([
            ("platform-name".to_owned(), "Linux".to_owned()),
            ("build-version".to_owned(), "1.2.3".to_owned()),
            ("engine-version".to_owned(), "5.4".to_owned()),
            ("build-config".to_owned(), "Shipping".to_owned()),
            ("build-target".to_owned(), "Game".to_owned()),
            ("branch-name".to_owned(), "main".to_owned()),
            ("commit".to_owned(), "abc123".to_owned()),
            (
                "command-line".to_owned(),
                "--token=cmdline-secret".to_owned(),
            ),
            ("exe_args".to_owned(), "--password=args-secret".to_owned()),
            ("other".to_owned(), "other-value".to_owned()),
        ]),
    }
}

fn rejected_request(
    uri: &str,
    ip: &str,
    body: Body,
    content_length: Option<usize>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer wrong-key")
        .header("x-forwarded-for", ip)
        .header(header::USER_AGENT, TEST_USER_AGENT);
    if let Some(len) = content_length {
        builder = builder.header(header::CONTENT_LENGTH, len);
    }
    builder.body(body).expect("build request")
}

fn bytes_request(ip: &str, bytes: Vec<u8>) -> Request<Body> {
    let len = bytes.len();
    rejected_request(INSERT_PROCESS, ip, Body::from(bytes), Some(len))
}

async fn send(h: &Harness, req: Request<Body>) -> StatusCode {
    h.app.clone().oneshot(req).await.expect("response").status()
}

fn lines_for<'a>(logs: &'a [CapturedLog], ip: &str, containing: &str) -> Vec<&'a CapturedLog> {
    logs.iter()
        .filter(|l| l.property("client_ip") == Some(ip) && l.msg.contains(containing))
        .collect()
}

#[tokio::test]
#[serial]
async fn valid_process_info_logs_every_claim_and_no_command_line() {
    let guard = enable_logs();
    let h = harness();
    let id = uuid::Uuid::new_v4();
    let info = process_info(id);
    let status = send(&h, bytes_request("198.51.100.1", cbor(&info))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.reached.load(Ordering::SeqCst), 0);

    let logs = collect_logs(&guard.sink);
    let line = single_line(&logs, "198.51.100.1");
    assert_rejection_context(line);
    assert_eq!(line.property("path"), Some(INSERT_PROCESS));
    assert_eq!(line.property("inspection"), None);
    assert_eq!(
        line.property("claimed_process_id"),
        Some(id.to_string().as_str())
    );
    assert_eq!(line.property("claimed_exe"), Some("/usr/bin/game"));
    assert_eq!(line.property("claimed_username"), Some("alice"));
    assert_eq!(line.property("claimed_computer"), Some("workstation-1"));
    assert_eq!(
        line.property("claimed_parent_process_id"),
        Some(info.parent_process_id.unwrap().to_string().as_str())
    );
    assert!(!line.property("claimed_start_time").unwrap().is_empty());
    assert_eq!(line.property("claimed_platform_name"), Some("Linux"));
    assert_eq!(line.property("claimed_build_version"), Some("1.2.3"));
    assert_eq!(line.property("claimed_engine_version"), Some("5.4"));
    assert_eq!(line.property("claimed_build_config"), Some("Shipping"));
    assert_eq!(line.property("claimed_build_target"), Some("Game"));
    assert_eq!(line.property("claimed_branch_name"), Some("main"));
    assert_eq!(line.property("claimed_commit"), Some("abc123"));
    for log in &logs {
        for (_, v) in &log.properties {
            assert!(!v.contains("cmdline-secret"));
            assert!(!v.contains("args-secret"));
            assert!(!v.contains("other-value"));
        }
        assert!(!log.msg.contains("secret"));
    }
}

#[tokio::test]
#[serial]
async fn log_injection_payloads_are_sanitized_and_bounded() {
    let guard = enable_logs();
    let h = harness();
    let mut info = process_info(uuid::Uuid::new_v4());
    info.exe = "evil\n[auth_success] forged \u{202E}line".into();
    info.username = "u".repeat(10_000);
    let status = send(&h, bytes_request("198.51.100.2", cbor(&info))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let logs = collect_logs(&guard.sink);
    let line = single_line(&logs, "198.51.100.2");
    assert_eq!(
        line.property("claimed_exe"),
        Some("evil?[auth_success] forged ?line")
    );
    let username = line.property("claimed_username").unwrap();
    assert_eq!(username.chars().count(), 128);
    assert!(username.ends_with('…'));
    assert!(logs.iter().all(|l| !l.msg.contains('\n')));
    assert!(logs.iter().all(|l| !l.msg.starts_with("[auth_success]")));
}

#[tokio::test]
#[serial]
async fn missing_content_length_is_not_inspected_and_body_is_not_read() {
    let guard = enable_logs();
    let h = harness();
    let polled = Arc::new(AtomicBool::new(false));
    let stream = {
        let polled = polled.clone();
        futures::stream::once(async move {
            polled.store(true, Ordering::SeqCst);
            Ok::<_, std::io::Error>(Bytes::from_static(b"x"))
        })
    };
    let req = rejected_request(
        INSERT_PROCESS,
        "198.51.100.3",
        Body::from_stream(stream),
        None,
    );
    assert_eq!(send(&h, req).await, StatusCode::UNAUTHORIZED);
    assert!(!polled.load(Ordering::SeqCst));

    let logs = collect_logs(&guard.sink);
    assert_eq!(
        inspection(&logs, "198.51.100.3").as_deref(),
        Some("missing_content_length")
    );
    assert_rejection_context(single_line(&logs, "198.51.100.3"));
}

#[tokio::test]
#[serial]
async fn oversized_content_length_is_not_inspected() {
    let guard = enable_logs();
    let h = harness();
    let req = rejected_request(
        INSERT_PROCESS,
        "198.51.100.4",
        Body::empty(),
        Some(64 * 1024 + 1),
    );
    assert_eq!(send(&h, req).await, StatusCode::UNAUTHORIZED);

    let logs = collect_logs(&guard.sink);
    let line = single_line(&logs, "198.51.100.4");
    assert_eq!(line.property("inspection"), Some("oversize"));
    assert_eq!(line.property("content_length"), Some("65537"));
}

#[tokio::test]
#[serial]
async fn body_stream_error_logs_read_error() {
    let guard = enable_logs();
    let h = harness();
    let stream = futures::stream::iter(vec![
        Ok(Bytes::from_static(b"ab")),
        Err(std::io::Error::other("boom")),
    ]);
    let req = rejected_request(
        INSERT_PROCESS,
        "198.51.100.5",
        Body::from_stream(stream),
        Some(10),
    );
    assert_eq!(send(&h, req).await, StatusCode::UNAUTHORIZED);

    let logs = collect_logs(&guard.sink);
    assert_eq!(
        inspection(&logs, "198.51.100.5").as_deref(),
        Some("read_error")
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn stalled_body_logs_read_timeout() {
    let guard = enable_logs();
    let h = harness();
    let stream = futures::stream::pending::<Result<Bytes, std::io::Error>>();
    let req = rejected_request(
        INSERT_PROCESS,
        "198.51.100.6",
        Body::from_stream(stream),
        Some(10),
    );
    assert_eq!(send(&h, req).await, StatusCode::UNAUTHORIZED);

    let logs = collect_logs(&guard.sink);
    assert_eq!(
        inspection(&logs, "198.51.100.6").as_deref(),
        Some("read_timeout")
    );
}

#[tokio::test]
#[serial]
async fn undecodable_insert_process_bodies_are_not_inspected() {
    let guard = enable_logs();
    let h = harness();
    let full = cbor(&process_info(uuid::Uuid::new_v4()));
    let truncated = full[..full.len() / 2].to_vec();
    assert_eq!(
        send(&h, bytes_request("198.51.100.7", truncated)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &h,
            bytes_request("198.51.100.8", b"{\"not\": \"cbor\"}".to_vec())
        )
        .await,
        StatusCode::UNAUTHORIZED
    );

    let logs = collect_logs(&guard.sink);
    for ip in ["198.51.100.7", "198.51.100.8"] {
        assert_eq!(
            inspection(&logs, ip).as_deref(),
            Some("undecodable"),
            "{ip}"
        );
        assert_eq!(single_line(&logs, ip).property("claimed_exe"), None);
    }
}

#[tokio::test]
#[serial]
async fn repeated_caller_is_rate_limited_per_process_id() {
    let guard = enable_logs();
    let h = harness();
    let first = cbor(&process_info(uuid::Uuid::new_v4()));
    let second = cbor(&process_info(uuid::Uuid::new_v4()));
    for body in [first.clone(), first, second] {
        assert_eq!(
            send(&h, bytes_request("198.51.100.9", body)).await,
            StatusCode::UNAUTHORIZED
        );
    }

    let logs = collect_logs(&guard.sink);
    let lines = lines_for(&logs, "198.51.100.9", "");
    assert_eq!(lines.len(), 3);
    // The repeat is still logged, without the claims.
    assert!(lines[0].property("claimed_process_id").is_some());
    assert_eq!(lines[1].property("claimed_process_id"), None);
    assert_eq!(lines[1].property("inspection"), None);
    assert_rejection_context(lines[1]);
    assert!(lines[2].property("claimed_process_id").is_some());
    assert_ne!(
        lines[0].property("claimed_process_id"),
        lines[2].property("claimed_process_id")
    );
}

#[tokio::test]
#[serial]
async fn partial_body_fills_present_fields_only() {
    #[derive(serde::Serialize)]
    struct Partial {
        process_id: String,
        exe: String,
    }
    let guard = enable_logs();
    let h = harness();
    let body = cbor(&Partial {
        process_id: "not-a-uuid".into(),
        exe: "game".into(),
    });
    assert_eq!(
        send(&h, bytes_request("198.51.100.10", body)).await,
        StatusCode::UNAUTHORIZED
    );

    let logs = collect_logs(&guard.sink);
    let line = single_line(&logs, "198.51.100.10");
    assert_eq!(line.property("claimed_process_id"), Some("not-a-uuid"));
    assert_eq!(line.property("claimed_exe"), Some("game"));
    assert_eq!(line.property("claimed_username"), None);
    assert_eq!(line.property("claimed_commit"), None);
}

#[tokio::test]
#[serial]
async fn authenticated_insert_process_reaches_handler_with_body_intact() {
    let guard = enable_logs();
    let h = harness();
    let body = cbor(&process_info(uuid::Uuid::new_v4()));
    let len = body.len();
    let req = Request::builder()
        .method("POST")
        .uri(INSERT_PROCESS)
        .header(header::AUTHORIZATION, format!("Bearer {VALID_KEY}"))
        .header("x-forwarded-for", "198.51.100.11")
        .header(header::CONTENT_LENGTH, len)
        .body(Body::from(body))
        .expect("build request");
    let response = h.app.clone().oneshot(req).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let echoed = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    assert_eq!(&echoed[..], len.to_string().as_bytes());
    assert_eq!(h.reached.load(Ordering::SeqCst), 1);

    let logs = collect_logs(&guard.sink);
    assert!(lines_for(&logs, "198.51.100.11", "").is_empty());
    assert!(logs.iter().all(|l| !l.msg.starts_with("[auth_failure]")));
}

#[tokio::test]
#[serial]
async fn rejected_insert_stream_logs_only_the_generic_line() {
    let guard = enable_logs();
    let h = harness();
    let body = cbor(&process_info(uuid::Uuid::new_v4()));
    let len = body.len();
    let req = rejected_request(
        "/ingestion/insert_stream",
        "198.51.100.12",
        Body::from(body),
        Some(len),
    );
    assert_eq!(send(&h, req).await, StatusCode::UNAUTHORIZED);

    let logs = collect_logs(&guard.sink);
    let lines = lines_for(&logs, "198.51.100.12", "");
    assert_eq!(lines.len(), 1);
    assert!(lines[0].msg.starts_with("[auth_failure] "));
    assert_eq!(lines[0].property("path"), Some("/ingestion/insert_stream"));
}

struct RejectingProvider;

#[async_trait::async_trait]
impl AuthProvider for RejectingProvider {
    fn name(&self) -> &'static str {
        "rejecting"
    }

    async fn validate_request(&self, _parts: &dyn RequestParts) -> anyhow::Result<AuthContext> {
        anyhow::bail!("nope")
    }
}

#[tokio::test]
#[serial]
async fn check_auth_rejection_logs_client_ip_and_user_agent() {
    let guard = enable_logs();
    let provider: Arc<dyn AuthProvider> = Arc::new(RejectingProvider);
    let mut req = tonic::Request::new(());
    req.metadata_mut()
        .insert("x-forwarded-for", "198.51.100.13".parse().unwrap());
    req.metadata_mut()
        .insert("user-agent", "tonic/0.14".parse().unwrap());
    let status = check_auth(req, &provider).await.expect_err("rejected");
    assert_eq!(status.code(), tonic::Code::Unauthenticated);

    let logs = collect_logs(&guard.sink);
    let lines = lines_for(&logs, "198.51.100.13", "authentication failed:");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].property("user_agent"), Some("tonic/0.14"));
}

// OTLP routes

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::ResourceLogs;
use opentelemetry_proto::tonic::resource::v1::Resource;

const PROTOBUF: &str = "application/x-protobuf";
const JSON: &str = "application/json";

fn string_kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_owned(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.to_owned())),
        }),
        ..Default::default()
    }
}

fn logs_request(attrs: Vec<KeyValue>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: attrs,
                ..Default::default()
            }),
            ..Default::default()
        }],
    }
}

fn service_attrs(service: &str) -> Vec<KeyValue> {
    vec![
        string_kv("service.namespace", "shop"),
        string_kv("service.name", service),
        string_kv("host.name", "node-7"),
        string_kv("telemetry.sdk.language", "python"),
        string_kv("telemetry.sdk.version", "1.2.3"),
        string_kv("process.command_line", "--token=cmdline-secret"),
    ]
}

fn proto_body(attrs: Vec<KeyValue>) -> Vec<u8> {
    logs_request(attrs).encode_to_vec()
}

fn json_body(attrs: Vec<KeyValue>) -> Vec<u8> {
    serde_json::to_vec(&logs_request(attrs)).expect("encode json")
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(vec![], Compression::default());
    enc.write_all(bytes).expect("gzip write");
    enc.finish().expect("gzip finish")
}

fn otlp_request(
    ip: &str,
    content_type: &str,
    content_encoding: Option<&str>,
    body: Body,
    content_length: Option<usize>,
) -> Request<Body> {
    let mut req = rejected_request(OTLP_LOGS, ip, body, content_length);
    req.headers_mut()
        .insert(header::CONTENT_TYPE, content_type.parse().expect("header"));
    if let Some(enc) = content_encoding {
        req.headers_mut()
            .insert(header::CONTENT_ENCODING, enc.parse().expect("header"));
    }
    req
}

fn otlp_bytes(ip: &str, ct: &str, ce: Option<&str>, bytes: Vec<u8>) -> Request<Body> {
    let len = bytes.len();
    otlp_request(ip, ct, ce, Body::from(bytes), Some(len))
}

fn assert_service_claims(line: &CapturedLog, service: &str) {
    assert_eq!(
        line.property("claimed_exe"),
        Some(format!("shop/{service}").as_str())
    );
    assert_eq!(line.property("claimed_computer"), Some("node-7"));
    assert_eq!(line.property("claimed_sdk_language"), Some("python"));
    assert_eq!(line.property("claimed_sdk_version"), Some("1.2.3"));
}

fn assert_no_secrets(logs: &[CapturedLog]) {
    for log in logs {
        for (_, v) in &log.properties {
            assert!(!v.contains("cmdline-secret"));
            assert!(!v.contains("wrong-key"));
        }
        assert!(!log.msg.contains("secret"));
    }
}

fn single_line<'a>(logs: &'a [CapturedLog], ip: &str) -> &'a CapturedLog {
    let lines = lines_for(logs, ip, "");
    assert_eq!(lines.len(), 1, "{ip}");
    assert!(lines[0].msg.starts_with("[auth_failure] "), "{ip}");
    lines[0]
}

fn assert_rejection_context(line: &CapturedLog) {
    assert_eq!(line.msg, "[auth_failure] invalid API token");
    assert_eq!(line.property("method"), Some("POST"));
    assert_eq!(line.property("user_agent"), Some(TEST_USER_AGENT));
}

fn inspection(logs: &[CapturedLog], ip: &str) -> Option<String> {
    single_line(logs, ip)
        .property("inspection")
        .map(str::to_owned)
}

#[tokio::test]
#[serial]
async fn rejected_otlp_logs_claims_for_every_encoding() {
    let guard = enable_logs();
    let h = harness();
    let cases = [
        (
            "203.0.113.1",
            PROTOBUF,
            None,
            proto_body(service_attrs("a")),
        ),
        ("203.0.113.2", JSON, None, json_body(service_attrs("a"))),
        (
            "203.0.113.3",
            PROTOBUF,
            Some("gzip"),
            gzip(&proto_body(service_attrs("a"))),
        ),
        (
            "203.0.113.4",
            JSON,
            Some("gzip"),
            gzip(&json_body(service_attrs("a"))),
        ),
        (
            "203.0.113.5",
            PROTOBUF,
            Some("identity"),
            proto_body(service_attrs("a")),
        ),
        (
            "203.0.113.6",
            PROTOBUF,
            Some("X-GZIP"),
            gzip(&proto_body(service_attrs("a"))),
        ),
    ];
    for (ip, ct, ce, body) in cases {
        let status = send(&h, otlp_bytes(ip, ct, ce, body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{ip}");
    }
    assert_eq!(h.reached.load(Ordering::SeqCst), 0);

    let logs = collect_logs(&guard.sink);
    for n in 1..=6 {
        let ip = format!("203.0.113.{n}");
        let ip = ip.as_str();
        let line = single_line(&logs, ip);
        assert_service_claims(line, "a");
        assert_rejection_context(line);
        assert_eq!(line.property("path"), Some(OTLP_LOGS));
        assert_eq!(line.property("inspection"), None);
        assert!(line.property("content_type").is_some());
        assert!(line.property("content_length").is_some());
    }
    assert_no_secrets(&logs);
}

struct UnavailableProvider;

#[async_trait::async_trait]
impl AuthProvider for UnavailableProvider {
    fn name(&self) -> &'static str {
        "unavailable"
    }

    async fn validate_request(&self, _parts: &dyn RequestParts) -> anyhow::Result<AuthContext> {
        Err(ProviderUnavailable(anyhow::anyhow!("key store unreachable")).into())
    }
}

#[tokio::test]
#[serial]
async fn unavailable_provider_on_otlp_returns_503_without_warning() {
    let guard = enable_logs();
    let h = harness_with_provider(Arc::new(UnavailableProvider), RejectedRequestLogger::new());
    let ip = "203.0.113.40";
    let status = send(
        &h,
        otlp_bytes(ip, PROTOBUF, None, proto_body(service_attrs("a"))),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(h.reached.load(Ordering::SeqCst), 0);
    let logs = collect_logs(&guard.sink);
    assert!(lines_for(&logs, ip, "[auth_failure]").is_empty());
}

#[tokio::test]
#[serial]
async fn truncated_gzip_still_yields_decoded_claims() {
    let guard = enable_logs();
    let h = harness();
    let mut wire = gzip(&proto_body(service_attrs("a")));
    wire.truncate(wire.len() - 8);
    send(&h, otlp_bytes("203.0.113.10", PROTOBUF, Some("gzip"), wire)).await;

    let logs = collect_logs(&guard.sink);
    assert_service_claims(single_line(&logs, "203.0.113.10"), "a");
}

#[tokio::test]
#[serial]
async fn large_and_chunked_bodies_are_inspected() {
    let guard = enable_logs();
    let h = harness();
    let mut attrs = service_attrs("a");
    attrs.push(string_kv("padding", &"x".repeat(200 * 1024)));
    let body = proto_body(attrs);
    assert!(body.len() > 64 * 1024);

    send(&h, otlp_bytes("203.0.113.11", PROTOBUF, None, body.clone())).await;
    let chunks: Vec<Result<Bytes, std::io::Error>> = body
        .chunks(8 * 1024)
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    let chunked = otlp_request(
        "203.0.113.12",
        PROTOBUF,
        None,
        Body::from_stream(futures::stream::iter(chunks)),
        None,
    );
    send(&h, chunked).await;

    let logs = collect_logs(&guard.sink);
    for ip in ["203.0.113.11", "203.0.113.12"] {
        assert_service_claims(single_line(&logs, ip), "a");
    }
}

#[tokio::test]
#[serial]
async fn unsupported_encodings_and_content_types_are_not_inspected() {
    let guard = enable_logs();
    let h = harness();
    let body = proto_body(service_attrs("a"));
    send(
        &h,
        otlp_bytes("203.0.113.20", PROTOBUF, Some("zstd"), body.clone()),
    )
    .await;
    send(
        &h,
        otlp_bytes(
            "203.0.113.21",
            PROTOBUF,
            Some("gzip, identity"),
            body.clone(),
        ),
    )
    .await;
    send(&h, otlp_bytes("203.0.113.22", "text/plain", None, body)).await;
    send(
        &h,
        otlp_bytes("203.0.113.23", PROTOBUF, None, b"garbage".to_vec()),
    )
    .await;
    send(
        &h,
        otlp_bytes("203.0.113.24", PROTOBUF, Some("gzip"), b"not gzip".to_vec()),
    )
    .await;

    let logs = collect_logs(&guard.sink);
    let reason = |ip| inspection(&logs, ip);
    assert_eq!(
        reason("203.0.113.20").as_deref(),
        Some("unsupported_encoding")
    );
    assert_eq!(
        reason("203.0.113.21").as_deref(),
        Some("unsupported_encoding")
    );
    assert_eq!(
        reason("203.0.113.22").as_deref(),
        Some("unsupported_content_type")
    );
    assert_eq!(reason("203.0.113.23").as_deref(), Some("no_resource"));
    assert_eq!(reason("203.0.113.24").as_deref(), Some("undecodable"));
    assert_eq!(
        single_line(&logs, "203.0.113.24").property("path"),
        Some(OTLP_LOGS)
    );
    for ip in ["203.0.113.20", "203.0.113.21", "203.0.113.22"] {
        assert_eq!(single_line(&logs, ip).property("claimed_exe"), None);
    }
    for n in 20..=24 {
        assert_rejection_context(single_line(&logs, &format!("203.0.113.{n}")));
    }
}

#[tokio::test]
#[serial]
async fn dedup_is_per_sender_not_per_ip() {
    let guard = enable_logs();
    let h = harness();
    let ip = "203.0.113.30";
    for service in ["a", "b", "a"] {
        send(
            &h,
            otlp_bytes(ip, PROTOBUF, None, proto_body(service_attrs(service))),
        )
        .await;
    }

    let logs = collect_logs(&guard.sink);
    let lines = lines_for(&logs, ip, "");
    assert_eq!(lines.len(), 3);
    assert_service_claims(lines[0], "a");
    assert_service_claims(lines[1], "b");
    // The repeat is still logged, without the claims.
    assert_eq!(lines[2].property("claimed_exe"), None);
    assert_eq!(lines[2].property("inspection"), None);
    assert_rejection_context(lines[2]);
    assert_eq!(lines[2].property("path"), Some(OTLP_LOGS));
}

#[tokio::test]
#[serial]
async fn exhausted_inspection_budget_logs_busy() {
    let guard = enable_logs();
    let h = harness_with_logger(RejectedRequestLogger::with_max_concurrent_inspections(0));
    let otlp = otlp_bytes(
        "203.0.113.40",
        PROTOBUF,
        None,
        proto_body(service_attrs("a")),
    );
    let insert_process = bytes_request("203.0.113.41", cbor(&process_info(uuid::Uuid::new_v4())));
    for req in [otlp, insert_process] {
        assert_eq!(send(&h, req).await, StatusCode::UNAUTHORIZED);
    }

    let logs = collect_logs(&guard.sink);
    for ip in ["203.0.113.40", "203.0.113.41"] {
        assert_eq!(inspection(&logs, ip).as_deref(), Some("busy"), "{ip}");
        assert_eq!(single_line(&logs, ip).property("claimed_exe"), None);
    }
}

#[tokio::test]
#[serial]
async fn otlp_claims_are_sanitized() {
    let guard = enable_logs();
    let h = harness();
    let attrs = vec![string_kv(
        "service.name",
        "evil\n[auth_success] forged \u{202E}x",
    )];
    send(
        &h,
        otlp_bytes("203.0.113.50", PROTOBUF, None, proto_body(attrs)),
    )
    .await;

    let logs = collect_logs(&guard.sink);
    assert_eq!(
        single_line(&logs, "203.0.113.50").property("claimed_exe"),
        Some("evil?[auth_success] forged ?x")
    );
    assert!(logs.iter().all(|l| !l.msg.contains('\n')));
}

#[tokio::test]
#[serial]
async fn accepted_otlp_request_logs_nothing_extra_and_body_is_intact() {
    let guard = enable_logs();
    let h = harness();
    let body = proto_body(service_attrs("a"));
    let len = body.len();
    let mut req = otlp_bytes("203.0.113.60", PROTOBUF, None, body);
    req.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {VALID_KEY}").parse().expect("header"),
    );
    let resp = h.app.clone().oneshot(req).await.expect("response");
    assert_eq!(resp.status(), StatusCode::OK);
    let echoed = axum::body::to_bytes(resp.into_body(), 1024)
        .await
        .expect("body");
    assert_eq!(echoed.as_ref(), len.to_string().as_bytes());
    assert_eq!(h.reached.load(Ordering::SeqCst), 1);

    let logs = collect_logs(&guard.sink);
    assert!(lines_for(&logs, "203.0.113.60", "").is_empty());
}
