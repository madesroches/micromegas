//! Tests for `micromegas_auth::db_api_key`.
//!
//! The "no DB" section below builds providers against a pool that is never
//! actually reachable (`connect_lazy` — the `firehose_tests.rs` trick — with an
//! explicit short `acquire_timeout`, since sqlx's default is 30s and some tests
//! here make two DB attempts). The `#[ignore]`d section needs a live Postgres
//! already migrated to schema v5 (`MICROMEGAS_SQL_CONNECTION_STRING`).

use base64::Engine;
use micromegas_auth::db_api_key::{
    ApiKeyTable, DbApiKeyAuthProvider, DbApiKeyConfig, dedicated_key_store_pool, generate_key,
    hash_key, key_store_has_live_rows,
};
use micromegas_auth::multi::MultiAuthProvider;
use micromegas_auth::types::{AuthProvider, HttpRequestParts, ProviderUnavailable, RequestParts};
use micromegas_tracing::event::in_memory_sink::InMemorySink;
use micromegas_tracing::metrics::MetricsMsgQueueAny;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;
use sqlx::Row;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::Duration;

fn unreachable_pool() -> sqlx::PgPool {
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible")
}

fn test_config() -> DbApiKeyConfig {
    DbApiKeyConfig {
        cache_size: 100,
        cache_ttl_secs: 60,
        unknown_cache_ttl_secs: 10,
        unknown_cache_size: 100,
    }
}

fn bearer_parts(token: &str) -> HttpRequestParts {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        format!("Bearer {token}").parse().expect("valid header"),
    );
    HttpRequestParts {
        headers,
        method: http::Method::GET,
        uri: "/test".parse().expect("valid uri"),
    }
}

fn count_integer_metric(sink: &InMemorySink, name: &str) -> u64 {
    let state = sink.state.lock().expect("sink lock");
    let mut count = 0u64;
    for block in &state.metrics_blocks {
        for evt in block.events.iter() {
            match evt {
                MetricsMsgQueueAny::IntegerMetricEvent(e) if e.desc.name == name => count += 1,
                MetricsMsgQueueAny::TaggedIntegerMetricEvent(e) if e.desc.name == name => {
                    count += 1
                }
                _ => {}
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// No DB
// ---------------------------------------------------------------------------

#[test]
fn hash_key_matches_known_vector_and_never_contains_key_bytes() {
    let hash = hash_key("hello world");
    assert_eq!(hash, sha256_hello_world());

    // The digest must never contain the plaintext key's bytes verbatim.
    let key = "mmk_super-secret-key-value-that-is-long-enough";
    let hash = hash_key(key);
    let hash_str = hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
    assert!(!hash_str.contains(key));
}

/// sha256("hello world") — a well-known test vector, computed independently of
/// this crate's `Sha256` usage to double as a known-answer check for `hash_key`.
fn sha256_hello_world() -> [u8; 32] {
    let hex = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("valid hex");
    }
    out
}

#[test]
fn generate_key_has_mmk_prefix_32_decoded_bytes_and_is_distinct() {
    let a = generate_key();
    let b = generate_key();
    assert_ne!(a, b);
    for key in [&a, &b] {
        let encoded = key.strip_prefix("mmk_").expect("mmk_ prefix");
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .expect("valid base64url");
        assert_eq!(decoded.len(), 32);
    }
}

#[test]
fn table_name_maps_to_expected_literals() {
    assert_eq!(ApiKeyTable::Ingestion.table_name(), "ingestion_api_keys");
    assert_eq!(ApiKeyTable::Analytics.table_name(), "analytics_api_keys");
}

#[tokio::test]
async fn missing_bearer_token_fails_before_any_db_access() {
    let provider =
        DbApiKeyAuthProvider::new(unreachable_pool(), ApiKeyTable::Ingestion, test_config());
    let parts = HttpRequestParts {
        headers: http::HeaderMap::new(),
        method: http::Method::GET,
        uri: "/test".parse().expect("valid uri"),
    };
    let result = provider.validate_request(&parts as &dyn RequestParts).await;
    assert!(result.is_err());
}

/// A DB error is not cached as `unknown`: two calls with the same token both
/// attempt the DB (asserted via the error surfacing as `ProviderUnavailable`
/// both times, rather than the second call short-circuiting through the
/// `unknown` cache — which would surface as a plain "invalid API token" instead).
#[test]
#[serial]
fn db_error_is_not_cached_as_unknown() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let provider =
            DbApiKeyAuthProvider::new(unreachable_pool(), ApiKeyTable::Ingestion, test_config());
        let parts = bearer_parts("some-token");

        let first = provider.validate_request(&parts as &dyn RequestParts).await;
        let first_err = first.expect_err("expected an error");
        assert!(first_err.downcast_ref::<ProviderUnavailable>().is_some());

        let second = provider.validate_request(&parts as &dyn RequestParts).await;
        let second_err = second.expect_err("expected an error");
        assert!(second_err.downcast_ref::<ProviderUnavailable>().is_some());
    });
}

