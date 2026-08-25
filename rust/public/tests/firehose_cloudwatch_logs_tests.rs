// HTTP-level tests for `micromegas::servers::firehose_cloudwatch_logs` — the Kinesis Data
// Firehose HTTP Endpoint Delivery route for CloudWatch Logs subscription-filter delivery.
//
// Uses `tower::ServiceExt::oneshot` against a lazily-connected Postgres pool + in-memory
// object store (never actually touched, since every case here either fails auth before
// the handler, sends zero/control-message records, or is #[ignore]d), matching the
// pattern in `firehose_tests.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use flate2::Compression;
use flate2::write::GzEncoder;
use micromegas::servers::firehose_cloudwatch_logs::firehose_router;
use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas_auth::types::AuthProvider;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_telemetry::blob_storage::BlobStorage;
use object_store::memory::InMemory;
use object_store::path::Path;
use std::io::Write;
use std::sync::Arc;
use tower::ServiceExt;

const ACCESS_KEY: &str = "test-cw-logs-firehose-access-key";
const ENDPOINT: &str = "/ingestion/cloudwatch/v1/logs/firehose";

fn make_test_service() -> Arc<WebIngestionService> {
    let blob_store = Arc::new(InMemory::new());
    let blob_storage = Arc::new(BlobStorage::new(blob_store, Path::default()));
    let pool = sqlx::PgPool::connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible");
    Arc::new(WebIngestionService::new(DataLakeConnection::new(
        pool,
        blob_storage,
    )))
}

fn make_auth_provider() -> Arc<dyn AuthProvider> {
    let json = format!(r#"[{{"name": "cw-logs-firehose-test", "key": "{ACCESS_KEY}"}}]"#);
    let keyring = parse_key_ring(&json).expect("parse keyring");
    Arc::new(ApiKeyAuthProvider::new(keyring))
}

fn empty_records_body(request_id: &str) -> String {
    format!(r#"{{"requestId":"{request_id}","timestamp":1578090901599,"records":[]}}"#)
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).expect("writing to gzip encoder");
    encoder.finish().expect("finishing gzip stream")
}

fn control_message_record() -> Vec<u8> {
    let json = r#"{"messageType":"CONTROL_MESSAGE","owner":"CloudwatchLogs","logGroup":"","logStream":"","subscriptionFilters":[],"logEvents":[{"id":"1","timestamp":1510109208016,"message":"CWL CONTROL MESSAGE: Checking health of destination Firehose."}]}"#;
    gzip(json.as_bytes())
}

fn envelope_with_records(request_id: &str, records: &[Vec<u8>]) -> String {
    use base64::Engine as _;
    let records_json: Vec<String> = records
        .iter()
        .map(|r| {
            format!(
                r#"{{"data":"{}"}}"#,
                base64::engine::general_purpose::STANDARD.encode(r)
            )
        })
        .collect();
    format!(
        r#"{{"requestId":"{request_id}","timestamp":1578090901599,"records":[{}]}}"#,
        records_json.join(",")
    )
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("reading response body");
    serde_json::from_slice(&body).expect("parsing response body as json")
}

#[tokio::test]
async fn missing_access_key_is_rejected_with_firehose_error_shape() {
    let service = make_test_service();
    let provider = make_auth_provider();
    let app = firehose_router(service, Some(provider));

    let request = Request::builder()
        .method("POST")
        .uri(ENDPOINT)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Amz-Firehose-Request-Id", "req-missing-key")
        .body(Body::from(empty_records_body("req-missing-key")))
        .expect("build request");

    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let json = response_json(response).await;
    assert_eq!(json["requestId"], "req-missing-key");
    assert!(json["timestamp"].is_number());
    assert!(json["errorMessage"].is_string());
}

#[tokio::test]
async fn wrong_access_key_is_rejected_with_firehose_error_shape() {
    let service = make_test_service();
    let provider = make_auth_provider();
    let app = firehose_router(service, Some(provider));

    let request = Request::builder()
        .method("POST")
        .uri(ENDPOINT)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Amz-Firehose-Request-Id", "req-wrong-key")
        .header("X-Amz-Firehose-Access-Key", "not-the-right-key")
        .body(Body::from(empty_records_body("req-wrong-key")))
        .expect("build request");

    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let json = response_json(response).await;
    assert_eq!(json["requestId"], "req-wrong-key");
    assert!(json["errorMessage"].is_string());
}

#[tokio::test]
async fn valid_key_control_message_returns_ack_with_no_db_write() {
    let service = make_test_service();
    let provider = make_auth_provider();
    let app = firehose_router(service, Some(provider));

    let body = envelope_with_records("req-control", &[control_message_record()]);
    let request = Request::builder()
        .method("POST")
        .uri(ENDPOINT)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Amz-Firehose-Request-Id", "req-control")
        .header("X-Amz-Firehose-Access-Key", ACCESS_KEY)
        .body(Body::from(body))
        .expect("build request");

    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response).await;
    assert_eq!(json["requestId"], "req-control");
    assert!(
        json.get("errorMessage").is_none(),
        "success response must not carry errorMessage: {json:?}"
    );
}

#[tokio::test]
async fn valid_key_gzip_empty_records_returns_ack_with_no_error_message() {
    let service = make_test_service();
    let provider = make_auth_provider();
    let app = firehose_router(service, Some(provider));

    let body = gzip(empty_records_body("req-gzip-ok").as_bytes());
    let request = Request::builder()
        .method("POST")
        .uri(ENDPOINT)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_ENCODING, "gzip")
        .header("X-Amz-Firehose-Request-Id", "req-gzip-ok")
        .header("X-Amz-Firehose-Access-Key", ACCESS_KEY)
        .body(Body::from(body))
        .expect("build request");

    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response).await;
    assert_eq!(json["requestId"], "req-gzip-ok");
    assert!(
        json.get("errorMessage").is_none(),
        "success response must not carry errorMessage: {json:?}"
    );
}

#[tokio::test]
async fn dev_mode_no_provider_accepts_request_without_access_key() {
    let service = make_test_service();
    let app = firehose_router(service, None);

    let request = Request::builder()
        .method("POST")
        .uri(ENDPOINT)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Amz-Firehose-Request-Id", "req-dev-mode")
        .body(Body::from(empty_records_body("req-dev-mode")))
        .expect("build request");

    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response).await;
    assert_eq!(json["requestId"], "req-dev-mode");
}

// Requires MICROMEGAS_SQL_CONNECTION_STRING (and object store env vars) to point at a
// live stack — records are actually written through to Postgres/object storage.
#[ignore]
#[tokio::test]
async fn full_data_message_ingest_succeeds_against_a_live_stack() {
    let service = WebIngestionService::from_env()
        .await
        .expect("creating service from env");
    let provider = make_auth_provider();
    let app = firehose_router(service, Some(provider));

    let json = r#"{"messageType":"DATA_MESSAGE","owner":"123456789012","logGroup":"/ecs/live-e2e","logStream":"live-e2e-stream","subscriptionFilters":["f"],"logEvents":[{"id":"evt-1","timestamp":1700000000000,"message":"live e2e line one"},{"id":"evt-2","timestamp":1700000000100,"message":"live e2e line two"}]}"#;
    let record = gzip(json.as_bytes());
    let body = envelope_with_records("req-live", &[record]);

    let request = Request::builder()
        .method("POST")
        .uri(ENDPOINT)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Amz-Firehose-Request-Id", "req-live")
        .header("X-Amz-Firehose-Access-Key", ACCESS_KEY)
        .body(Body::from(body))
        .expect("build request");

    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
}
