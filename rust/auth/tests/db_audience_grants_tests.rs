//! Tests for `micromegas_auth::db_audience_grants`.
//!
//! The "no DB" section builds a `DbAudienceGrantsSource` against a pool that is never actually
//! reachable (`connect_lazy` with a short `acquire_timeout` — see `test_utils::unreachable_pool`),
//! exercising the cold-start-failure path with no live Postgres. The `#[ignore]`d section needs a
//! live Postgres and creates a throwaway `audience_grants` table in its own schema (the
//! `default_provider_tests.rs` trick) rather than depending on `micromegas-ingestion`'s full
//! migration for one small table.

mod test_utils;

use micromegas_auth::db_audience_grants::{DbAudienceGrantsConfig, DbAudienceGrantsSource};
use micromegas_auth::policy::{
    AudienceGrants, AudienceMintPolicy, AudienceReadPolicy, MintPolicy, ReadPolicy,
};
use micromegas_auth::types::{AuthContext, AuthType};
use serial_test::serial;
use sqlx::postgres::PgPoolOptions;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use test_utils::unreachable_pool;

fn ttl(secs: u64) -> Duration {
    Duration::from_secs(secs)
}

fn caller(email: Option<&str>, groups: Vec<String>) -> AuthContext {
    AuthContext {
        subject: "test-subject".to_string(),
        email: email.map(String::from),
        issuer: "test-issuer".to_string(),
        audience: None,
        expires_at: None,
        auth_type: AuthType::Oidc,
        allow_delegation: false,
        bound_audience: None,
        read_audiences: vec![],
        memberships: groups.into(),
    }
}

// ---------------------------------------------------------------------------
// No DB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cold_start_against_unreachable_db_is_err() {
    let source = DbAudienceGrantsSource::new(unreachable_pool(), ttl(60));
    let result = source.current().await;
    assert!(
        result.is_err(),
        "a fresh source with no prior snapshot must fail closed"
    );
}

/// The second call lands inside the same throttled cold-start window: it must still return an
/// error (there is nothing successful to serve), and the error names the throttled-retry state
/// rather than surfacing as a second identical connection failure.
#[tokio::test]
async fn cold_start_failure_is_throttled_within_the_ttl_window() {
    let source = DbAudienceGrantsSource::new(unreachable_pool(), ttl(60));
    let _first = source.current().await.expect_err("first attempt fails");
    let second = source
        .current()
        .await
        .expect_err("second attempt is still cold-start with no snapshot to serve");
    assert!(
        second.to_string().contains("retry available in"),
        "expected the throttled-state message, got: {second}"
    );
}

