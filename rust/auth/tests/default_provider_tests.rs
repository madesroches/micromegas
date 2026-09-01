//! Tests for `micromegas_auth::default_provider::ProviderBuilder` and its
//! startup-existence rules.
//!
//! Most tests here need a live, already-migrated-to-v5 Postgres
//! (`MICROMEGAS_SQL_CONNECTION_STRING`) and are `#[ignore]`d — they exercise
//! `key_store_has_live_rows` directly, so an unmigrated or `connect_lazy` pool
//! cannot stand in for the "genuinely missing relation" case. A few
//! `build_chain()` tests do not need a DB at all (it issues no existence
//! query) and run in ordinary CI.
//!
//! Every test here mutates process-wide env vars (`MICROMEGAS_API_KEYS`,
//! `MICROMEGAS_OIDC_CONFIG`), so all are `#[serial]` with an `EnvGuard` that
//! restores them on drop — the same pattern as
//! `rust/ingestion/tests/data_lake_config_tests.rs`.

#![cfg(test)]

mod test_utils;

use micromegas_auth::db_api_key::{ApiKeyTable, hash_key, key_store_has_live_rows};
use micromegas_auth::default_provider::ProviderBuilder;
use micromegas_auth::policy::PUBLIC_AUDIENCE;
use micromegas_auth::types::{HttpRequestParts, RequestParts};
use serial_test::serial;
use std::str::FromStr;
use test_utils::unreachable_pool;

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

async fn insert_live_key(pool: &sqlx::PgPool, name: &str, key: &str, audience: &str) -> uuid::Uuid {
    let key_id = uuid::Uuid::new_v4();
    let hash = hash_key(key);
    sqlx::query(
        "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by, audience)
         VALUES ($1, $2, $3, now(), 'test', $4)",
    )
    .bind(key_id)
    .bind(&hash[..])
    .bind(name)
    .bind(audience)
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
/// returned provider, with no restart: without this, a first-minted key would
/// not authenticate until the process restarts.
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
    let key_id = insert_live_key(
        &pool,
        "provider-always-registered-test",
        &key,
        PUBLIC_AUDIENCE,
    )
    .await;

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
    let key_id = insert_live_key(&pool, "non-empty-table-test", &key, PUBLIC_AUDIENCE).await;

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
/// Uses the same throwaway-schema / `search_path` trick as
/// `missing_relation_is_err_not_none` below, but here the schema is created
/// up front with an empty `ingestion_api_keys` table (just the `revoked_at`
/// column `key_store_has_live_rows`'s query touches), so this test never
/// depends on the *real* `ingestion_api_keys` table in the shared
/// `MICROMEGAS_SQL_CONNECTION_STRING` database being empty. Other live-DB
/// test binaries (`rust/public/tests/api_keys_tests.rs`,
/// `rust/auth/tests/db_api_key_tests.rs`) insert rows into that same shared
/// table, and cargo runs test binaries as separate concurrent processes —
/// `#[serial]` only serializes within one binary, so asserting against the
/// shared table directly would be spuriously flaky.
///
/// The throwaway schema is dropped unconditionally at the end (even if an
/// assertion below fails) by deferring the actual checks into a `Result`
/// and only `expect`-ing it after cleanup has run.
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

    let base_conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live, migrated Postgres");
    let schema = "mm_1383_test_empty_live_rows_schema";

    let setup_pool = sqlx::PgPool::connect(&base_conn_str)
        .await
        .expect("connecting to metadata Postgres");
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping any stale throwaway schema from a previous failed run");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&setup_pool)
        .await
        .expect("creating throwaway schema");
    sqlx::query(&format!(
        "CREATE TABLE {schema}.ingestion_api_keys (key_id UUID PRIMARY KEY, revoked_at TIMESTAMPTZ)"
    ))
    .execute(&setup_pool)
    .await
    .expect("creating throwaway empty ingestion_api_keys table");

    let opts = sqlx::postgres::PgConnectOptions::from_str(&base_conn_str)
        .expect("valid connection string")
        .options([("search_path", schema)]);
    let throwaway_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("connecting with a throwaway search_path");

    let test_result: Result<(), String> = async {
        let has_rows = key_store_has_live_rows(&throwaway_pool, ApiKeyTable::Ingestion)
            .await
            .map_err(|e| format!("query should succeed against the throwaway schema: {e:#}"))?;
        if has_rows {
            return Err("throwaway table must start with no live rows".to_string());
        }

        let result = ProviderBuilder::new("")
            .with_db_key_store(throwaway_pool.clone(), ApiKeyTable::Ingestion)
            .build()
            .await
            .map_err(|e| format!("build should succeed: {e:#}"))?;
        if result.is_some() {
            return Err(
                "an empty key store with nothing else configured must yield None".to_string(),
            );
        }
        Ok(())
    }
    .await;

    throwaway_pool.close().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping throwaway schema");

    test_result.expect("test assertions");
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
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(API_KEYS_VAR);
        std::env::remove_var(OIDC_CONFIG_VAR);
    }

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

