//! Periodic self-observability collector for the metadata Postgres:
//! samples the standard `pg_stat_*` catalog views (plus index/table sizes) and
//! emits the readings as micromegas metrics through the process's own tracing
//! sink. This turns questions that today can only be answered by connecting
//! to the DB (e.g. *which indexes are dead weight*) into hard evidence
//! queryable via FlightSQL.
//!
//! The collector emits **raw cumulative counters** exactly as Postgres reports
//! them and never resets any statistic (no `pg_stat_reset*` call anywhere in
//! this module) — deltas and rates are a query-time concern, and
//! `pg_db_stats_reset_timestamp` lets that analysis segment on counter-reset
//! boundaries.
//!
//! Modeled on `object-cache-srv::saturation_monitor`: a pure sampling function
//! (`collect_pg_stats`) split from the scheduling plumbing (`PgStatsTask`), so
//! it can be driven directly by tests.

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_tracing::intern_string::intern_string;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set::{Property, PropertySet};
use sqlx::Row;
use sqlx::postgres::PgRow;
use std::sync::Arc;

use super::cron_task::TaskCallback;

/// Reads a `bigint` counter column as `u64`, defaulting to `0` when it's
/// `NULL`. Postgres's `pg_stat_user_tables`/`pg_stat_user_indexes` counters
/// can read `NULL` rather than `0` on a freshly created relation, before the
/// stats collector's first report — this is a real, observed condition (not
/// a defensive guess), not merely a schema nullability quirk.
fn get_counter(row: &PgRow, col: &str) -> Result<u64> {
    let value: Option<i64> = row.try_get(col).with_context(|| col.to_string())?;
    Ok(value.unwrap_or(0) as u64)
}

/// `{table}` tags for a per-table metric. `table` is a runtime string (read
/// from `pg_stat_user_tables`) — the metadata schema's relation set is fixed
/// and small (see the collector's design notes), so interning it is a bounded,
/// one-time-per-distinct-name leak.
fn table_tags(table: &str) -> &'static PropertySet {
    PropertySet::find_or_create(vec![Property::new("table", intern_string(table))])
}

/// `{table, index}` tags for a per-index metric.
fn index_tags(table: &str, index: &str) -> &'static PropertySet {
    PropertySet::find_or_create(vec![
        Property::new("table", intern_string(table)),
        Property::new("index", intern_string(index)),
    ])
}

/// `{state}` tags for a `pg_stat_activity` connection-count metric.
fn state_tags(state: &str) -> &'static PropertySet {
    PropertySet::find_or_create(vec![Property::new("state", intern_string(state))])
}

/// Per-index metrics: `pg_stat_user_indexes` joined with
/// `pg_relation_size(indexrelid)`, tagged `{table, index}`.
async fn collect_index_stats(pool: &sqlx::PgPool) -> Result<()> {
    let rows = sqlx::query(
        "SELECT relname AS table_name,
                indexrelname AS index_name,
                idx_scan,
                idx_tup_read,
                idx_tup_fetch,
                pg_relation_size(indexrelid) AS index_size_bytes
         FROM pg_stat_user_indexes",
    )
    .fetch_all(pool)
    .await
    .with_context(|| "querying pg_stat_user_indexes")?;

    for row in rows {
        let table: String = row.try_get("table_name").with_context(|| "table_name")?;
        let index: String = row.try_get("index_name").with_context(|| "index_name")?;
        let tags = index_tags(&table, &index);

        imetric!(
            "pg_index_scans",
            "count",
            tags,
            get_counter(&row, "idx_scan")?
        );
        imetric!(
            "pg_index_tuples_read",
            "count",
            tags,
            get_counter(&row, "idx_tup_read")?
        );
        imetric!(
            "pg_index_tuples_fetched",
            "count",
            tags,
            get_counter(&row, "idx_tup_fetch")?
        );
        imetric!(
            "pg_index_size_bytes",
            "bytes",
            tags,
            get_counter(&row, "index_size_bytes")?
        );
    }
    Ok(())
}

/// Per-table metrics: `pg_stat_user_tables`, tagged `{table}`.
async fn collect_table_stats(pool: &sqlx::PgPool) -> Result<()> {
    let rows = sqlx::query(
        "SELECT relname AS table_name,
                seq_scan,
                idx_scan,
                n_live_tup,
                n_dead_tup,
                n_tup_ins,
                n_tup_upd,
                n_tup_del,
                extract(epoch from now() - last_autovacuum)::float8 AS seconds_since_autovacuum
         FROM pg_stat_user_tables",
    )
    .fetch_all(pool)
    .await
    .with_context(|| "querying pg_stat_user_tables")?;

    for row in rows {
        let table: String = row.try_get("table_name").with_context(|| "table_name")?;
        let tags = table_tags(&table);

        imetric!(
            "pg_table_seq_scans",
            "count",
            tags,
            get_counter(&row, "seq_scan")?
        );
        imetric!(
            "pg_table_idx_scans",
            "count",
            tags,
            get_counter(&row, "idx_scan")?
        );
        imetric!(
            "pg_table_live_tuples",
            "count",
            tags,
            get_counter(&row, "n_live_tup")?
        );
        imetric!(
            "pg_table_dead_tuples",
            "count",
            tags,
            get_counter(&row, "n_dead_tup")?
        );
        imetric!(
            "pg_table_tuples_inserted",
            "count",
            tags,
            get_counter(&row, "n_tup_ins")?
        );
        imetric!(
            "pg_table_tuples_updated",
            "count",
            tags,
            get_counter(&row, "n_tup_upd")?
        );
        imetric!(
            "pg_table_tuples_deleted",
            "count",
            tags,
            get_counter(&row, "n_tup_del")?
        );

        let seconds_since_autovacuum: Option<f64> = row
            .try_get("seconds_since_autovacuum")
            .with_context(|| "seconds_since_autovacuum")?;
        if let Some(age) = seconds_since_autovacuum {
            fmetric!("pg_table_seconds_since_autovacuum", "seconds", tags, age);
        }
    }
    Ok(())
}

