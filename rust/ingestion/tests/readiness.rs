// Integration tests for the ingestion readiness probe.
// Tests that require a live DB/blob store are marked #[ignore].
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_telemetry::blob_storage::BlobStorage;
use object_store::memory::InMemory;
use object_store::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

#[tokio::test]
async fn cache_hit_returns_true_without_probing() {
    let service = make_test_service();
    service.set_ready_until(Instant::now() + Duration::from_secs(60));
    assert!(
        service.check_ready().await,
        "should return true on cache hit without probing dependencies"
    );
}

// Requires MICROMEGAS_SQL_CONNECTION_STRING (and object store env vars) to point at a live stack.
#[ignore]
#[tokio::test]
async fn check_ready_returns_true_when_dependencies_healthy() {
    let service = WebIngestionService::from_env()
        .await
        .expect("creating service from env");
    assert!(
        service.check_ready().await,
        "should return true when DB and blob storage are reachable"
    );
}
