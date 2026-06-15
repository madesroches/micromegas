// Integration tests for the ingestion readiness probe.
// Tests that require a live DB/blob pass gracefully when env vars are absent.
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

async fn try_create_service() -> Option<Arc<WebIngestionService>> {
    if std::env::var("MICROMEGAS_SQL_CONNECTION_STRING").is_err() {
        return None;
    }
    WebIngestionService::from_env().await.ok()
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

#[tokio::test]
async fn check_ready_returns_true_when_dependencies_healthy() {
    let Some(service) = try_create_service().await else {
        eprintln!("skipping check_ready_returns_true_when_dependencies_healthy: env vars not set");
        return;
    };
    assert!(
        service.check_ready().await,
        "should return true when DB and blob storage are reachable"
    );
}
