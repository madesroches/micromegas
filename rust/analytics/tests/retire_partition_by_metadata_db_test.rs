//! DB-backed tests for `retire_partition_by_metadata`'s required, disambiguating fifth
//! `file_schema_hash` argument. `migration.rs`'s `lakehouse_partitions_no_overlap` exclusion
//! constraint is scoped *by* `file_schema_hash`, so an old-schema partition and a current-schema
//! partition may legally share every other column -- the scenarios below insert such synthetic
//! collisions directly into `lakehouse_partitions` and drive the UDF through SQL on an admin
//! `SessionContext`, exactly as `retire_incompatible_partitions` does in production.
//!
//! `#[ignore]`d, requires a live `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI`
//! -- mirrors `list_audience_grants_db_test.rs`'s convention; does not run under a plain
//! `cargo test`.

mod common;

use anyhow::{Context, Result};
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use common::db_fixtures::ensure_telemetry_guard;
use datafusion::arrow::array::{Array, RecordBatch, StringArray};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::NullPartitionProvider;
use micromegas_analytics::lakehouse::query::make_session_context;
use micromegas_analytics::lakehouse::read_scope::{CallerContext, IsolationConfig, ReadScope};
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

const VIEW_SET_NAME: &str = "log_entries";

async fn lakehouse() -> Result<Arc<LakehouseContext>> {
    ensure_telemetry_guard();
    LakehouseContext::from_env().await
}

fn admin_caller() -> CallerContext {
    CallerContext {
        read_scope: ReadScope::All,
        is_admin: true,
        isolation_config: Arc::new(IsolationConfig::default()),
        admin_principal_possible: true,
        identity: None,
        grant_selectors: Vec::<String>::new().into(),
    }
}

/// A fresh, random `view_instance_id` per test: these tests share a persistent dev lake with
/// every other test and prior run.
fn instance_id() -> String {
    format!("retire-partition-by-metadata-db-test-{}", Uuid::new_v4())
}

/// A base instant truncated to microsecond precision: Postgres `timestamptz` columns only store
/// microseconds, so a nanosecond-precision `Utc::now()` would never compare equal to the same
/// value read back out of `lakehouse_partitions`, and the `TIMESTAMP` literal round-trips through
/// SQL text too.
fn base_time() -> Result<DateTime<Utc>> {
    Ok((Utc::now() - TimeDelta::minutes(30)).duration_trunc(TimeDelta::microseconds(1))?)
}

/// Inserts a synthetic partition row directly, bypassing the write path so a collision that would
/// otherwise require two concurrent writers racing the exclusion constraint can be set up
/// deterministically. `file_path` is `None` for an empty partition.
async fn insert_partition_row(
    pool: &sqlx::PgPool,
    view_instance_id: &str,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    file_schema_hash: &[u8],
    file_path: Option<&str>,
) -> Result<()> {
    let file_size: i64 = if file_path.is_some() { 1 } else { 0 };
    sqlx::query(
        "INSERT INTO lakehouse_partitions
         (view_set_name, view_instance_id, begin_insert_time, end_insert_time,
          min_event_time, max_event_time, updated, file_path, file_size,
          file_schema_hash, source_data_hash, num_rows)
         VALUES ($1, $2, $3, $4, $3, $4, NOW(), $5, $6, $7, $8, 1);",
    )
    .bind(VIEW_SET_NAME)
    .bind(view_instance_id)
    .bind(begin)
    .bind(end)
    .bind(file_path)
    .bind(file_size)
    .bind(file_schema_hash)
    .bind(1i64.to_le_bytes().to_vec())
    .execute(pool)
    .await
    .with_context(|| "inserting synthetic partition row")?;
    Ok(())
}

async fn surviving_hashes(pool: &sqlx::PgPool, view_instance_id: &str) -> Result<Vec<Vec<u8>>> {
    let rows: Vec<(Vec<u8>,)> = sqlx::query_as(
        "SELECT file_schema_hash FROM lakehouse_partitions
         WHERE view_set_name = $1 AND view_instance_id = $2
         ORDER BY file_schema_hash;",
    )
    .bind(VIEW_SET_NAME)
    .bind(view_instance_id)
    .fetch_all(pool)
    .await
    .with_context(|| "listing surviving partitions")?;
    Ok(rows.into_iter().map(|(hash,)| hash).collect())
}

async fn temporary_files_count(pool: &sqlx::PgPool, file_path: &str) -> Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM temporary_files WHERE file_path = $1")
        .bind(file_path)
        .fetch_one(pool)
        .await
        .with_context(|| "counting temporary_files rows")?;
    Ok(row.try_get("n")?)
}