/// A cold-start store outage denies even when the env grant map alone would be permissive --
/// there is no known-good state to fall back to (design question 1: the fail-closed guarantee is
/// preserved exactly for this case).
#[tokio::test]
async fn read_policy_with_unreachable_store_fails_closed_even_with_permissive_env_grants() {
    let grants = AudienceGrants::parse(r#"{"team-alpha": ["*"]}"#).expect("valid grants");
    let store = Arc::new(DbAudienceGrantsSource::new(unreachable_pool(), ttl(60)));
    let policy = AudienceReadPolicy::new(grants).with_store(Some(store));
    let ctx = caller(None, vec![]);
    let result = policy.resolve(&ctx).await;
    assert!(
        result.is_err(),
        "a cold-start store outage must deny, not silently fall back to the env map alone"
    );
}

/// Same fail-closed property on the mint axis: `with_store` has no production call site yet, but
/// its behavior is still pinned by a unit test per the plan's stated mint-side scope.
#[tokio::test]
async fn mint_policy_with_unreachable_store_fails_closed() {
    let grants = AudienceGrants::parse(
        r#"{"alice-laptop": {"read": [], "mint": ["user:alice@example.com"]}}"#,
    )
    .expect("valid grants");
    let store = Arc::new(DbAudienceGrantsSource::new(unreachable_pool(), ttl(60)));
    let policy = AudienceMintPolicy::new(grants).with_store(Some(store));
    let ctx = caller(Some("alice@example.com"), vec![]);
    let result = policy.resolve_audience(&ctx, Some("alice-laptop")).await;
    assert!(
        result.is_err(),
        "a cold-start store outage must deny mint resolution too"
    );
}

/// Smoke test for `DbGroupsSource`, which shares `SnapshotSource`'s cache mechanics with
/// `DbAudienceGrantsSource` -- exercises the same cold-start-failure path against an
/// unreachable pool, pinning that the generic extraction still works for the second instantiation.
#[tokio::test]
async fn group_store_cold_start_against_unreachable_db_is_err() {
    use micromegas_auth::groups::DbGroupsSource;
    let source = DbGroupsSource::new(unreachable_pool(), ttl(60));
    let result = source.current().await;
    assert!(
        result.is_err(),
        "a fresh group store with no prior snapshot must fail closed"
    );
}

// ---------------------------------------------------------------------------
// DbAudienceGrantsConfig::from_env_with_prefix -- the flat, unprefixed `MICROMEGAS_AUTH_
// CACHE_TTL_SECONDS` knob `DbApiKeyConfig`/`DbGroupsConfig` also read, ignoring `prefix`.
// ---------------------------------------------------------------------------

const UNPREFIXED_VAR: &str = "MICROMEGAS_AUTH_CACHE_TTL_SECONDS";
/// A role prefix that must have no effect on this knob -- passed to every call below to pin that.
const SOME_PREFIX: &str = "MICROMEGAS_1489_GRANTS_TESTS";

struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(UNPREFIXED_VAR);
        }
    }
}

#[test]
#[serial]
fn config_from_env_defaults_to_60_when_unset() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(UNPREFIXED_VAR);
    }
    assert_eq!(
        DbAudienceGrantsConfig::from_env_with_prefix(SOME_PREFIX).cache_ttl_secs,
        60
    );
}

#[test]
#[serial]
fn config_from_env_reads_the_flat_unprefixed_var_regardless_of_prefix() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(UNPREFIXED_VAR, "120");
    }
    assert_eq!(
        DbAudienceGrantsConfig::from_env_with_prefix(SOME_PREFIX).cache_ttl_secs,
        120
    );
    // A role-prefixed variant of this knob does not exist -- setting one would have no effect,
    // but there is nothing to set, since `from_env_with_prefix` never even builds that name.
    assert_eq!(
        DbAudienceGrantsConfig::from_env_with_prefix("").cache_ttl_secs,
        120
    );
}

// ---------------------------------------------------------------------------
// #[ignore], live Postgres. Creates a throwaway `audience_grants` table in its own schema
// (the `default_provider_tests.rs` trick) rather than depending on `micromegas-ingestion`'s
// migration for one small table.
// ---------------------------------------------------------------------------

async fn throwaway_pool(schema: &str) -> (sqlx::PgPool, sqlx::PgPool, String) {
    let base_conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live Postgres");

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
        "CREATE TABLE {schema}.audience_grants (
           audience   VARCHAR(255) NOT NULL,
           axis       VARCHAR(4) NOT NULL,
           selector   VARCHAR(255) NOT NULL,
           created_at TIMESTAMPTZ NOT NULL,
           created_by VARCHAR(255) NOT NULL,
           PRIMARY KEY (audience, axis, selector)
         )"
    ))
    .execute(&setup_pool)
    .await
    .expect("creating throwaway audience_grants table");

    let opts = sqlx::postgres::PgConnectOptions::from_str(&base_conn_str)
        .expect("valid connection string")
        .options([("search_path", schema)]);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await
        .expect("connecting with a throwaway search_path");

    (setup_pool, pool, base_conn_str)
}

async fn drop_schema(setup_pool: &sqlx::PgPool, schema: &str) {
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(setup_pool)
        .await
        .expect("dropping throwaway schema");
}

