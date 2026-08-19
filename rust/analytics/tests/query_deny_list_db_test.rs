//! DB-backed tests for the admin-managed query deny list (`tasks/query_deny_list_plan.md`).
//! `#[ignore]`d `#[tokio::test]`s requiring a live `MICROMEGAS_SQL_CONNECTION_STRING` /
//! `MICROMEGAS_OBJECT_STORE_URI`; mirrors `ownership_rewrite_db_test.rs`'s convention. Does not
//! run under a plain `cargo test`.

mod common;

use anyhow::{Context, Result};
use chrono::Utc;
use common::db_fixtures::ensure_telemetry_guard;
use micromegas_analytics::lakehouse::migration::migrate_lakehouse;
use micromegas_analytics::lakehouse::query_deny_list::QueryDenyList;
use micromegas_ingestion::data_lake_connection::connect_to_data_lake;
use sqlx::Row;
use uuid::Uuid;

async fn connect() -> Result<sqlx::Pool<sqlx::Postgres>> {
    ensure_telemetry_guard();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    Ok(lake.db_pool)
}

/// Deletes every row this test file may have inserted, identified by `created_by`, so repeated
/// runs against a shared dev database don't accumulate rules across runs.
async fn cleanup(pool: &sqlx::Pool<sqlx::Postgres>, created_by: &str) {
    let _ = sqlx::query("DELETE FROM query_deny_list WHERE created_by = $1")
        .bind(created_by)
        .execute(pool)
        .await;
}

#[ignore]
#[tokio::test]
async fn migration_v8_to_v9_applies_cleanly() -> Result<()> {
    let pool = connect().await?;
    // Idempotent -- runs the migration again on a lakehouse schema that may already be at v9
    // (e.g. from `flight-sql-srv` having started against this DB), or applies v9 fresh on one
    // that was still at v8 or earlier.
    migrate_lakehouse(pool.clone()).await?;
    let row = sqlx::query("SELECT to_regclass('query_deny_list') IS NOT NULL AS exists")
        .fetch_one(&pool)
        .await?;
    let exists: bool = row.try_get("exists")?;
    assert!(exists, "query_deny_list table should exist after migration");
    Ok(())
}

#[ignore]
#[tokio::test]
async fn insert_refresh_check_then_delete_refresh_no_longer_matches() -> Result<()> {
    let pool = connect().await?;
    migrate_lakehouse(pool.clone()).await?;
    let created_by = format!("query_deny_list_db_test-{}", Uuid::new_v4());
    let list = QueryDenyList::new(pool.clone());

    let ctx = datafusion::execution::context::SessionContext::new();
    let compiled = micromegas_analytics::lakehouse::query_deny_list::compile_match_expr(
        &ctx,
        "client = 'test-client'",
    )?;
    let row = list
        .insert("client = 'test-client'", compiled, "db test", &created_by)
        .await?;

    list.refresh().await?;

    let attr = micromegas_analytics::lakehouse::query_deny_list::QueryAttribution {
        user_id: "u",
        email: "u@example.com",
        service_account: None,
        client: "test-client",
        agent: "none",
        entrypoint: "script",
        session: None,
        notebook: None,
        cell: None,
        client_ip: "10.0.0.1",
        sql: "SELECT 1",
        sql_hash: "0000000000000000",
    };
    let matched = list
        .check(&attr)
        .expect("should match right after insert+refresh");
    assert_eq!(matched.row.rule_id, row.rule_id);

    let deleted = list.delete(row.rule_id).await?;
    assert!(deleted);
    list.refresh().await?;
    assert!(
        list.check(&attr).is_none(),
        "deleted rule should no longer match after refresh"
    );

    cleanup(&pool, &created_by).await;
    Ok(())
}

#[ignore]
#[tokio::test]
async fn hit_flush_keeps_most_recent_and_skips_unhit_rules() -> Result<()> {
    let pool = connect().await?;
    migrate_lakehouse(pool.clone()).await?;
    let created_by = format!("query_deny_list_db_test-{}", Uuid::new_v4());
    let list = QueryDenyList::new(pool.clone());

    let ctx = datafusion::execution::context::SessionContext::new();
    let hit_expr = micromegas_analytics::lakehouse::query_deny_list::compile_match_expr(
        &ctx,
        "client = 'hit'",
    )?;
    let hit_row = list
        .insert("client = 'hit'", hit_expr, "db test: hit rule", &created_by)
        .await?;
    let quiet_expr = micromegas_analytics::lakehouse::query_deny_list::compile_match_expr(
        &ctx,
        "client = 'never-hit'",
    )?;
    list.insert(
        "client = 'never-hit'",
        quiet_expr,
        "db test: quiet rule",
        &created_by,
    )
    .await?;
    list.refresh().await?;

    let attr = micromegas_analytics::lakehouse::query_deny_list::QueryAttribution {
        user_id: "u",
        email: "u@example.com",
        service_account: None,
        client: "hit",
        agent: "none",
        entrypoint: "script",
        session: None,
        notebook: None,
        cell: None,
        client_ip: "10.0.0.1",
        sql: "SELECT 1",
        sql_hash: "0000000000000000",
    };
    // Several hits before the flush -- `refresh` should record the most recent, not double-count.
    for _ in 0..3 {
        let matched = list.check(&attr).expect("hit rule should match");
        matched.record_hit();
    }
    let before_flush = Utc::now();
    list.refresh().await?;

    let hit_last_hit_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query("SELECT last_hit_at FROM query_deny_list WHERE rule_id = $1")
            .bind(hit_row.rule_id)
            .fetch_one(&pool)
            .await?
            .try_get("last_hit_at")?;
    let hit_last_hit_at = hit_last_hit_at.expect("hit rule should have a flushed last_hit_at");
    assert!(
        hit_last_hit_at >= before_flush - chrono::Duration::seconds(2),
        "last_hit_at should be close to the flush time"
    );

    let never_hit_last_hit_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query(
        "SELECT last_hit_at FROM query_deny_list WHERE created_by = $1 AND match_expr = 'client = ''never-hit'''",
    )
    .bind(&created_by)
    .fetch_one(&pool)
    .await?
    .try_get("last_hit_at")?;
    assert!(
        never_hit_last_hit_at.is_none(),
        "a rule not hit this tick should not be written at all"
    );

    cleanup(&pool, &created_by).await;
    Ok(())
}

