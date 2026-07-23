# Maintenance Daemon

`telemetry-maintenance-srv` is the maintenance daemon that keeps the data lake
healthy. It is a long-running service you deploy alongside
[ingestion](ingestion.md) and [FlightSQL](flight-sql.md): it materializes views
on a schedule and runs retention cleanup.

Ad-hoc administration — inspecting partitions, retiring incompatible ones — is
done through SQL and the Python API, not by driving this binary. See
[Admin SQL Functions](functions-reference.md).

It reads the lake from the environment:

| Variable | Required | Description |
|---|---|---|
| `MICROMEGAS_SQL_CONNECTION_STRING` | Yes | PostgreSQL connection for lake metadata |
| `MICROMEGAS_OBJECT_STORE_URI` | Yes | Object store holding the partitions |

## Running the daemon

```bash
# from the rust/ directory
cargo run --release --bin telemetry-maintenance-srv
```

The Docker image (`maintenance.Dockerfile`) runs `telemetry-maintenance-srv` as
its entrypoint; no arguments are required.

| Flag | Default | Description |
|---|---|---|
| `--shutdown-grace-period-seconds` | `25` | Seconds to let in-flight tasks finish on `SIGTERM` |
| `--retention-days` | `90` | Delete lake data older than this many days (retention horizon) |

On `SIGTERM` the daemon stops scheduling new tasks and drains those already
running, up to the grace period. See
[Service Lifecycle & Shutdown](service-lifecycle.md). Interrupted materialization
is safe to redo — a task that doesn't finish simply leaves its partition
unwritten, and the next scheduled run redoes it.

Run a **single** `telemetry-maintenance-srv` instance per lake. The scheduled
tasks are not partitioned across instances, so multiple daemons would
redundantly materialize the same partitions. Materialization is idempotent, so
this is wasteful rather than corrupting — but there is no benefit to more than
one.

## What it does

The daemon keeps materialized views current by running several scheduled tasks. The
four materialization tasks each work a trailing window at their own granularity, so
recent data lands in fine-grained partitions quickly while older data is
consolidated into coarser ones:

| Task | Period | Work |
|---|---|---|
| Every second | 1 s | Materialize the newest 1-second partitions. Skipped when the daemon is more than 10 s behind — the minute task backfills the gap. |
| Every minute | 1 min | Materialize 1-minute partitions. |
| pg_stats | 1 min | Sample the metadata Postgres's `pg_stat_*` views (see [below](#metadata-postgres-self-observability)). |
| Every hour | 1 h | Retention cleanup (see below), then materialize 1-hour partitions. |
| Every day | 1 day | Materialize 1-day partitions. |

### Retention

The hourly task performs cleanup automatically:

- **Deletes lake data older than the retention horizon** — blocks, streams, and
  processes past the horizon are removed.
- **Deletes expired temporary files** left behind by query execution.

The retention horizon defaults to 90 days and is configurable via
`--retention-days` or the `MICROMEGAS_RETENTION_DAYS` environment variable:

```bash
telemetry-maintenance-srv --retention-days 30
# or
export MICROMEGAS_RETENTION_DAYS=30
```

The flag takes precedence over the environment variable, which in turn takes
precedence over the default.

## Metadata Postgres self-observability

Once a minute, the daemon samples the metadata Postgres's standard `pg_stat_*`
catalog views (plus index/table sizes) and emits the readings as micromegas
metrics through its own tracing sink, so they land in the lake's `measures`
view like any other telemetry — no extra wiring or credentials required. This
turns questions that could previously only be answered by connecting to the DB
directly (e.g. *which indexes are dead weight*) into evidence queryable via
FlightSQL.

All counters are emitted **raw and cumulative**, exactly as Postgres reports
them — the collector never calls `pg_stat_reset*` and holds no state between
ticks. Deltas and rates are a query-time concern.

| Metric family | Tags | Source |
|---|---|---|
| `pg_index_scans`, `pg_index_tuples_read`, `pg_index_tuples_fetched`, `pg_index_size_bytes` | `{table, index}` | `pg_stat_user_indexes` + `pg_relation_size` |
| `pg_table_seq_scans`, `pg_table_idx_scans`, `pg_table_live_tuples`, `pg_table_dead_tuples`, `pg_table_tuples_inserted`, `pg_table_tuples_updated`, `pg_table_tuples_deleted`, `pg_table_seconds_since_autovacuum` | `{table}` | `pg_stat_user_tables` |
| `pg_db_blocks_hit`, `pg_db_blocks_read`, `pg_db_xact_commit`, `pg_db_xact_rollback`, `pg_db_deadlocks`, `pg_db_temp_bytes`, `pg_db_stats_reset_timestamp` | — | `pg_stat_database` |
| `pg_activity_connections` | `{state}` | `pg_stat_activity`, grouped |
| `pg_activity_oldest_xact_age_seconds` | — | `pg_stat_activity` |
| `pg_pool_size`, `pg_pool_idle` | — | the daemon's own `sqlx::PgPool` (client-side, no query) |

Tags are read with the `property_get` SQL function, e.g.
`property_get(properties, 'index')`.

`pg_db_stats_reset_timestamp` marks counter-reset boundaries (a clean restart on
PG15+, a crash, or an Aurora failover/patch/instance replacement) — segment on
it rather than assuming counters only ever increase.

### Sample queries

Indexes with zero scans over the observed window (candidates for removal):

```sql
SELECT property_get(properties, 'table')  AS table,
       property_get(properties, 'index')  AS index,
       max(value)                          AS idx_scans
FROM measures
WHERE name = 'pg_index_scans'
GROUP BY 1, 2
HAVING max(value) = 0
ORDER BY 1, 2;
```

Cache-hit ratio over a window (as a delta, since the counters are cumulative):

```sql
WITH bounds AS (
    SELECT min(value) FILTER (WHERE name = 'pg_db_blocks_hit')  AS hit_start,
           max(value) FILTER (WHERE name = 'pg_db_blocks_hit')  AS hit_end,
           min(value) FILTER (WHERE name = 'pg_db_blocks_read') AS read_start,
           max(value) FILTER (WHERE name = 'pg_db_blocks_read') AS read_end
    FROM measures
    WHERE name IN ('pg_db_blocks_hit', 'pg_db_blocks_read')
)
SELECT (hit_end - hit_start)::float
       / nullif((hit_end - hit_start) + (read_end - read_start), 0) AS cache_hit_ratio
FROM bounds;
```

### Out of scope (follow-ups)

- **`pg_stat_statements`** — needs `shared_preload_libraries` and `CREATE
  EXTENSION` on the Aurora cluster parameter group.
- **Aurora/CloudWatch-only signals** (`ACUUtilization`, Performance Insights
  `db.load.avg`) — need the AWS SDK and IAM credentials, unlike the in-DB views
  above which require none.

## Ad-hoc administration

Manual maintenance — backfilling a time range, retiring stale or
schema-incompatible partitions — runs through the FlightSQL server, not this
binary:

- **SQL functions** such as `materialize_partitions()` (backfill a time range),
  `regenerate_partitions()` (force-rebuild existing partitions directly from
  source data, bypassing the freshness check `materialize_partitions()` stops
  at), `retire_partitions()`, and `retire_partition_by_metadata()`.
- **Python helpers** such as `micromegas.admin.list_incompatible_partitions()` and
  `micromegas.admin.retire_incompatible_partitions()`.

Both are documented in [Admin SQL Functions](functions-reference.md).