/// A key-store outage is a `ProviderUnavailable`, not a generic rejection.
#[test]
#[serial]
fn outage_is_provider_unavailable() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let provider =
            DbApiKeyAuthProvider::new(unreachable_pool(), ApiKeyTable::Ingestion, test_config());
        let parts = bearer_parts("another-token");
        let result = provider.validate_request(&parts as &dyn RequestParts).await;
        let err = result.expect_err("expected an error");
        assert!(err.downcast_ref::<ProviderUnavailable>().is_some());
    });
}

/// `db_api_key_error_count` fires on a DB error, unconditionally — even when
/// the rate-limited `error!` line itself is suppressed on the second attempt.
#[test]
#[serial]
fn db_api_key_error_count_fires_on_db_error() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let guard = init_in_memory_tracing();
        let provider =
            DbApiKeyAuthProvider::new(unreachable_pool(), ApiKeyTable::Ingestion, test_config());
        let parts = bearer_parts("metric-test-token");

        let _ = provider.validate_request(&parts as &dyn RequestParts).await;
        let _ = provider.validate_request(&parts as &dyn RequestParts).await;

        micromegas_tracing::dispatch::flush_metrics_buffer();
        assert_eq!(
            count_integer_metric(&guard.sink, "db_api_key_error_count"),
            2
        );
    });
}

#[tokio::test]
async fn dedicated_key_store_pool_is_small_and_lazy() {
    let lake_pool = unreachable_pool();
    // Building the dedicated pool must not itself attempt a connection (it uses
    // `connect_lazy_with`), so this must return promptly even though the lake
    // pool is unreachable.
    let key_pool = dedicated_key_store_pool(&lake_pool);
    assert_eq!(key_pool.options().get_max_connections(), 4);
}