#[ignore]
#[tokio::test]
async fn refresh_failure_keeps_previous_snapshot() -> Result<()> {
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let dedicated_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&connection_string)
        .await?;
    migrate_lakehouse(dedicated_pool.clone()).await?;
    let created_by = format!("query_deny_list_db_test-{}", Uuid::new_v4());
    let list = QueryDenyList::new(dedicated_pool.clone());

    let ctx = datafusion::execution::context::SessionContext::new();
    let compiled = micromegas_analytics::lakehouse::query_deny_list::compile_match_expr(
        &ctx,
        "client = 'still-standing'",
    )?;
    list.insert(
        "client = 'still-standing'",
        compiled,
        "db test",
        &created_by,
    )
    .await?;
    list.refresh().await?;

    let attr = micromegas_analytics::lakehouse::query_deny_list::QueryAttribution {
        user_id: "u",
        email: "u@example.com",
        service_account: None,
        client: "still-standing",
        agent: "none",
        entrypoint: "script",
        session: None,
        notebook: None,
        cell: None,
        client_ip: "10.0.0.1",
        sql: "SELECT 1",
        sql_hash: "0000000000000000",
    };
    assert!(
        list.check(&attr).is_some(),
        "rule should match before the pool closes"
    );

    // Closing the pool this `QueryDenyList` was built with makes the next `refresh` fail --
    // fail-open means the previous, still-matching snapshot survives.
    dedicated_pool.close().await;
    assert!(
        list.refresh().await.is_err(),
        "refresh against a closed pool should fail"
    );
    assert!(
        list.check(&attr).is_some(),
        "a failed refresh must keep the previous snapshot (fail-open)"
    );

    // Cleanup via a fresh pool, since `dedicated_pool` is now closed.
    let cleanup_pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&connection_string)
        .await?;
    cleanup(&cleanup_pool, &created_by).await;
    Ok(())
}

#[ignore]
#[tokio::test]
async fn a_rule_that_fails_to_compile_is_skipped_others_stay_enforced() -> Result<()> {
    let pool = connect().await?;
    migrate_lakehouse(pool.clone()).await?;
    let created_by = format!("query_deny_list_db_test-{}", Uuid::new_v4());
    let list = QueryDenyList::new(pool.clone());

    let ctx = datafusion::execution::context::SessionContext::new();
    let good_expr = micromegas_analytics::lakehouse::query_deny_list::compile_match_expr(
        &ctx,
        "client = 'good'",
    )?;
    let good_row = list
        .insert(
            "client = 'good'",
            good_expr,
            "db test: good rule",
            &created_by,
        )
        .await?;

    // Written directly, simulating a row from a newer replica whose `match_expr` this version
    // cannot compile (an unregistered function).
    let bad_rule_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO query_deny_list (rule_id, created_at, created_by, reason, match_expr) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(bad_rule_id)
    .bind(Utc::now())
    .bind(&created_by)
    .bind("db test: bad rule")
    .bind("this_function_does_not_exist(sql)")
    .execute(&pool)
    .await?;

    list.refresh().await?;

    let good_attr = micromegas_analytics::lakehouse::query_deny_list::QueryAttribution {
        user_id: "u",
        email: "u@example.com",
        service_account: None,
        client: "good",
        agent: "none",
        entrypoint: "script",
        session: None,
        notebook: None,
        cell: None,
        client_ip: "10.0.0.1",
        sql: "SELECT 1",
        sql_hash: "0000000000000000",
    };
    let matched = list
        .check(&good_attr)
        .expect("the good rule should still be enforced");
    assert_eq!(matched.row.rule_id, good_row.rule_id);

    let all_rows = list.list().await?;
    assert!(
        all_rows.iter().any(|r| r.rule_id == bad_rule_id),
        "the uncompilable row should still be listed (it's not deleted, just not enforced)"
    );

    cleanup(&pool, &created_by).await;
    Ok(())
}