/// Database-wide metrics: `pg_stat_database` filtered to `current_database()`
/// (single row), untagged.
async fn collect_database_stats(pool: &sqlx::PgPool) -> Result<()> {
    let row = sqlx::query(
        "SELECT blks_hit,
                blks_read,
                xact_commit,
                xact_rollback,
                deadlocks,
                temp_bytes,
                extract(epoch from stats_reset)::float8 AS stats_reset_epoch
         FROM pg_stat_database
         WHERE datname = current_database()",
    )
    .fetch_one(pool)
    .await
    .with_context(|| "querying pg_stat_database")?;

    imetric!("pg_db_blocks_hit", "count", get_counter(&row, "blks_hit")?);
    imetric!(
        "pg_db_blocks_read",
        "count",
        get_counter(&row, "blks_read")?
    );
    imetric!(
        "pg_db_xact_commit",
        "count",
        get_counter(&row, "xact_commit")?
    );
    imetric!(
        "pg_db_xact_rollback",
        "count",
        get_counter(&row, "xact_rollback")?
    );
    imetric!("pg_db_deadlocks", "count", get_counter(&row, "deadlocks")?);
    imetric!(
        "pg_db_temp_bytes",
        "bytes",
        get_counter(&row, "temp_bytes")?
    );

    let stats_reset_epoch: Option<f64> = row
        .try_get("stats_reset_epoch")
        .with_context(|| "stats_reset_epoch")?;
    if let Some(epoch) = stats_reset_epoch {
        fmetric!("pg_db_stats_reset_timestamp", "seconds", epoch);
    }
    Ok(())
}

/// Point-in-time activity metrics: connection counts per `state` and the age
/// of the oldest in-progress transaction, both from `pg_stat_activity`
/// filtered to `current_database()`.
async fn collect_activity_stats(pool: &sqlx::PgPool) -> Result<()> {
    let state_rows = sqlx::query(
        "SELECT coalesce(state, 'unknown') AS state, count(*) AS connections
         FROM pg_stat_activity
         WHERE datname = current_database()
         GROUP BY 1",
    )
    .fetch_all(pool)
    .await
    .with_context(|| "querying pg_stat_activity (by state)")?;

    for row in state_rows {
        let state: String = row.try_get("state").with_context(|| "state")?;
        imetric!(
            "pg_activity_connections",
            "count",
            state_tags(&state),
            get_counter(&row, "connections")?
        );
    }

    let oldest_xact_row = sqlx::query(
        "SELECT extract(epoch from now() - min(xact_start))::float8 AS oldest_xact_age_seconds
         FROM pg_stat_activity
         WHERE datname = current_database()",
    )
    .fetch_one(pool)
    .await
    .with_context(|| "querying pg_stat_activity (oldest xact)")?;

    let oldest_xact_age_seconds: Option<f64> = oldest_xact_row
        .try_get("oldest_xact_age_seconds")
        .with_context(|| "oldest_xact_age_seconds")?;
    if let Some(age) = oldest_xact_age_seconds {
        fmetric!("pg_activity_oldest_xact_age_seconds", "seconds", age);
    }
    Ok(())
}

/// Client-side connection-pool gauges. No query: read directly off the
/// `sqlx::PgPool` handle.
fn collect_pool_stats(pool: &sqlx::PgPool) {
    imetric!("pg_pool_size", "count", pool.size() as u64);
    imetric!("pg_pool_idle", "count", pool.num_idle() as u64);
}

/// Samples all `pg_stat_*` families and emits them as metrics. Never issues a
/// `pg_stat_reset*` call — only `SELECT`s.
///
/// If a single catalog query fails, it's logged and the others still run (a
/// transient catalog hiccup shouldn't drop the whole tick); the first error
/// encountered is returned at the end via `anyhow` context.
pub async fn collect_pg_stats(pool: &sqlx::PgPool) -> Result<()> {
    let mut first_err: Option<anyhow::Error> = None;

    if let Err(e) = collect_index_stats(pool).await {
        error!("pg_stats: {e:?}");
        first_err.get_or_insert(e);
    }
    if let Err(e) = collect_table_stats(pool).await {
        error!("pg_stats: {e:?}");
        first_err.get_or_insert(e);
    }
    if let Err(e) = collect_database_stats(pool).await {
        error!("pg_stats: {e:?}");
        first_err.get_or_insert(e);
    }
    if let Err(e) = collect_activity_stats(pool).await {
        error!("pg_stats: {e:?}");
        first_err.get_or_insert(e);
    }
    collect_pool_stats(pool);

    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Cron task wiring: runs `collect_pg_stats` against the lakehouse's metadata
/// DB pool once per tick.
pub struct PgStatsTask {
    pub lakehouse: Arc<LakehouseContext>,
}

#[async_trait]
impl TaskCallback for PgStatsTask {
    #[span_fn]
    async fn run(&self, _task_scheduled_time: DateTime<Utc>) -> Result<()> {
        collect_pg_stats(&self.lakehouse.lake().db_pool).await
    }
}
