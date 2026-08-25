// Lightweight (no live database) tests for the process-audience cache added to
// `check_process_audience_conflict` (perf follow-up to AbAC Stage 5, #1373). The service is
// built with an unreachable `PgPool` (lazy connect, per `readiness.rs`'s precedent), so any test
// that reaches the database at all fails -- letting these tests prove a cache hit skips the
// `SELECT` entirely, rather than merely asserting a return value that a real DB call could also
// have produced.
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_telemetry::blob_storage::BlobStorage;
use object_store::memory::InMemory;
use object_store::path::Path;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

fn make_test_service() -> WebIngestionService {
    let blob_store = Arc::new(InMemory::new());
    let blob_storage = Arc::new(BlobStorage::new(blob_store, Path::default()));
    // Short explicit acquire_timeout (sqlx's default is 30s) -- see
    // `rust/auth/tests/db_api_key_tests.rs`'s `unreachable_pool` for the same pattern.
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible");
    WebIngestionService::new_for_test(DataLakeConnection::new(pool, blob_storage))
}

/// A cache hit (same `process_id`, same audience as the one already confirmed conflict-free)
/// must return `Ok` without ever touching the database -- the database here is unreachable, so a
/// `SELECT` attempt would surface as an `Err`.
#[tokio::test]
async fn cache_hit_skips_the_database() {
    let service = make_test_service();
    let process_id = Uuid::new_v4();
    let audience = WriteAudience::new("team-a").expect("valid audience");

    service.prime_process_audience_cache_for_test(process_id, audience.clone());

    service
        .check_process_audience_conflict_for_test(process_id, &audience)
        .await
        .expect("a cache hit must succeed without querying the unreachable database");
}

/// A cache miss (audience differs from the one cached for this `process_id`) must still fall
/// through to the real, database-backed check -- proving the cache-hit path above is actually
/// being skipped, not that the guard is unconditionally a no-op. Against the unreachable
/// database, that DB attempt surfaces as an `Err`.
#[tokio::test]
async fn cache_miss_falls_through_to_the_database() {
    let service = make_test_service();
    let process_id = Uuid::new_v4();
    let cached_audience = WriteAudience::new("team-a").expect("valid audience");
    let incoming_audience = WriteAudience::new("team-b").expect("valid audience");

    service.prime_process_audience_cache_for_test(process_id, cached_audience);

    let result = service
        .check_process_audience_conflict_for_test(process_id, &incoming_audience)
        .await;
    assert!(
        result.is_err(),
        "a cache miss must attempt the database-backed check, which must fail against an \
         unreachable database -- got {result:?}"
    );
}
