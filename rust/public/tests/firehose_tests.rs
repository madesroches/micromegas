// HTTP-level tests for `micromegas::servers::firehose` — the Kinesis Data Firehose HTTP
// Endpoint Delivery route for OTLP metrics (CloudWatch Metric Streams).
//
// Uses `tower::ServiceExt::oneshot` against a lazily-connected Postgres pool + in-memory
// object store (never actually touched, since every case here either fails auth before
// the handler or sends zero records), matching the pattern in
// `rust/ingestion/tests/readiness.rs`. A DB-backed full-ingest test is `#[ignore]`d.

use anyhow::Result;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use flate2::Compression;
use flate2::write::GzEncoder;
use micromegas::servers::firehose::firehose_router;
use micromegas::servers::write_audience::StampingConfig;
use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas_auth::types::{AuthContext, AuthProvider, AuthType, RequestParts};
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

/// `StampingConfig` with `require_write_audience` off -- the default, and what every case here
/// but the new differential one below exercises.
fn stamping_off() -> Arc<StampingConfig> {
    Arc::new(StampingConfig::new(false))
}

/// A stub `AuthProvider` returning a fixed `AuthContext` carrying `bound_audience: Some(..)`
/// (AbAC Stage 5, #1373). `ApiKeyAuthProvider`'s own keys always carry `bound_audience: None`
/// (`auth/src/api_key.rs`) -- only a live `DbApiKeyAuthProvider` (needs Postgres) ever produces
/// `Some(..)` -- so this is the only way to exercise a bound-audience credential on this
/// DB-less harness. Mirrors `public/tests/read_policy_threading_tests.rs`'s `GroupsAuthProvider`
/// precedent for a minimal stub `AuthProvider`.
#[derive(Debug)]
struct BoundAudienceProvider {
    audience: &'static str,
}

#[async_trait]
impl AuthProvider for BoundAudienceProvider {
    async fn validate_request(&self, _parts: &dyn RequestParts) -> Result<AuthContext> {
        Ok(AuthContext {
            subject: "bound-audience-test".to_string(),
            email: None,
            issuer: "api_key".to_string(),
            audience: None,
            expires_at: None,
            auth_type: AuthType::ApiKey,
            is_admin: false,
            allow_delegation: false,
            bound_audience: Some(self.audience.to_string()),
            read_audiences: vec![],
            groups: vec![],
        })
    }
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
    let app = firehose_router(service, Some(provider), stamping_off());

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
    let app = firehose_router(service, Some(provider), stamping_off());

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
    let app = firehose_router(service, Some(provider), stamping_off());

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
    let app = firehose_router(service, None, stamping_off());

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

#[tokio::test]
async fn require_write_audience_differentiates_bound_from_audience_less_credential() {
    // AbAC Stage 5 (#1373, §5): with the knob on, only a credential whose `AuthContext` carries
    // a `bound_audience` gets a clean ack -- proving `firehose_auth_middleware` no longer
    // discards the validated context (its old `Ok(_ctx) => { ... }` arm dropped it entirely).
    // Zero records both times: this harness's lazy pool points at an unreachable database, so
    // every case must stop before touching it.
    let stamping = Arc::new(StampingConfig::new(true));

    let bound_provider: Arc<dyn AuthProvider> =
        Arc::new(BoundAudienceProvider { audience: "team-a" });
    let bound_app = firehose_router(make_test_service(), Some(bound_provider), stamping.clone());
    let bound_request = Request::builder()
        .method("POST")
        .uri(ENDPOINT)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Amz-Firehose-Request-Id", "req-bound")
        .header(
            "X-Amz-Firehose-Access-Key",
            "irrelevant-for-this-stub-provider",
        )
        .body(Body::from(empty_records_body("req-bound")))
        .expect("build request");
    let bound_response = bound_app
        .oneshot(bound_request)
        .await
        .expect("call service");
    assert_eq!(bound_response.status(), StatusCode::OK);
    let bound_json = response_json(bound_response).await;
    assert!(
        bound_json.get("errorMessage").is_none(),
        "a credential with a bound audience must get a clean ack: {bound_json:?}"
    );

    let audience_less_provider = make_auth_provider();
    let audience_less_app =
        firehose_router(make_test_service(), Some(audience_less_provider), stamping);
    let audience_less_request = Request::builder()
        .method("POST")
        .uri(ENDPOINT)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Amz-Firehose-Request-Id", "req-unstamped")
        .header("X-Amz-Firehose-Access-Key", ACCESS_KEY)
        .body(Body::from(empty_records_body("req-unstamped")))
        .expect("build request");
    let audience_less_response = audience_less_app
        .oneshot(audience_less_request)
        .await
        .expect("call service");
    assert!(
        audience_less_response.status().is_client_error(),
        "an audience-less credential must be rejected when REQUIRE_WRITE_AUDIENCE is set: {:?}",
        audience_less_response.status()
    );
    let audience_less_json = response_json(audience_less_response).await;
    assert!(audience_less_json["errorMessage"].is_string());
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
    let app = firehose_router(service, Some(provider), stamping_off());

    let make_request = |name: &str, value: i64| -> ExportMetricsServiceRequest {
        ExportMetricsServiceRequest {
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
        }
    };
    // Real Firehose records are always length-delimited-framed (CloudWatch Metric Streams'
    // OpenTelemetry 1.0.0 output format), even for a single message per record.
    let make_record = |name: &str, value: i64| -> Vec<u8> {
        make_request(name, value).encode_length_delimited_to_vec()
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

    let response = app.clone().oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::OK);

    // A single Firehose record can also pack two length-delimited messages back to back
    // (issue #1381) — assert both decode and land as separate blocks.
    let mut packed_record = make_request("metric.c", 3).encode_length_delimited_to_vec();
    packed_record.extend(make_request("metric.d", 4).encode_length_delimited_to_vec());
    let packed_body = format!(
        r#"{{"requestId":"req-live-packed","timestamp":1,"records":[{{"data":"{}"}}]}}"#,
        engine.encode(packed_record)
    );

    let packed_request = Request::builder()
        .method("POST")
        .uri(ENDPOINT)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Amz-Firehose-Request-Id", "req-live-packed")
        .header("X-Amz-Firehose-Access-Key", ACCESS_KEY)
        .body(Body::from(packed_body))
        .expect("build request");

    let packed_response = app.oneshot(packed_request).await.expect("call service");
    assert_eq!(packed_response.status(), StatusCode::OK);
}
