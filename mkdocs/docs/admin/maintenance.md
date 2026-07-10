# Maintenance Daemon

`telemetry-admin` runs the maintenance daemon that keeps the data lake healthy. It
is a long-running service you deploy alongside [ingestion](ingestion.md) and
[FlightSQL](flight-sql.md): it materializes views on a schedule and runs retention
cleanup.

Ad-hoc administration — inspecting partitions, retiring incompatible ones,
removing duplicates — is done through SQL and the Python API, not by driving this
binary. See [Admin SQL Functions](functions-reference.md).

It reads the lake from the environment:

| Variable | Required | Description |
|---|---|---|
| `MICROMEGAS_SQL_CONNECTION_STRING` | Yes | PostgreSQL connection for lake metadata |
| `MICROMEGAS_OBJECT_STORE_URI` | Yes | Object store holding the partitions |

## Running the daemon

```bash
# from the rust/ directory
cargo run --release --bin telemetry-admin -- crond
```

The Docker image (`admin.Dockerfile`) runs `telemetry-admin` as its entrypoint;
pass `crond` as the command.

| Flag | Default | Description |
|---|---|---|
| `--shutdown-grace-period-seconds` | `25` | Seconds to let in-flight tasks finish on `SIGTERM` |

On `SIGTERM` the daemon stops scheduling new tasks and drains those already
running, up to the grace period. See
[Service Lifecycle & Shutdown](service-lifecycle.md). Interrupted materialization
is safe to redo — a task that doesn't finish simply leaves its partition
unwritten, and the next scheduled run redoes it.

Run a **single** `crond` instance per lake. The scheduled tasks are not
partitioned across instances, so multiple daemons would redundantly materialize
the same partitions. Materialization is idempotent, so this is wasteful rather
than corrupting — but there is no benefit to more than one.

## What it does

The daemon keeps materialized views current by running four scheduled tasks. Each
task materializes a trailing window at its own granularity, so recent data lands
in fine-grained partitions quickly while older data is consolidated into coarser
ones:

| Task | Period | Work |
|---|---|---|
| Every second | 1 s | Materialize the newest 1-second partitions. Skipped when the daemon is more than 10 s behind — the minute task backfills the gap. |
| Every minute | 1 min | Materialize 1-minute partitions. |
| Every hour | 1 h | Retention cleanup (see below), then materialize 1-hour partitions. |
| Every day | 1 day | Materialize 1-day partitions. |

### Retention

The hourly task performs cleanup automatically:

- **Deletes lake data older than 90 days** — blocks, streams, and processes past
  the retention horizon are removed.
- **Deletes expired temporary files** left behind by query execution.

The 90-day retention is the daemon's built-in policy.

## Ad-hoc administration

Manual maintenance — backfilling a time range, retiring stale or
schema-incompatible partitions, removing duplicate processes/streams/blocks — runs
through the FlightSQL server, not this binary:

- **SQL functions** such as `materialize_partitions()` (backfill a time range),
  `retire_partitions()`, `retire_partition_by_metadata()`, and the
  `delete_duplicate_*()` UDFs.
- **Python helpers** such as `micromegas.admin.list_incompatible_partitions()` and
  `micromegas.admin.retire_incompatible_partitions()`.

Both are documented in [Admin SQL Functions](functions-reference.md).
