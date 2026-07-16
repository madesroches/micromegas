// HTTP-level tests for `micromegas::servers::firehose` — the Kinesis Data Firehose HTTP
// Endpoint Delivery route for OTLP metrics (CloudWatch Metric Streams).
//
// Uses `tower::ServiceExt::oneshot` against a lazily-connected Postgres pool + in-memory
// object store (never actually touched, since every case here either fails auth before
// the handler or sends zero records), matching the pattern in
// `rust/ingestion/tests/readiness.rs`. A DB-backed full-ingest test is `#[ignore]`d.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use flate2::Compression;
use flate2::write::GzEncoder;
use micromegas::servers::firehose::firehose_router;
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

const ACCESS_KEY: &str = "test-firehose-access-key";
const ENDPOINT: &str = "/ingestion/otlp/v1/metrics/firehose";

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
    let json = format!(r#"[{{"name": "firehose-test", "key": "{ACCESS_KEY}"}}]"#);
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
    assert!(json["timestamp"].is_number());
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
async fn full_multi_record_ingest_succeeds_against_a_live_stack() {
    use base64::Engine as _;
    use micromegas_otel_ingestion::proto::{
        AnyValue, ExportMetricsServiceRequest, KeyValue, Metric, Resource, ResourceMetrics,
        ScopeMetrics, any_value, metric,
    };
    use opentelemetry_proto::tonic::metrics::v1::{Gauge, NumberDataPoint, number_data_point};
    use prost::Message;

    let service = WebIngestionService::from_env()
        .await
        .expect("creating service from env");
    let provider = make_auth_provider();
    let app = firehose_router(service, Some(provider));

    let make_record = |name: &str, value: i64| -> Vec<u8> {
        let req = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".into(),
                        key_strindex: 0,
                        value: Some(AnyValue {
                            value: Some(any_value::Value::StringValue("firehose-e2e".to_string())),
                        }),
                    }],
                    dropped_attributes_count: 0,
                    entity_refs: vec![],
                }),
                scope_metrics: vec![ScopeMetrics {
                    scope: None,
                    metrics: vec![Metric {
                        name: name.to_string(),
                        description: String::new(),
                        unit: "1".to_string(),
                        metadata: vec![],
                        data: Some(metric::Data::Gauge(Gauge {
                            data_points: vec![NumberDataPoint {
                                attributes: vec![],
                                start_time_unix_nano: 0,
                                time_unix_nano: 1_700_000_000_000_000_000,
                                exemplars: vec![],
                                flags: 0,
                                value: Some(number_data_point::Value::AsInt(value)),
                            }],
                        })),
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        req.encode_to_vec()
    };

    let engine = base64::engine::general_purpose::STANDARD;
    let records_json = [
        format!(
            r#"{{"data":"{}"}}"#,
            engine.encode(make_record("metric.a", 1))
        ),
        format!(
            r#"{{"data":"{}"}}"#,
            engine.encode(make_record("metric.b", 2))
        ),
    ]
    .join(",");
    let body = format!(r#"{{"requestId":"req-live","timestamp":1,"records":[{records_json}]}}"#);

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