/// **`build_chain()` keeps the DB provider even where `build()` would discard
/// it**: an empty key store with nothing else configured makes `build()` return
/// `None` (pinned above by `empty_table_and_nothing_else_yields_none`), but
/// `build_chain()` still returns a chain, and a key minted into that
/// previously-empty table authenticates through it with no restart.
///
/// Uses its own throwaway schema (same trick as
/// `empty_table_and_nothing_else_yields_none`) rather than the shared
/// `ingestion_api_keys` table, and the throwaway table carries the full v5
/// shape (`key_id`, `key_hash`, `name`, `created_at`, `created_by`,
/// `last_used_at`, `revoked_at`) plus the later-added `audience` column, since
/// `insert_live_key` and the provider's lookup query need all of them, not
/// just `key_id`/`revoked_at`.
#[ignore]
#[tokio::test]
#[serial]
async fn build_chain_authenticates_key_minted_into_empty_table() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(API_KEYS_VAR);
        std::env::remove_var(OIDC_CONFIG_VAR);
    }

    let base_conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live, migrated Postgres");
    let schema = "mm_1550_test_build_chain_schema";

    let setup_pool = sqlx::PgPool::connect(&base_conn_str)
        .await
        .expect("connecting to metadata Postgres");
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping any stale throwaway schema from a previous failed run");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&setup_pool)
        .await
        .expect("creating throwaway schema");
    sqlx::query(&format!(
        "CREATE TABLE {schema}.ingestion_api_keys (
           key_id       UUID PRIMARY KEY,
           key_hash     BYTEA NOT NULL,
           name         VARCHAR(255) NOT NULL,
           created_at   TIMESTAMPTZ NOT NULL,
           created_by   VARCHAR(255) NOT NULL,
           last_used_at TIMESTAMPTZ,
           revoked_at   TIMESTAMPTZ,
           audience     VARCHAR(255) NOT NULL
         )"
    ))
    .execute(&setup_pool)
    .await
    .expect("creating throwaway v5-shaped ingestion_api_keys table");

    let opts = sqlx::postgres::PgConnectOptions::from_str(&base_conn_str)
        .expect("valid connection string")
        .options([("search_path", schema)]);
    let throwaway_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("connecting with a throwaway search_path");

    let test_result: Result<(), String> = async {
        let build_result = ProviderBuilder::new("")
            .with_db_key_store(throwaway_pool.clone(), ApiKeyTable::Ingestion)
            .build()
            .await
            .map_err(|e| format!("build should succeed: {e:#}"))?;
        if build_result.is_some() {
            return Err(
                "an empty key store with nothing else configured must still yield None from build()"
                    .to_string(),
            );
        }

        let chain = ProviderBuilder::new("")
            .with_db_key_store(throwaway_pool.clone(), ApiKeyTable::Ingestion)
            .build_chain()
            .await
            .map_err(|e| format!("build_chain should succeed: {e:#}"))?;

        // Minted *after* build_chain() returned.
        let key = format!("mmk_test_build_chain_{}", uuid::Uuid::new_v4());
        insert_live_key(
            &throwaway_pool,
            "build-chain-empty-table-test",
            &key,
            PUBLIC_AUDIENCE,
        )
        .await;

        let parts = bearer_parts(&key);
        let result = chain.validate_request(&parts as &dyn RequestParts).await;
        if result.is_err() {
            return Err(format!(
                "a key minted after build_chain() returned must authenticate with no restart: {:?}",
                result.err()
            ));
        }
        Ok(())
    }
    .await;

    throwaway_pool.close().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping throwaway schema");

    test_result.expect("test assertions");
}

/// **No key store, no existence query**: `build_chain()` never calls
/// `key_store_has_live_rows`, so composing it against an unreachable pool still
/// returns `Ok` promptly — unlike `build()`, whose existence query fails to
/// connect and surfaces as `Err`.
#[tokio::test]
#[serial]
async fn build_chain_issues_no_startup_query() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(API_KEYS_VAR);
        std::env::remove_var(OIDC_CONFIG_VAR);
    }

    let build_err = ProviderBuilder::new("")
        .with_db_key_store(unreachable_pool(), ApiKeyTable::Ingestion)
        .build()
        .await;
    assert!(
        build_err.is_err(),
        "build() must fail when its existence query can't reach the pool"
    );

    let chain_result = ProviderBuilder::new("")
        .with_db_key_store(unreachable_pool(), ApiKeyTable::Ingestion)
        .build_chain()
        .await;
    assert!(
        chain_result.is_ok(),
        "build_chain() must not issue any startup query against the key store"
    );
}

/// **Env keys alone, no key store**: `build_chain()` composes and authenticates
/// through env keys just like `build()` does.
#[tokio::test]
#[serial]
async fn build_chain_with_env_keys_only_authenticates() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(
            API_KEYS_VAR,
            r#"[{"name": "env", "key": "chain-env-secret"}]"#,
        );
        std::env::remove_var(OIDC_CONFIG_VAR);
    }

    let chain = ProviderBuilder::new("")
        .build_chain()
        .await
        .expect("build_chain should succeed with only env keys configured");

    let parts = bearer_parts("chain-env-secret");
    let result = chain.validate_request(&parts as &dyn RequestParts).await;
    assert!(
        result.is_ok(),
        "an env-configured key must authenticate through the build_chain() chain"
    );
}