async fn cleanup(pool: &sqlx::PgPool, view_instance_id: &str) {
    let _ = sqlx::query(
        "DELETE FROM lakehouse_partitions WHERE view_set_name = $1 AND view_instance_id = $2",
    )
    .bind(VIEW_SET_NAME)
    .bind(view_instance_id)
    .execute(pool)
    .await;
}

/// Calls `retire_partition_by_metadata` through SQL on a fresh admin `SessionContext`, returning
/// the first result string. On error the UDF appends a trailing `ROLLED_BACK: ...` marker to the
/// same result array, so a single-row input can legally produce two output rows; this only checks
/// that the second row, when present, is that marker.
async fn retire(
    lakehouse: Arc<LakehouseContext>,
    view_instance_id: &str,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    file_schema_hash_hex: &str,
) -> Result<String> {
    let ctx = make_session_context(
        lakehouse,
        Arc::new(NullPartitionProvider {}),
        None,
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        admin_caller(),
    )
    .await
    .with_context(|| "make_session_context")?;
    let sql = format!(
        "SELECT retire_partition_by_metadata('{VIEW_SET_NAME}', '{view_instance_id}', TIMESTAMP '{}', TIMESTAMP '{}', decode('{file_schema_hash_hex}', 'hex')) as result",
        begin.to_rfc3339(),
        end.to_rfc3339(),
    );
    let batches: Vec<RecordBatch> = ctx.sql(&sql).await?.collect().await?;
    let mut results = Vec::new();
    for batch in &batches {
        let col = batch
            .column_by_name("result")
            .expect("result column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("result is Utf8");
        for i in 0..batch.num_rows() {
            results.push(col.value(i).to_string());
        }
    }
    assert!(!results.is_empty(), "expected at least one result row");
    if let Some(second) = results.get(1) {
        assert!(
            second.starts_with("ROLLED_BACK:"),
            "unexpected second result row: {second}"
        );
    }
    Ok(results.into_iter().next().expect("one result"))
}

/// Two rows sharing a zero-width range and the same `file_schema_hash` collide: the `SELECT`'s
/// `file_schema_hash = $5` predicate cannot disambiguate them, since they already share it.
#[ignore]
#[tokio::test]
async fn zero_width_collision_same_hash_is_ambiguous() -> Result<()> {
    let lakehouse = lakehouse().await?;
    let pool = lakehouse.lake().db_pool.clone();
    let view_instance_id = instance_id();
    let point = base_time()?;

    let file_a = format!("test/{view_instance_id}/a-{}.parquet", Uuid::new_v4());
    let file_b = format!("test/{view_instance_id}/b-{}.parquet", Uuid::new_v4());
    insert_partition_row(
        &pool,
        &view_instance_id,
        point,
        point,
        &[4u8],
        Some(&file_a),
    )
    .await?;
    insert_partition_row(
        &pool,
        &view_instance_id,
        point,
        point,
        &[4u8],
        Some(&file_b),
    )
    .await?;

    let result = retire(lakehouse.clone(), &view_instance_id, point, point, "04").await?;
    assert!(
        result.starts_with("ERROR:"),
        "expected an ambiguity error, got: {result}"
    );
    assert!(
        result.contains("Ambiguous match"),
        "expected the ambiguity to be named, got: {result}"
    );

    assert_eq!(
        surviving_hashes(&pool, &view_instance_id).await?.len(),
        2,
        "both colliding rows must survive an ambiguous match"
    );
    assert_eq!(temporary_files_count(&pool, &file_a).await?, 0);
    assert_eq!(temporary_files_count(&pool, &file_b).await?, 0);

    cleanup(&pool, &view_instance_id).await;
    Ok(())
}

/// The headline regression test: two rows share every metadata column except `file_schema_hash`
/// (legal under the exclusion constraint, which is scoped by hash). The fifth argument must
/// resolve to exactly one of them, leaving the other untouched.
#[ignore]
#[tokio::test]
async fn collision_five_arguments_retires_only_matching_hash() -> Result<()> {
    let lakehouse = lakehouse().await?;
    let pool = lakehouse.lake().db_pool.clone();
    let view_instance_id = instance_id();
    let t0 = base_time()?;
    let t1 = t0 + TimeDelta::seconds(10);

    let file_old = format!("test/{view_instance_id}/old-{}.parquet", Uuid::new_v4());
    let file_new = format!("test/{view_instance_id}/new-{}.parquet", Uuid::new_v4());
    insert_partition_row(&pool, &view_instance_id, t0, t1, &[4u8], Some(&file_old)).await?;
    insert_partition_row(&pool, &view_instance_id, t0, t1, &[5u8], Some(&file_new)).await?;

    let result = retire(lakehouse.clone(), &view_instance_id, t0, t1, "04").await?;
    assert!(
        result.starts_with("SUCCESS:"),
        "expected success, got: {result}"
    );

    let survivors = surviving_hashes(&pool, &view_instance_id).await?;
    assert_eq!(
        survivors,
        vec![vec![5u8]],
        "only the [5] row should survive"
    );
    assert_eq!(
        temporary_files_count(&pool, &file_old).await?,
        1,
        "the retired [4] file should be queued for cleanup"
    );
    assert_eq!(
        temporary_files_count(&pool, &file_new).await?,
        0,
        "the surviving [5] file must not be queued for cleanup"
    );

    cleanup(&pool, &view_instance_id).await;
    Ok(())
}

/// The unchanged happy path: a single, unambiguous partition retires successfully when the
/// caller's hash matches.
#[ignore]
#[tokio::test]
async fn unique_partition_five_arguments_retires() -> Result<()> {
    let lakehouse = lakehouse().await?;
    let pool = lakehouse.lake().db_pool.clone();
    let view_instance_id = instance_id();
    let t0 = base_time()?;
    let t1 = t0 + TimeDelta::seconds(10);

    let file_path = format!("test/{view_instance_id}/{}.parquet", Uuid::new_v4());
    insert_partition_row(&pool, &view_instance_id, t0, t1, &[4u8], Some(&file_path)).await?;

    let result = retire(lakehouse.clone(), &view_instance_id, t0, t1, "04").await?;
    assert!(
        result.starts_with("SUCCESS:"),
        "expected success, got: {result}"
    );
    assert!(
        surviving_hashes(&pool, &view_instance_id).await?.is_empty(),
        "the sole partition should be retired"
    );
    assert_eq!(temporary_files_count(&pool, &file_path).await?, 1);

    cleanup(&pool, &view_instance_id).await;
    Ok(())
}

/// A hash that doesn't match any row is treated exactly like no matching row at all -- the
/// existing partition, under a different hash, must be left untouched.
#[ignore]
#[tokio::test]
async fn five_arguments_hash_mismatch_is_not_found() -> Result<()> {
    let lakehouse = lakehouse().await?;
    let pool = lakehouse.lake().db_pool.clone();
    let view_instance_id = instance_id();
    let t0 = base_time()?;
    let t1 = t0 + TimeDelta::seconds(10);

    let file_path = format!("test/{view_instance_id}/{}.parquet", Uuid::new_v4());
    insert_partition_row(&pool, &view_instance_id, t0, t1, &[4u8], Some(&file_path)).await?;

    let result = retire(lakehouse.clone(), &view_instance_id, t0, t1, "05").await?;
    assert!(
        result.starts_with("ERROR:"),
        "expected an error, got: {result}"
    );
    assert!(
        result.contains("Partition not found"),
        "expected a not-found error, got: {result}"
    );

    let survivors = surviving_hashes(&pool, &view_instance_id).await?;
    assert_eq!(
        survivors,
        vec![vec![4u8]],
        "the existing row must be untouched by a mismatched hash"
    );
    assert_eq!(temporary_files_count(&pool, &file_path).await?, 0);

    cleanup(&pool, &view_instance_id).await;
    Ok(())
}

/// An empty partition (`file_path IS NULL`) retires cleanly with no `temporary_files` insert,
/// confirming the `fetch_optional` -> `fetch_all` switch didn't regress the NULL-file path.
#[ignore]
#[tokio::test]
async fn empty_partition_retires_with_no_cleanup_row() -> Result<()> {
    let lakehouse = lakehouse().await?;
    let pool = lakehouse.lake().db_pool.clone();
    let view_instance_id = instance_id();
    let t0 = base_time()?;
    let t1 = t0 + TimeDelta::seconds(10);

    insert_partition_row(&pool, &view_instance_id, t0, t1, &[4u8], None).await?;

    let before: i64 = sqlx::query("SELECT COUNT(*) AS n FROM temporary_files")
        .fetch_one(&pool)
        .await?
        .try_get("n")?;

    let result = retire(lakehouse.clone(), &view_instance_id, t0, t1, "04").await?;
    assert!(
        result.starts_with("SUCCESS:"),
        "expected success, got: {result}"
    );
    assert!(
        surviving_hashes(&pool, &view_instance_id).await?.is_empty(),
        "the empty partition should be retired"
    );

    let after: i64 = sqlx::query("SELECT COUNT(*) AS n FROM temporary_files")
        .fetch_one(&pool)
        .await?
        .try_get("n")?;
    assert_eq!(
        after, before,
        "an empty partition (file_path IS NULL) must not enqueue a temporary_files row"
    );

    cleanup(&pool, &view_instance_id).await;
    Ok(())
}
