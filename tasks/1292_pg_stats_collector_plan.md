# Postgres `pg_stat_*` Self-Observability Collector Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1292

## Overview

Add a periodic collector to the maintenance daemon that samples PostgreSQL's standard `pg_stat_*`
views (plus index/table sizes) once a minute and emits the readings as micromegas metrics through the
daemon's own tracing sink. This gives runtime self-observability of the metadata Postgres — turning
questions we can currently only answer statically (e.g. *which indexes are dead weight*) into hard
evidence queryable via FlightSQL. The metadata DB runs on Aurora Serverless v2, so the in-DB
`pg_stat_*` views are the portable primary source with no AWS credentials required.

The collector emits **raw cumulative counters** and **never resets Postgres statistics** — deltas and
rates are computed at query time, and we capture `pg_stat_database.stats_reset` so analysis can
segment on counter-reset boundaries. Scope is Phase 1 only (in-DB views, no new dependencies);
`pg_stat_statements` and Aurora/CloudWatch signals are explicit follow-ups tracked elsewhere.

## Current State

### Maintenance daemon and cron framework

`rust/public/src/servers/maintenance.rs` hosts the maintenance daemon. `daemon()` (lines 293–369)
constructs four `CronTask`s (`every_day`, `every_hour`, `every minute`, `every second`), each backed
by a `TaskCallback` impl (`EveryDayTask`, `EveryHourTask`, `EveryMinuteTask`, `EverySecondTask`), and
spawns one `run_tasks_forever` runner loop per cadence via a `JoinSet`, wiring each to a
`ShutdownFanout` subscriber for graceful drain.

