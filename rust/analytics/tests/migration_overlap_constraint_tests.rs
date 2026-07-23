//! DB-gated smoke tests for the lakehouse migration's `lakehouse_partitions_no_overlap`
//! exclusion constraint (schema v7). Requires a scratch PostgreSQL reachable via
//! `MICROMEGAS_SQL_CONNECTION_STRING`; run explicitly with:
//! `cargo test -p micromegas-analytics --test migration_overlap_constraint_tests -- --ignored`
//! The database is assumed disposable: the test migrates it from scratch and mutates
//! schema state to exercise the detect-then-fail path.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use micromegas_analytics::lakehouse::migration::migrate_lakehouse;

async fn insert_test_partition(
    executor: impl sqlx::PgExecutor<'_>,
    view_instance_id: &str,
    file_path: &str,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    file_schema_hash: Vec<u8>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO lakehouse_partitions VALUES($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 2, $13);",
    )
    .bind("test_view")
    .bind(view_instance_id)
    .bind(begin)
    .bind(end)
    .bind(begin)
    .bind(end)
    .bind(Utc::now())
    .bind(file_path)
    .bind(1024_i64)
    .bind(file_schema_hash)
    .bind(vec![0_u8])
    .bind(10_i64)
    .bind(Some(vec![String::from("insert_time")]))
    .execute(executor)
    .await
    .map(|_| ())
}

#[ignore]
#[tokio::test]
async fn migration_enforces_partition_overlap_constraint() -> Result<()> {
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let pool = sqlx::PgPool::connect(&connection_string)
        .await
        .with_context(|| "connecting to postgres")?;

    // Fresh database migrates all the way to the latest version, including btree_gist and the
    // exclusion constraint.
    migrate_lakehouse(pool.clone())
        .await
        .with_context(|| "migrating fresh lakehouse")?;

    let t0 = Utc::now();
    let hour = TimeDelta::hours(1);

    insert_test_partition(&pool, "global", "a.parquet", t0, t0 + hour, vec![0_u8])
        .await
        .with_context(|| "inserting base partition")?;

    // Adjacent partition sharing a boundary: tstzrange is '[)', must not conflict.
    insert_test_partition(
        &pool,
        "global",
        "b.parquet",
        t0 + hour,
        t0 + hour * 2,
        vec![0_u8],
    )
    .await
    .with_context(|| "inserting adjacent partition")?;

    // Same range, different view_instance_id: must not conflict.
    insert_test_partition(
        &pool,
        "other_instance",
        "c.parquet",
        t0,
        t0 + hour,
        vec![0_u8],
    )
    .await
    .with_context(|| "inserting same-range partition for another instance")?;

    // Same range, same instance, different file_schema_hash: must not conflict -- the legal
    // schema-rollout coexistence state (old-schema partitions linger until
    // retire_incompatible_partitions runs, while new-schema writes overlap them).
    insert_test_partition(&pool, "global", "e.parquet", t0, t0 + hour, vec![1_u8])
        .await
        .with_context(|| "inserting same-range partition under another schema hash")?;

    // Overlapping same-schema partition: must be rejected by the exclusion constraint, reported
    // through the structured constraint name (what insert_partition_transaction matches on).
    let overlap_err = insert_test_partition(
        &pool,
        "global",
        "d.parquet",
        t0 + TimeDelta::minutes(30),
        t0 + TimeDelta::minutes(90),
        vec![0_u8],
    )
    .await
    .expect_err("overlapping partition insert should be rejected");
    let constraint = overlap_err
        .as_database_error()
        .and_then(|db_err| db_err.constraint().map(String::from));
    assert_eq!(
        constraint.as_deref(),
        Some("lakehouse_partitions_no_overlap"),
        "expected the overlap to be rejected by the exclusion constraint, got: {overlap_err:?}"
    );

    // Retire+insert replacement in one transaction (the write path's shape): deleting the
    // contained partitions and inserting a wider one must not self-conflict.
    let mut tr = pool.begin().await?;
    sqlx::query("DELETE FROM lakehouse_partitions WHERE view_instance_id = 'global';")
        .execute(&mut *tr)
        .await?;
    insert_test_partition(
        &mut *tr,
        "global",
        "merged.parquet",
        t0,
        t0 + hour * 2,
        vec![0_u8],
    )
    .await
    .with_context(|| "inserting replacement partition in retire+insert transaction")?;
    tr.commit().await?;

    // Detect-then-fail: rewind the schema to v6 (drop the v7 column and constraint), plant an
    // overlapping same-schema pair plus an overlapping *cross-schema* pair, and verify the
    // migration refuses to add the constraint naming only the same-schema conflict -- the
    // cross-schema overlap is the legal schema-rollout coexistence state and must be ignored
    // by the detector, matching the constraint's file_schema_hash scoping.
    sqlx::query(
        "ALTER TABLE lakehouse_partitions DROP CONSTRAINT lakehouse_partitions_no_overlap;",
    )
    .execute(&pool)
    .await?;
    sqlx::query("ALTER TABLE lakehouse_partitions DROP COLUMN sort_order;")
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE lakehouse_migration SET version = 6;")
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO lakehouse_partitions VALUES
         ('test_view', 'global', $1, $2, $1, $2, $1, 'x.parquet', 1024, '\\x00', '\\x00', 10, 2),
         ('test_view', 'global', $3, $4, $3, $4, $3, 'y.parquet', 1024, '\\x00', '\\x00', 10, 2),
         ('test_view', 'global', $5, $6, $5, $6, $5, 'w.parquet', 1024, '\\x00', '\\x00', 10, 2),
         ('test_view', 'global', $7, $8, $7, $8, $7, 'z.parquet', 1024, '\\x01', '\\x00', 10, 2);",
    )
    .bind(t0 + hour * 3)
    .bind(t0 + hour * 5)
    .bind(t0 + hour * 4)
    .bind(t0 + hour * 6)
    .bind(t0 + hour * 8)
    .bind(t0 + hour * 10)
    .bind(t0 + hour * 9)
    .bind(t0 + hour * 11)
    .execute(&pool)
    .await
    .with_context(|| "planting overlapping pairs for detect-then-fail")?;

    let migration_err = migrate_lakehouse(pool.clone())
        .await
        .expect_err("migration should refuse to add the constraint over overlapping partitions");
    let msg = format!("{migration_err:?}");
    assert!(
        msg.contains("overlapping insert-time ranges") && msg.contains("x.parquet"),
        "expected a legible detect-then-fail error naming the conflicting rows, got: {msg}"
    );
    assert!(
        !msg.contains("w.parquet") && !msg.contains("z.parquet"),
        "the cross-schema overlap must not be reported as a conflict, got: {msg}"
    );

    // Clean up only the same-schema overlap; the cross-schema pair stays, and the migration
    // (constraint included) must complete over it.
    sqlx::query("DELETE FROM lakehouse_partitions WHERE file_path IN ('x.parquet', 'y.parquet');")
        .execute(&pool)
        .await?;
    migrate_lakehouse(pool.clone())
        .await
        .with_context(|| "re-running migration after retiring the same-schema overlap")?;
    Ok(())
}
