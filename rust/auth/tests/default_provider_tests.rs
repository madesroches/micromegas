//! Tests for `micromegas_auth::default_provider::ProviderBuilder` and §3's
//! startup-existence rules (#1383). All four bullets here need a live,
//! already-migrated-to-v5 Postgres (`MICROMEGAS_SQL_CONNECTION_STRING`) — this
//! exercises `key_store_has_live_rows` directly, so an unmigrated or
//! `connect_lazy` pool cannot stand in for the "genuinely missing relation" case.
//!
//! Every test here mutates process-wide env vars (`MICROMEGAS_API_KEYS`,
//! `MICROMEGAS_OIDC_CONFIG`), so all are `#[serial]` with an `EnvGuard` that
//! restores them on drop — the same pattern as
//! `rust/ingestion/tests/data_lake_config_tests.rs`.

#![cfg(test)]

use micromegas_auth::db_api_key::{ApiKeyTable, hash_key, key_store_has_live_rows};
use micromegas_auth::default_provider::ProviderBuilder;
use micromegas_auth::types::{HttpRequestParts, RequestParts};
use serial_test::serial;
use std::str::FromStr;

const API_KEYS_VAR: &str = "MICROMEGAS_API_KEYS";
const OIDC_CONFIG_VAR: &str = "MICROMEGAS_OIDC_CONFIG";

/// Clears both env vars on drop so a failing assertion in one test can't leak
/// state into the next.
struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(API_KEYS_VAR);
            std::env::remove_var(OIDC_CONFIG_VAR);
        }
    }
}

async fn live_pool() -> sqlx::PgPool {
    let conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live, migrated Postgres");
    sqlx::PgPool::connect(&conn_str)
        .await
        .expect("connecting to metadata Postgres")
}

async fn insert_live_key(pool: &sqlx::PgPool, name: &str, key: &str) -> uuid::Uuid {
    let key_id = uuid::Uuid::new_v4();
    let hash = hash_key(key);
    sqlx::query(
        "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by)
         VALUES ($1, $2, $3, now(), 'test')",
    )
    .bind(key_id)
    .bind(&hash[..])
    .bind(name)
    .execute(pool)
    .await
    .expect("inserting test key");
    key_id
}

async fn cleanup_key(pool: &sqlx::PgPool, key_id: uuid::Uuid) {
    let _ = sqlx::query("DELETE FROM ingestion_api_keys WHERE key_id = $1")
        .bind(key_id)
        .execute(pool)
        .await;
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

/// **Provider always registered**: `with_db_key_store` attached alongside an env
/// keyring (so `build()` returns `Some` regardless of the table's contents)
/// still produces a chain containing the DB provider — asserted by inserting a
/// row *after* `build()` returns and authenticating its key through the
/// returned provider, with no restart. This is the regression §3 calls out:
/// without it, a first-minted key would not authenticate until the process
/// restarts.
#[ignore]
#[tokio::test]
#[serial]
async fn provider_always_registered_authenticates_key_minted_after_build() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(API_KEYS_VAR, r#"[{"name": "env", "key": "env-secret"}]"#);
        std::env::remove_var(OIDC_CONFIG_VAR);
    }

    let pool = live_pool().await;
    let provider = ProviderBuilder::new("")
        .with_db_key_store(pool.clone(), ApiKeyTable::Ingestion)
        .build()
        .await
        .expect("build should succeed")
        .expect("env keys configured, so build() must return Some");

    // Minted *after* build() returned.
    let key = format!("mmk_test_registered_{}", uuid::Uuid::new_v4());
    let key_id = insert_live_key(&pool, "provider-always-registered-test", &key).await;

    let parts = bearer_parts(&key);
    let result = provider.validate_request(&parts as &dyn RequestParts).await;
    assert!(
        result.is_ok(),
        "a key minted after build() must authenticate with no restart"
    );

    cleanup_key(&pool, key_id).await;
}

/// **Non-empty table ⇒ `Some`**: a table with at least one live row and no env
/// keys / OIDC configured still yields `Ok(Some(_))` from `build()`.
#[ignore]
#[tokio::test]
#[serial]
async fn non_empty_table_alone_counts_as_configured() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(API_KEYS_VAR);
        std::env::remove_var(OIDC_CONFIG_VAR);
    }

    let pool = live_pool().await;
    let key = format!("mmk_test_nonempty_{}", uuid::Uuid::new_v4());
    let key_id = insert_live_key(&pool, "non-empty-table-test", &key).await;

    let result = ProviderBuilder::new("")
        .with_db_key_store(pool.clone(), ApiKeyTable::Ingestion)
        .build()
        .await
        .expect("build should succeed");
    assert!(
        result.is_some(),
        "a non-empty key store alone must count as auth configured"
    );

    cleanup_key(&pool, key_id).await;
}

/// **Empty table + nothing else configured ⇒ `Ok(None)`**, preserving the
/// "genuinely empty deployment" startup guard.
///
/// Assumes this runs against a scratch/test database whose `ingestion_api_keys`
/// table currently has no live rows — the same assumption every other
/// live-Postgres test in this repo makes about its schema/tables.
#[ignore]
#[tokio::test]
#[serial]
async fn empty_table_and_nothing_else_yields_none() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(API_KEYS_VAR);
        std::env::remove_var(OIDC_CONFIG_VAR);
    }

    let pool = live_pool().await;
    let has_rows = key_store_has_live_rows(&pool, ApiKeyTable::Ingestion)
        .await
        .expect("query should succeed against a migrated schema");
    assert!(
        !has_rows,
        "this test requires a scratch DB with no live ingestion_api_keys rows"
    );

    let result = ProviderBuilder::new("")
        .with_db_key_store(pool, ApiKeyTable::Ingestion)
        .build()
        .await
        .expect("build should succeed");
    assert!(result.is_none());
}

/// **Missing relation ⇒ `Err` naming the table**: a throwaway single-connection
/// pool whose `search_path` (set in the connect options themselves) points at a
/// schema that does not exist, so every connection this pool ever hands out
/// resolves the unqualified table name to nothing — without mutating
/// `search_path` on any connection borrowed from the shared pool.
#[ignore]
#[tokio::test]
#[serial]
async fn missing_relation_is_err_not_none() {
    let base_conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live Postgres instance");
    let opts = sqlx::postgres::PgConnectOptions::from_str(&base_conn_str)
        .expect("valid connection string")
        .options([("search_path", "mm_1383_test_throwaway_empty_schema")]);
    let throwaway_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("connecting with a throwaway search_path");

    let result = key_store_has_live_rows(&throwaway_pool, ApiKeyTable::Ingestion).await;
    let err = result.expect_err("relation must not resolve under the throwaway search_path");

    let db_err = err
        .chain()
        .find_map(|e| e.downcast_ref::<sqlx::Error>())
        .and_then(sqlx::Error::as_database_error)
        .expect("expected a sqlx database error in the chain");
    assert_eq!(
        db_err.code().as_deref(),
        Some("42P01"),
        "expected undefined_table (42P01), got: {db_err}"
    );

    // build() must propagate this, not silently treat it as Ok(None).
    let build_result = ProviderBuilder::new("")
        .with_db_key_store(throwaway_pool, ApiKeyTable::Ingestion)
        .build()
        .await;
    assert!(
        build_result.is_err(),
        "build() must propagate a missing-relation error, never Ok(None)"
    );
}