#[ignore]
#[tokio::test]
async fn live_first_load_reflects_seeded_rows() {
    let schema = "mm_1489_grants_test_first_load";
    let (setup_pool, pool, _conn) = throwaway_pool(schema).await;

    let test_result: Result<(), String> = async {
        sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ('team-alpha', 'read', 'group:eng', now(), 'test')",
        )
        .execute(&pool)
        .await
        .map_err(|e| format!("seeding a row: {e:#}"))?;

        let source = DbAudienceGrantsSource::new(pool.clone(), ttl(60));
        let grants = source
            .current()
            .await
            .map_err(|e| format!("first load should succeed: {e:#}"))?;

        let policy = AudienceReadPolicy::new((*grants).clone());
        let ctx = caller(None, vec!["eng".to_string()]);
        let resolved = policy
            .resolve(&ctx)
            .await
            .map_err(|e| format!("resolve should succeed: {e:#}"))?
            .into_inner();
        if !resolved.contains(&"team-alpha".to_string()) {
            return Err(format!(
                "expected team-alpha in resolved set, got {resolved:?}"
            ));
        }
        Ok(())
    }
    .await;

    pool.close().await;
    drop_schema(&setup_pool, schema).await;
    test_result.expect("test assertions");
}

/// After one successful load, a refresh failure (the table dropped out from under the source)
/// keeps serving the prior snapshot rather than propagating `Err` -- design question 1's
/// serve-stale-on-failure semantics.
#[ignore]
#[tokio::test]
async fn live_refresh_failure_after_success_serves_stale_snapshot() {
    let schema = "mm_1489_grants_test_refresh_failure";
    let (setup_pool, pool, _conn) = throwaway_pool(schema).await;

    let test_result: Result<(), String> = async {
        sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ('team-alpha', 'read', '*', now(), 'test')",
        )
        .execute(&pool)
        .await
        .map_err(|e| format!("seeding a row: {e:#}"))?;

        // A 1-second TTL so the second `current()` call, after a short sleep, is forced past the
        // post-success `fetched_at` gate and attempts a real refresh.
        let source = DbAudienceGrantsSource::new(pool.clone(), ttl(1));
        source
            .current()
            .await
            .map_err(|e| format!("first load should succeed: {e:#}"))?;

        sqlx::query("DROP TABLE audience_grants")
            .execute(&pool)
            .await
            .map_err(|e| format!("dropping the table to force a refresh failure: {e:#}"))?;

        tokio::time::sleep(Duration::from_millis(1100)).await;

        let grants = source.current().await.map_err(|e| {
            format!("a post-success refresh failure must serve stale, not Err: {e:#}")
        })?;
        let ctx = caller(None, vec![]);
        // Exercise the served snapshot through the policy seam rather than reaching into
        // `AudienceGrants`'s private fields.
        let policy = AudienceReadPolicy::new((*grants).clone());
        let resolved = policy
            .resolve(&ctx)
            .await
            .map_err(|e| format!("resolve over the stale snapshot should still succeed: {e:#}"))?
            .into_inner();
        if !resolved.contains(&"team-alpha".to_string()) {
            return Err(format!(
                "expected the stale snapshot to still grant team-alpha, got {resolved:?}"
            ));
        }
        Ok(())
    }
    .await;

    pool.close().await;
    drop_schema(&setup_pool, schema).await;
    test_result.expect("test assertions");
}

/// No prior snapshot, and the table itself doesn't exist -- a cold-start failure, `Err`.
#[ignore]
#[tokio::test]
async fn live_cold_start_failure_against_missing_table_is_err() {
    let schema = "mm_1489_grants_test_cold_start_missing_table";
    let base_conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live Postgres");

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
        .expect("creating throwaway (table-less) schema");

    let opts = sqlx::postgres::PgConnectOptions::from_str(&base_conn_str)
        .expect("valid connection string")
        .options([("search_path", schema)]);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("connecting with a throwaway search_path");

    let source = DbAudienceGrantsSource::new(pool.clone(), ttl(60));
    let result = source.current().await;

    pool.close().await;
    drop_schema(&setup_pool, schema).await;

    assert!(
        result.is_err(),
        "a missing table with no prior snapshot must fail closed"
    );
}