`rust/public/src/servers/cron_task.rs` defines the framework:
- `TaskCallback` trait (`async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()>`).
- `CronTask::new(name, period, offset, callback)` computes `next_run` and is polled by the runner.
- `CronTask::spawn()` records `task_tick_delay`/`task_tick_latency` metrics and runs the callback
  inside `spawn_with_context` (so the callback's telemetry is attributed correctly).

The existing `EveryMinuteTask` (period `1min`, offset `30s`) is dedicated to view materialization —
we add a *separate* task rather than piggybacking, since this is a distinct concern.

### DB pool access

`LakehouseContext::lake()` (`analytics/src/lakehouse/lakehouse_context.rs:105`) returns
`&Arc<DataLakeConnection>`, whose `db_pool` field (`ingestion/src/data_lake_connection.rs:14`) is a
`sqlx::PgPool`. `PgPool` exposes `.size()` and `.num_idle()` for the client-side pool metrics.
Queries in the codebase use the untyped `sqlx::query(...).bind(...).fetch_all(pool)` +
`row.try_get::<T, _>("col")` pattern (e.g. `partition_cache.rs:54`); no compile-time `sqlx::query!`
macros are used, so the collector's queries follow the same runtime pattern.

### Metric macros and tagged (per-relation) metrics

`imetric!`/`fmetric!` (`tracing/src/macros.rs:163`, `:203`) each have a 3-arg (untagged) and a 4-arg
(tagged) form. The tagged form takes a `&'static PropertySet` as the third argument:

```rust
imetric!("name", "unit", property_set, value);   // property_set: &'static PropertySet
```

`PropertySet::find_or_create(Vec<Property>) -> &'static Self` interns the set
(`tracing/src/property_set.rs:43`). Crucially, `Property::new(name, value)` requires **both** args to
be `&'static str` (`property_set.rs:19`) — property values are statically allocated by design (see
the module's "The user is expected to manage the cardinality" note). Runtime strings (table/index
names read from Postgres) therefore cannot be passed directly.

The idiomatic escape hatch already exists: `intern_string(&str) -> &'static str`
(`tracing/src/intern_string.rs:5`), which interns into a never-released global container. Because the
set of user tables/indexes in the metadata schema is small and stable (see cardinality below), the
one-time-per-distinct-name leak is bounded and safe. The `object-cache/src/metric_tags.rs` module is
the in-repo precedent for building interned `PropertySet`s for tagged metrics.

### Cardinality is bounded

The metadata schema is fixed by `ingestion/src/sql_telemetry_db.rs` + `sql_migration.rs`: tables
`processes`, `streams`, `blocks`, `payloads`, `lakehouse_partitions`, the `screens_*` tables, etc.,
with ~20–30 named indexes total (`process_id`, `parent_process_id`, `block_begin_time`,
`block_end_time`, `screens_screen_type`, `screens_created_at`, …). `pg_stat_user_indexes` /
`pg_stat_user_tables` enumerate exactly this bounded set, so tagging per relation is safe.

### How emitted metrics reach the lakehouse

The maintenance daemon's `main` is annotated `#[micromegas_main]`
(`telemetry-maintenance-srv/src/main.rs:31`), which installs the tracing sink that ships the
process's own telemetry to ingestion. Metrics emitted by the collector therefore land in the lake's
`measures` view like any other process telemetry — no extra wiring. In monolith mode
(`monolith/src/main.rs:284`) the same `daemon()` runs in-process, equally instrumented.

### Query surface

The `measures` view schema (`analytics/src/metrics_table.rs:19`) has columns `time`, `target`,
`name`, `unit`, `value` (Float64), and a JSONB `properties` column. Tagged properties are read in SQL
via the `property_get("properties", 'table')` UDF (`analytics/src/lakehouse/mod.rs:87`,
usage e.g. `process_streams.rs:12`). So per-relation series are recovered with
`property_get("properties", 'index')` / `'table'` / `'state'`.

## Design

### Module layout

Following the `object-cache-srv/src/saturation_monitor.rs` precedent (a pure sampling function split
from the spawn/loop plumbing, so it can be unit-tested directly), add a new module
`rust/public/src/servers/pg_stats.rs`:

- `pub async fn collect_pg_stats(pool: &sqlx::PgPool) -> Result<()>` — runs the catalog queries and
  emits all metrics. Pure with respect to scheduling; the integration test drives it directly.
- `pub struct PgStatsTask { pub lakehouse: Arc<LakehouseContext> }` implementing `TaskCallback`,
  whose `run()` calls `collect_pg_stats(&self.lakehouse.lake().db_pool)`.

`maintenance.rs` only gains the wiring in `daemon()` (construct the `CronTask`, spawn a runner). This
keeps `maintenance.rs` focused on orchestration and the collection logic isolated and testable
(open/closed: no existing task is modified).

Register the module in `rust/public/src/servers/mod.rs`.

### Interned property-set helper

Add a small helper in `pg_stats.rs` to build per-relation tag sets from runtime names:

```rust
fn table_tags(table: &str) -> &'static PropertySet {
    PropertySet::find_or_create(vec![Property::new("table", intern_string(table))])
}
fn index_tags(table: &str, index: &str) -> &'static PropertySet {
    PropertySet::find_or_create(vec![
        Property::new("table", intern_string(table)),
        Property::new("index", intern_string(index)),
    ])
}
fn state_tags(state: &str) -> &'static PropertySet {
    PropertySet::find_or_create(vec![Property::new("state", intern_string(state))])
}
```

`"table"`/`"index"`/`"state"` keys are string literals (already `&'static`); only the runtime values
need `intern_string`.

### Metrics emitted

All counters are emitted **raw and cumulative** (as reported by Postgres). Units follow existing
conventions (`"count"`, `"bytes"`, `"seconds"`). Naming uses a `pg_` prefix, mirroring the
`object_cache_*` convention.

**Per index** — `pg_stat_user_indexes` joined with `pg_relation_size(indexrelid)`, tagged
`{table, index}`:

| Metric | Unit | Source column |
|---|---|---|
| `pg_index_scans` | count | `idx_scan` |
| `pg_index_tuples_read` | count | `idx_tup_read` |
| `pg_index_tuples_fetched` | count | `idx_tup_fetch` |
| `pg_index_size_bytes` | bytes | `pg_relation_size(indexrelid)` |

**Per table** — `pg_stat_user_tables`, tagged `{table}`:

| Metric | Unit | Source column |
|---|---|---|
| `pg_table_seq_scans` | count | `seq_scan` |
| `pg_table_idx_scans` | count | `idx_scan` |
| `pg_table_live_tuples` | count | `n_live_tup` |
| `pg_table_dead_tuples` | count | `n_dead_tup` |
| `pg_table_tuples_inserted` | count | `n_tup_ins` |
| `pg_table_tuples_updated` | count | `n_tup_upd` |
| `pg_table_tuples_deleted` | count | `n_tup_del` |
| `pg_table_seconds_since_autovacuum` | seconds | `extract(epoch from now() - last_autovacuum)` |

`last_autovacuum` is nullable (never autovacuumed) — emit the age metric only when non-NULL rather
than a sentinel, so `WHERE name = 'pg_table_seconds_since_autovacuum'` naturally excludes untouched
tables.

**Database** — `pg_stat_database WHERE datname = current_database()`, untagged (single row):

| Metric | Unit | Source column |
|---|---|---|
| `pg_db_blocks_hit` | count | `blks_hit` |
| `pg_db_blocks_read` | count | `blks_read` |
| `pg_db_xact_commit` | count | `xact_commit` |
| `pg_db_xact_rollback` | count | `xact_rollback` |
| `pg_db_deadlocks` | count | `deadlocks` |
| `pg_db_temp_bytes` | bytes | `temp_bytes` |
| `pg_db_stats_reset_timestamp` | seconds | `extract(epoch from stats_reset)` |

`pg_db_stats_reset_timestamp` is the reset-boundary marker: when it jumps, downstream counters have
been zeroed (clean restart on PG15+, crash, or Aurora failover/patch/instance-replacement). Queries
segment on it instead of only inferring resets from `v[t2] < v[t1]`. Emit only when non-NULL.

**Activity** — `pg_stat_activity WHERE datname = current_database()` (point-in-time):

| Metric | Unit | Query | Tag |
|---|---|---|---|
| `pg_activity_connections` | count | `GROUP BY state`, `count(*)` per state | `{state}` |
| `pg_activity_oldest_xact_age_seconds` | seconds | `extract(epoch from now() - min(xact_start))` | — |

`state` can be NULL — coalesce to `'unknown'` (`WHERE datname = current_database()` already excludes
background workers, which have NULL `datname`). Oldest-xact age is emitted only when at least one
transaction is in progress (`min(xact_start)` non-NULL).

**Client-side pool** — no query, from `sqlx::PgPool`:

| Metric | Unit | Source |
|---|---|---|
| `pg_pool_size` | count | `pool.size()` |
| `pg_pool_idle` | count | `pool.num_idle()` |

### Value type note

`imetric!` takes `u64`. Postgres counters are `bigint` (`i64`); read as `i64` via `try_get` and cast
with `as u64` (all these counters are non-negative). Sizes from `pg_relation_size` are `bigint`
bytes. Epoch/age values are fractional seconds → `fmetric!` (`f64`). Pool sizes are `usize` → `u64`.

### Queries

Five read-only catalog queries per tick (all cheap, no locks beyond catalog snapshots):

1. `pg_stat_user_indexes` ⨝ `pg_relation_size` — one row per index.
2. `pg_stat_user_tables` — one row per table.
3. `pg_stat_database` filtered to `current_database()` — one row.
4. `pg_stat_activity` filtered to `current_database()`, grouped by `coalesce(state,'unknown')`, plus
   `min(xact_start)`. (Can be one query returning state/count rows plus a separate scalar for oldest
   xact, or two small queries — implementer's choice; keep it simple.)
5. (no query) pool gauges.

If any single query fails, log and continue with the others (return the first error at the end via
`anyhow` context) — a transient catalog hiccup shouldn't drop the whole tick. Runner-level errors are
already logged by `log_task_result`.

### Scheduling

In `daemon()`, add:

```rust
let pg_stats = CronTask::new(
    String::from("pg_stats"),
    TimeDelta::minutes(1),
    TimeDelta::seconds(15),      // offset away from the materialization tasks' 30s
    Arc::new(PgStatsTask { lakehouse: lakehouse.clone() }),
)?;
...
runners.spawn(run_tasks_forever(vec![pg_stats], 1, fanout.subscribe()));
```

`lakehouse.clone()` must be taken *before* the existing `every_second`'s `EverySecondTask { lakehouse, views }` move (which currently consumes `lakehouse` by value) — reorder so the clone happens first. Max parallelism 1 (a single collector, no overlap needed).

## Implementation Steps

1. **Create `rust/public/src/servers/pg_stats.rs`**:
   - Imports: `anyhow::{Context, Result}`, `async_trait`, `chrono::{DateTime, Utc}`,
     `micromegas_tracing::prelude::*`, `micromegas_tracing::property_set::{Property, PropertySet}`,
     `micromegas_tracing::intern_string::intern_string`, `LakehouseContext`, `std::sync::Arc`.
   - Tag helpers (`table_tags`, `index_tags`, `state_tags`).
   - `collect_pg_stats(pool)` running the queries and emitting metrics per the tables above.
   - `PgStatsTask` struct + `#[async_trait] impl TaskCallback` (annotate `run` with `#[span_fn]`).
2. **Register module** in `rust/public/src/servers/mod.rs`.
3. **Wire into `daemon()`** in `maintenance.rs`: clone `lakehouse` for `PgStatsTask` before the
   `every_second` move, construct the `pg_stats` `CronTask`, and spawn its runner.
4. **Integration test** in `rust/public/tests/` (see Testing Strategy) driving `collect_pg_stats`
   against a real/ephemeral Postgres.
5. **Docs**: add a "Metadata Postgres self-observability" section with the unused-index sample query
   (see Documentation).
6. **Verify** `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`, and a manual
   run against the local test env confirming rows land in `measures`.

## Files to Modify

- **Create** `rust/public/src/servers/pg_stats.rs` — collector + `PgStatsTask`.
- `rust/public/src/servers/mod.rs` — register module.
- `rust/public/src/servers/maintenance.rs` — wire the new `CronTask` + runner into `daemon()`.
- **Create** `rust/public/tests/pg_stats_test.rs` (or extend an existing maintenance test) — see below.
- Docs: a page under `mkdocs/docs/` (metrics/observability reference) with the sample queries.

## Trade-offs

- **New dedicated task vs. extending `EveryMinuteTask`.** A separate `PgStatsTask` keeps
  materialization and DB-stats collection independent (single responsibility; failures don't
  cross-contaminate; independent cadence/offset). Cost: one more runner loop. Chosen for isolation.
- **Raw cumulative counters vs. computing deltas/rates in the collector.** Emitting raw counters (per
  the issue's counter-reset section) keeps the collector stateless, makes reset detection possible
  downstream, and matches how Postgres exposes the data. Rates are a query-time concern. This mirrors
  the object-cache foyer counters (cumulative, rate computed later). We deliberately do **not** hold
  previous-sample state to pre-compute rates.
- **`intern_string` leak vs. a bounded LRU/cache of tag sets.** Interning leaks one `&'static str`
  per distinct table/index name, but the metadata schema's relation set is fixed and small, so total
  leak is a few dozen short strings for process lifetime. A bounded cache would add complexity for no
  real benefit here. This is the same tradeoff `metric_tags.rs` already accepts.
- **Tagging per relation (higher cardinality) vs. one blob metric.** Per-relation tags are what make
  `WHERE property_get(properties,'index')=…` and `idx_scan = 0` sweeps possible — the whole point of
  the issue. Cardinality is bounded, so this is safe.
- **`GROUP BY state` in SQL vs. counting in Rust.** Let Postgres aggregate; fewer rows over the wire
  and simpler Rust.

## Documentation

- Add/extend an observability reference page under `mkdocs/docs/` documenting the `pg_*` metric
  family (names, units, tags) and, per the acceptance criteria, a **sample unused-index query**:

  ```sql
  -- Indexes with zero scans over the observed window (candidates for removal).
  -- Uses the latest sample per index; segment on the stats_reset boundary for correctness.
  SELECT property_get(properties, 'table')  AS table,
         property_get(properties, 'index')  AS index,
         max(value)                          AS idx_scans
  FROM measures
  WHERE name = 'pg_index_scans'
  GROUP BY 1, 2
  HAVING max(value) = 0
  ORDER BY 1, 2;
  ```

  Include a companion cache-hit-ratio example
  (`pg_db_blocks_hit / (pg_db_blocks_hit + pg_db_blocks_read)` as a delta over a window) and a note
  that `pg_db_stats_reset_timestamp` marks counter-reset boundaries to segment on.
- Note the two explicit follow-ups (out of scope here): `pg_stat_statements` (needs
  `shared_preload_libraries` + `CREATE EXTENSION` on the Aurora cluster parameter group) and
  Aurora/CloudWatch-only signals (`ACUUtilization`, Performance Insights `db.load.avg`; needs AWS SDK
  + IAM).

## Testing Strategy

- **Unit / integration**: drive `collect_pg_stats(&pool)` directly against a Postgres instance (the
  local test env / an ephemeral DB), following the "pure sample function" testability the
  `saturation_monitor` split enables. Assert it returns `Ok(())` and, if a mock/collector dispatch is
  available, that it emits the expected metric names; at minimum assert no error against a schema with
  the standard tables present.
- **`pg_stat_reset` safety**: a code-level assertion/review that the collector issues only `SELECT`s
  (grep the module for `pg_stat_reset`), never a reset call. Emphasize this in the test file comment.
- **End-to-end manual check**: start the local split services
  (`local_test_env/ai_scripts/start_services.py`), let the maintenance daemon tick once, then
  `micromegas-query "SELECT name, count(*) FROM measures WHERE name LIKE 'pg_%' GROUP BY name"` to
  confirm all five families land and the per-relation tags are populated
  (`property_get(properties,'index')`).
- **Standard gates**: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`.

## Open Questions

1. **Offset/cadence**: plan uses period `1min` / offset `15s` (staggered from materialization's 30s).
   Acceptable, or prefer a different cadence?
2. **`last_autovacuum` representation**: plan emits *seconds since* autovacuum (directly usable as
   lag). Prefer raw epoch timestamp instead for consistency with `pg_db_stats_reset_timestamp`?
   (The stats_reset one is emitted as epoch because it's a boundary marker; autovacuum age is emitted
   as age because lag is the signal — but this could be unified either way.)
3. **Scope of relations**: `pg_stat_user_*` covers *all* user tables in the metadata DB. That
   includes the `analytics-web-srv` app_db tables if they share the database. Confirm we want all
   user relations (recommended — bounded and useful) rather than an allowlist.