#[tokio::test]
async fn multi_provider_composes_env_and_db_types() {
    // Compile-time / construction smoke test: a MultiAuthProvider can hold both
    // an env ApiKeyAuthProvider and a DbApiKeyAuthProvider side by side.
    let keyring =
        micromegas_auth::api_key::parse_key_ring(r#"[{"name": "t", "key": "secret"}]"#).unwrap();
    let env_provider = Arc::new(micromegas_auth::api_key::ApiKeyAuthProvider::new(keyring));
    let db_provider = Arc::new(DbApiKeyAuthProvider::new(
        unreachable_pool(),
        ApiKeyTable::Ingestion,
        test_config(),
    ));
    let _multi = MultiAuthProvider::new()
        .with_provider(env_provider)
        .with_provider(db_provider);
}

// ---------------------------------------------------------------------------
// #[ignore], live Postgres (already migrated to schema v5)
// ---------------------------------------------------------------------------

async fn live_pool() -> sqlx::PgPool {
    let conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live, migrated Postgres");
    sqlx::PgPool::connect(&conn_str)
        .await
        .expect("connecting to metadata Postgres")
}

async fn insert_live_key(
    pool: &sqlx::PgPool,
    table: ApiKeyTable,
    name: &str,
    key: &str,
) -> uuid::Uuid {
    let key_id = uuid::Uuid::new_v4();
    let hash = hash_key(key);
    sqlx::query(&format!(
        "INSERT INTO {} (key_id, key_hash, name, created_at, created_by) VALUES ($1, $2, $3, now(), 'test')",
        table.table_name()
    ))
    .bind(key_id)
    .bind(&hash[..])
    .bind(name)
    .execute(pool)
    .await
    .expect("inserting test key");
    key_id
}

async fn cleanup_key(pool: &sqlx::PgPool, table: ApiKeyTable, key_id: uuid::Uuid) {
    let _ = sqlx::query(&format!(
        "DELETE FROM {} WHERE key_id = $1",
        table.table_name()
    ))
    .bind(key_id)
    .execute(pool)
    .await;
}

#[ignore]
#[tokio::test]
async fn live_row_authenticates_with_expected_context() {
    let pool = live_pool().await;
    let key = format!("mmk_test_{}", uuid::Uuid::new_v4());
    let key_id = insert_live_key(&pool, ApiKeyTable::Ingestion, "db-api-key-test-row", &key).await;

    let provider = DbApiKeyAuthProvider::new(pool.clone(), ApiKeyTable::Ingestion, test_config());
    let parts = bearer_parts(&key);
    let ctx = provider
        .validate_request(&parts as &dyn RequestParts)
        .await
        .expect("live key should authenticate");
    assert_eq!(ctx.subject, "db-api-key-test-row");
    assert!(matches!(
        ctx.auth_type,
        micromegas_auth::types::AuthType::ApiKey
    ));
    assert!(!ctx.is_admin);
    assert!(ctx.allow_delegation);

    cleanup_key(&pool, ApiKeyTable::Ingestion, key_id).await;
}

#[ignore]
#[tokio::test]
async fn live_unknown_key_is_rejected() {
    let pool = live_pool().await;
    let provider = DbApiKeyAuthProvider::new(pool, ApiKeyTable::Ingestion, test_config());
    let parts = bearer_parts(&format!("mmk_unknown_{}", uuid::Uuid::new_v4()));
    let result = provider.validate_request(&parts as &dyn RequestParts).await;
    assert!(result.is_err());
}

#[ignore]
#[tokio::test]
async fn live_revocation_latency_is_bounded_by_ttl() {
    let pool = live_pool().await;
    let key = format!("mmk_test_revoke_{}", uuid::Uuid::new_v4());
    let key_id = insert_live_key(
        &pool,
        ApiKeyTable::Ingestion,
        "db-api-key-test-revoke",
        &key,
    )
    .await;

    // cache_ttl_secs: 0 — revocation takes effect immediately, since nothing is
    // cached across calls.
    let zero_ttl_config = DbApiKeyConfig {
        cache_ttl_secs: 0,
        ..test_config()
    };
    let provider = DbApiKeyAuthProvider::new(pool.clone(), ApiKeyTable::Ingestion, zero_ttl_config);
    let parts = bearer_parts(&key);
    provider
        .validate_request(&parts as &dyn RequestParts)
        .await
        .expect("key should authenticate before revocation");

    sqlx::query("UPDATE ingestion_api_keys SET revoked_at = now() WHERE key_id = $1")
        .bind(key_id)
        .execute(&pool)
        .await
        .expect("revoking key");

    let result = provider.validate_request(&parts as &dyn RequestParts).await;
    assert!(
        result.is_err(),
        "revoked key must be rejected once uncached"
    );

    cleanup_key(&pool, ApiKeyTable::Ingestion, key_id).await;

    // Second half: a nonzero TTL means the key keeps authenticating immediately
    // after revocation, until the cache entry ages out — bounded, not
    // instantaneous, invalidation.
    let key2 = format!("mmk_test_revoke2_{}", uuid::Uuid::new_v4());
    let key_id2 = insert_live_key(
        &pool,
        ApiKeyTable::Ingestion,
        "db-api-key-test-revoke2",
        &key2,
    )
    .await;
    let provider2 = DbApiKeyAuthProvider::new(pool.clone(), ApiKeyTable::Ingestion, test_config());
    let parts2 = bearer_parts(&key2);
    provider2
        .validate_request(&parts2 as &dyn RequestParts)
        .await
        .expect("key should authenticate and populate the cache");

    sqlx::query("UPDATE ingestion_api_keys SET revoked_at = now() WHERE key_id = $1")
        .bind(key_id2)
        .execute(&pool)
        .await
        .expect("revoking key");

    provider2
        .validate_request(&parts2 as &dyn RequestParts)
        .await
        .expect("cached key keeps authenticating until the TTL elapses");

    cleanup_key(&pool, ApiKeyTable::Ingestion, key_id2).await;
}

#[ignore]
#[tokio::test]
async fn live_no_cleartext_is_stored() {
    let pool = live_pool().await;
    let key = format!("mmk_test_cleartext_{}", uuid::Uuid::new_v4());
    let key_id = insert_live_key(
        &pool,
        ApiKeyTable::Ingestion,
        "db-api-key-test-cleartext",
        &key,
    )
    .await;

    let row = sqlx::query("SELECT key_hash FROM ingestion_api_keys WHERE key_id = $1")
        .bind(key_id)
        .fetch_one(&pool)
        .await
        .expect("fetching row");
    let stored_hash: Vec<u8> = row.get::<Vec<u8>, _>("key_hash");
    assert_eq!(stored_hash, hash_key(&key).to_vec());

    cleanup_key(&pool, ApiKeyTable::Ingestion, key_id).await;
}

#[ignore]
#[tokio::test]
async fn live_last_used_at_written_on_miss_not_on_hit() {
    let pool = live_pool().await;
    let key = format!("mmk_test_last_used_{}", uuid::Uuid::new_v4());
    let key_id = insert_live_key(
        &pool,
        ApiKeyTable::Ingestion,
        "db-api-key-test-last-used",
        &key,
    )
    .await;

    let provider = DbApiKeyAuthProvider::new(pool.clone(), ApiKeyTable::Ingestion, test_config());
    let parts = bearer_parts(&key);

    let row = sqlx::query("SELECT last_used_at FROM ingestion_api_keys WHERE key_id = $1")
        .bind(key_id)
        .fetch_one(&pool)
        .await
        .expect("fetching row");
    let before: Option<chrono::DateTime<chrono::Utc>> = row.get("last_used_at");
    assert!(before.is_none());

    provider
        .validate_request(&parts as &dyn RequestParts)
        .await
        .expect("first call is a cache miss and should authenticate");

    let row = sqlx::query("SELECT last_used_at FROM ingestion_api_keys WHERE key_id = $1")
        .bind(key_id)
        .fetch_one(&pool)
        .await
        .expect("fetching row");
    let after_miss: Option<chrono::DateTime<chrono::Utc>> = row.get("last_used_at");
    assert!(after_miss.is_some());

    provider
        .validate_request(&parts as &dyn RequestParts)
        .await
        .expect("second call is a cache hit and should authenticate");

    let row = sqlx::query("SELECT last_used_at FROM ingestion_api_keys WHERE key_id = $1")
        .bind(key_id)
        .fetch_one(&pool)
        .await
        .expect("fetching row");
    let after_hit: Option<chrono::DateTime<chrono::Utc>> = row.get("last_used_at");
    assert_eq!(
        after_miss, after_hit,
        "a cache hit must not re-issue the UPDATE"
    );

    cleanup_key(&pool, ApiKeyTable::Ingestion, key_id).await;
}

#[ignore]
#[tokio::test]
async fn live_env_and_db_compose() {
    let pool = live_pool().await;
    let key = format!("mmk_test_compose_{}", uuid::Uuid::new_v4());
    let key_id = insert_live_key(
        &pool,
        ApiKeyTable::Ingestion,
        "db-api-key-test-compose",
        &key,
    )
    .await;

    let env_keyring =
        micromegas_auth::api_key::parse_key_ring(r#"[{"name": "env-key", "key": "env-secret"}]"#)
            .expect("parse keyring");
    let env_provider = Arc::new(micromegas_auth::api_key::ApiKeyAuthProvider::new(
        env_keyring,
    ));
    let db_provider = Arc::new(DbApiKeyAuthProvider::new(
        pool.clone(),
        ApiKeyTable::Ingestion,
        test_config(),
    ));
    let multi = MultiAuthProvider::new()
        .with_provider(env_provider)
        .with_provider(db_provider);

    let env_parts = bearer_parts("env-secret");
    multi
        .validate_request(&env_parts as &dyn RequestParts)
        .await
        .expect("env key should authenticate");

    let db_parts = bearer_parts(&key);
    multi
        .validate_request(&db_parts as &dyn RequestParts)
        .await
        .expect("db key should authenticate");

    cleanup_key(&pool, ApiKeyTable::Ingestion, key_id).await;
}

#[ignore]
#[tokio::test]
async fn live_surface_separation_both_directions() {
    let pool = live_pool().await;
    let ingestion_key = format!("mmk_test_surf_ing_{}", uuid::Uuid::new_v4());
    let ingestion_key_id = insert_live_key(
        &pool,
        ApiKeyTable::Ingestion,
        "db-api-key-test-surf-ing",
        &ingestion_key,
    )
    .await;
    let analytics_key = format!("mmk_test_surf_ana_{}", uuid::Uuid::new_v4());
    let analytics_key_id = insert_live_key(
        &pool,
        ApiKeyTable::Analytics,
        "db-api-key-test-surf-ana",
        &analytics_key,
    )
    .await;

    let ingestion_provider =
        DbApiKeyAuthProvider::new(pool.clone(), ApiKeyTable::Ingestion, test_config());
    let analytics_provider =
        DbApiKeyAuthProvider::new(pool.clone(), ApiKeyTable::Analytics, test_config());

    // An ingestion row is rejected by a provider bound to Analytics.
    let ingestion_parts = bearer_parts(&ingestion_key);
    assert!(
        analytics_provider
            .validate_request(&ingestion_parts as &dyn RequestParts)
            .await
            .is_err()
    );
    // And vice versa: an analytics row is rejected by a provider bound to
    // Ingestion.
    let analytics_parts = bearer_parts(&analytics_key);
    assert!(
        ingestion_provider
            .validate_request(&analytics_parts as &dyn RequestParts)
            .await
            .is_err()
    );

    cleanup_key(&pool, ApiKeyTable::Ingestion, ingestion_key_id).await;
    cleanup_key(&pool, ApiKeyTable::Analytics, analytics_key_id).await;
}

#[ignore]
#[tokio::test]
async fn live_key_store_has_live_rows_reflects_state() {
    let pool = live_pool().await;
    let key = format!("mmk_test_exist_{}", uuid::Uuid::new_v4());
    let key_id =
        insert_live_key(&pool, ApiKeyTable::Ingestion, "db-api-key-test-exist", &key).await;

    let has_rows = key_store_has_live_rows(&pool, ApiKeyTable::Ingestion)
        .await
        .expect("query should succeed against a migrated schema");
    assert!(has_rows);

    cleanup_key(&pool, ApiKeyTable::Ingestion, key_id).await;
}