/// A row that slipped past the table's own shape (no `CHECK` constraints in this throwaway
/// table, standing in for a hand-edited row via a direct `psql` session) fails the whole
/// snapshot load loudly, per `AudienceGrants::from_rows`'s re-validation.
#[ignore]
#[tokio::test]
async fn live_malformed_row_fails_the_whole_snapshot_load() {
    let schema = "mm_1489_grants_test_malformed_row";
    let (setup_pool, pool, _conn) = throwaway_pool(schema).await;

    let test_result: Result<(), String> = async {
        sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ('not a valid audience', 'read', '*', now(), 'test')",
        )
        .execute(&pool)
        .await
        .map_err(|e| format!("seeding a malformed row: {e:#}"))?;

        let source = DbAudienceGrantsSource::new(pool.clone(), ttl(60));
        let result = source.current().await;
        if result.is_ok() {
            return Err("expected a malformed row to fail the whole load".to_string());
        }
        Ok(())
    }
    .await;

    pool.close().await;
    drop_schema(&setup_pool, schema).await;
    test_result.expect("test assertions");
}

/// End to end: a grant written straight to the table (standing in for the admin route, which
/// this crate cannot call directly) reaches `AudienceMintPolicy::resolve_audience` through
/// `with_store`, merged with the env-equivalent map passed to `AudienceMintPolicy::new`.
#[ignore]
#[tokio::test]
async fn live_mint_policy_with_store_merges_a_store_granted_selector() {
    let schema = "mm_1489_grants_test_mint_merge";
    let (setup_pool, pool, _conn) = throwaway_pool(schema).await;

    let test_result: Result<(), String> = async {
        sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ('alice-laptop', 'mint', 'user:alice@example.com', now(), 'test')",
        )
        .execute(&pool)
        .await
        .map_err(|e| format!("seeding a row: {e:#}"))?;

        let store = Arc::new(DbAudienceGrantsSource::new(pool.clone(), ttl(60)));
        let policy = AudienceMintPolicy::new(AudienceGrants::empty()).with_store(Some(store));
        let ctx = caller(Some("alice@example.com"), vec![]);
        let resolved = policy
            .resolve_audience(&ctx, Some("alice-laptop"))
            .await
            .map_err(|e| format!("expected the store-granted mint to resolve: {e:#}"))?;
        if resolved != "alice-laptop" {
            return Err(format!("unexpected resolved audience {resolved:?}"));
        }
        Ok(())
    }
    .await;

    pool.close().await;
    drop_schema(&setup_pool, schema).await;
    test_result.expect("test assertions");
}

/// End to end on the read axis: `AudienceReadPolicy::with_store` is the only store-wiring path
/// with a real production call site (`flight_sql_server.rs`, `monolith/src/main.rs`), so this
/// proves the store snapshot's `readers()` loop in `AudienceReadPolicy::resolve` actually grants
/// access, not merely that a store-backed policy doesn't error -- the counterpart to
/// `live_mint_policy_with_store_merges_a_store_granted_selector` above, on the axis that is
/// actually wired in production.
#[ignore]
#[tokio::test]
async fn live_read_policy_with_store_grants_a_store_granted_selector() {
    let schema = "mm_1489_grants_test_read_store";
    let (setup_pool, pool, _conn) = throwaway_pool(schema).await;

    let test_result: Result<(), String> = async {
        sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ('team-alpha', 'read', 'group:eng', now(), 'test')",
        )
        .execute(&pool)
        .await
        .map_err(|e| format!("seeding a row: {e:#}"))?;

        let store = Arc::new(DbAudienceGrantsSource::new(pool.clone(), ttl(60)));
        let policy = AudienceReadPolicy::new(AudienceGrants::empty()).with_store(Some(store));
        let ctx = caller(None, vec!["eng".to_string()]);
        let resolved = policy
            .resolve(&ctx)
            .await
            .map_err(|e| format!("expected the store-granted read to resolve: {e:#}"))?
            .into_inner();
        if !resolved.contains(&"team-alpha".to_string()) {
            return Err(format!(
                "expected team-alpha (store-granted) in resolved set, got {resolved:?}"
            ));
        }
        Ok(())
    }
    .await;

    pool.close().await;
    drop_schema(&setup_pool, schema).await;
    test_result.expect("test assertions");
}
