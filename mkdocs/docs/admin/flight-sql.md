# FlightSQL Server

`flight-sql-srv` is the Apache Arrow FlightSQL service that answers SQL queries
against the data lake. It runs a DataFusion engine over the partitions written by
[ingestion](ingestion.md) and materialized by the [maintenance daemon](maintenance.md),
and streams results back over gRPC.

Clients — the [Python API](../query-guide/python-api.md), `micromegas-query`, the
[Grafana plugin](../grafana/index.md), and the [analytics web app](web-app.md) —
all connect here.

## Running the binary

```bash
# from the rust/ directory
cargo run --release --bin flight-sql-srv
```

The gRPC listener binds to `0.0.0.0:50051` by default; change it with
`--flightsql-listen-addr`. See [Listen addresses](service-lifecycle.md#listen-addresses). The Docker image
(`flight-sql.Dockerfile`) exposes that port as its entrypoint.

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `MICROMEGAS_SQL_CONNECTION_STRING` | Yes | PostgreSQL connection for lake metadata |
| `MICROMEGAS_OBJECT_STORE_URI` | Yes | Object store holding the partitions; see [Object Storage](object-storage.md) |
| `MICROMEGAS_OIDC_CONFIG` | No | OIDC configuration JSON |
| `MICROMEGAS_STATIC_TABLES_URL` | No | Location of static lookup tables to load at startup; any [Object Storage](object-storage.md) URI form |
| `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` | No | Shared whole-table snapshot cache TTL, in seconds, for the API-key, audience-grant, settings, and local-group (`admins` and any `group:` selector) stores — default `60`. Membership and admin changes take effect within this TTL per process. See [Groups](groups.md) |
| `MICROMEGAS_PUBLIC_VIEW_SETS` | No | Env fallback for the `public_view_sets` [setting](authorization.md#settings) (a stored value wins). Comma-separated view-set names `OwnershipRewrite` skips entirely (no audience filtering) — an operator-responsibility allowlist for genuinely aggregated/non-PII view sets only; unset (empty) by default |
| `MICROMEGAS_DEFAULT_AUDIENCE` | No | The audience a credential with no bound ingestion audience is **stamped** with at write time (default `public`). Set it identically on every role that builds a lakehouse — this one, the [Maintenance](maintenance.md) daemon, the [monolith](monolith.md), and [ingestion](ingestion.md) — since the maintenance role bakes the value into partitions and the ingestion role stamps new rows with it. See [Audience stamping](authorization.md#audience-stamping) |
| `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS` | No | Drain timeout on `SIGTERM` (default: `25`) |
| `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` | No | Query engine memory budget in MB; unset means an unbounded pool (the local-development default). This **is** set in real deployments — each FlightSQL query gets its own `ScopedMemoryPool` wrapper over this shared budget, and its peak usage is reported per query as `peak_memory_bytes` in the [query audit log](../query-guide/query-audit-log.md) |
| `MICROMEGAS_DATAFUSION_MAX_TEMP_DIRECTORY_MB` | No | Cap on total spill-file bytes across all concurrent queries, in MB; default 100 GB (DataFusion's own default), far larger than a typical Fargate container's local disk. Exceeding the cap fails whichever query's spill write pushes past it — not necessarily the query that consumed most of the budget |
| `MICROMEGAS_QUERY_DENY_REFRESH_SECONDS` | No | [Query deny list](functions-reference.md#query-deny-list) snapshot refresh / `last_hit_at` flush interval; default `10`. Also the bound on cross-replica propagation of a newly created or removed rule — the inserting replica applies its own rule immediately, other replicas within one tick |
| `MICROMEGAS_QUERY_DENY_MAX_RULES` | No | [Query deny list](functions-reference.md#query-deny-list) rule cap; default `100`. Bounds the per-query evaluation cost (~3.4 µs at one rule, ~45 µs at the cap) |
| `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` | No | How often this replica reloads [DDL-defined materialized views](materialized-views.md) from Postgres; default `60`. A `CREATE`/`DROP MATERIALIZED VIEW` executed on this replica reloads immediately after commit — this interval only bounds how long a *different* replica lags the change |
| `MICROMEGAS_ADMISSION_BUDGET_GB` | No | Per-replica budget of [query admission](#query-admission), in GB of estimated scan bytes; decimals allowed (e.g. `0.5`). Unset disables admission, and queries are only measured. A value that does not parse fails startup |
| `MICROMEGAS_ADMISSION_MIN_COST_MB` | No | Floor charged per query so a query with no scan cost still occupies a slot; default `8`. Read only when the budget is set |
| `MICROMEGAS_ADMISSION_MAX_PER_IDENTITY_GB` | No | The most scan cost one identity may hold in flight, in GB, decimals allowed; default 80% of the budget. Read only when the budget is set |
| `MICROMEGAS_ADMISSION_OPAQUE_LEAF_COST_MB` | No | Flat cost charged for a plan leaf whose scan cannot be seen at plan time, such as a trace-generating table function; default `64`. Also used to fill `estimated_scan_bytes` when admission is disabled |

`CREATE [OR REPLACE] MATERIALIZED VIEW` / `DROP MATERIALIZED VIEW` are ordinary SQL statements sent
to this server, gated by the same admin check as the [admin SQL
functions](functions-reference.md) — see [Materialized Views](materialized-views.md).

## CLI flags

| Flag | Default | Description |
|---|---|---|
| `--flightsql-listen-addr` | `0.0.0.0:50051` | gRPC bind address (env `MICROMEGAS_FLIGHTSQL_LISTEN_ADDR`) |
| `--disable-auth` | off | Disable authentication (development only) |
| `--health-listen-addr` | none | Address for the HTTP health/readiness sidecar (e.g. `0.0.0.0:8082`; env `MICROMEGAS_FLIGHTSQL_HEALTH_LISTEN_ADDR`) |
| `--shutdown-grace-period-seconds` | `25` | Seconds to drain in-flight RPCs on `SIGTERM` |

## Authentication

If neither `MICROMEGAS_OIDC_CONFIG` nor a non-empty `analytics_api_keys` DB
table is present, the server refuses to start unless `--disable-auth` is
passed. Membership in the reserved `admins` local group
(see [Groups](groups.md)) grants access to administrative SQL functions — see
[Admin SQL Functions](functions-reference.md). For provider configuration and
precedence, see [Authentication](authentication.md).

flight-sql validates `analytics_api_keys` (see [API Keys](api-keys.md)) but
mints nothing over HTTP itself — it has no key-management routes of its own.
Analytics keys are minted, listed, and revoked through
`analytics-web-srv`'s own HTTP routes instead — see
[API Keys](api-keys.md#minting-an-analytics-key-over-http). This also covers
the key-only deployment (no OIDC) some Grafana setups use — see
[Grafana Authentication](../grafana/authentication.md).

`--disable-auth` treats every FlightSQL caller as admin — it is a development-only
flag, never for production use. An `analytics_api_keys` caller normally
carries no email and so can never match a `user:`/`group:` member of `admins`,
meaning it cannot call the [admin SQL functions](functions-reference.md) —
except while `admins` still holds its seeded wildcard (`*`) member, in which
case every authenticated caller, API keys included, is admin (see
[Groups](groups.md)'s upgrade path). `bulk_ingest` (`CommandStatementIngest`)
is likewise admin-gated, via a separate mechanism from the admin SQL functions
— see
[bulk_ingest(table_name, table)](../query-guide/python-api.md#bulk_ingesttable_name-table)
for detail.

## Query deny list

Every replica checks each query against a small, shared set of admin-managed deny rules before
spending any real work on it — see [Query Deny List](functions-reference.md#query-deny-list) for
the SQL functions and [Admin → Query Deny List](web-app.md) for the web screen. Two things worth
knowing operationally:

- **Propagation is polled, not pushed.** Rules live in Postgres; each replica refreshes its own
  in-memory copy every `MICROMEGAS_QUERY_DENY_REFRESH_SECONDS` (default 10s). The replica that
  creates or removes a rule applies it to itself immediately; every other replica picks it up
  within one tick — negligible against an incident measured in minutes.
- **Fail-open, by design.** A refresh that can't reach Postgres keeps the previous snapshot
  (with a `warn!` and a `query_deny_refresh_error_count` metric) rather than denying every query;
  a rule whose expression a given replica can't compile (e.g. after a downgrade) is dropped from
  that replica's snapshot alone (`query_deny_compile_error_count`), never enforced blindly and
  never fatal. This is an availability valve, not an authorization control — those (`ReadScope`,
  audience guards) fail closed and are unaffected.

**Anti-jam escape hatch.** A rule that happens to match every query an admin's own recovery
statement would send can't lock the valve shut: the check is skipped for a statement naming
`deny_queries`/`remove_query_denial`/`list_query_denials`, from a caller who could reach those
functions anyway (an admin, or any authenticated caller on a deployment with no admin principal
at all — see [Admin SQL Functions](functions-reference.md) above).

### Watching for denials

A denial shows up at three different volumes, deliberately: a `warn!` line per denial (visible to
anything already watching warning-level logs), a per-rule rate metric, and the full-detail audit
row. Paste these straight into a dashboard:

```sql
-- every denial in the last hour, one row each
SELECT time, msg
FROM log_entries
WHERE level <= 3          -- Fatal, Error, Warn
  AND msg LIKE 'query denied%'
  AND time >= NOW() - INTERVAL '1 hour'
ORDER BY time DESC;
```

```sql
-- denial rate per rule, per minute
SELECT date_bin(INTERVAL '1 minute', time) AS minute,
       property_get(properties, 'rule_id')  AS rule_id,
       sum(value)                           AS denied
FROM measures
WHERE name = 'query_denied'
  AND time >= NOW() - INTERVAL '6 hours'
GROUP BY minute, rule_id
ORDER BY minute;
```

For the full-detail row (SQL text, fingerprint, complete attribution), see the [query audit
log](../query-guide/query-audit-log.md) — filter on `error_class = "denied"`.

## Query admission

One identity can fill the query engine's memory pool on its own, for example a shared service
account behind dashboards that auto-refresh. Query admission keeps that from starving everyone
else, without ever rejecting a query: each identity gets its own first-in, first-out queue, a
budget caps how much scan work runs at once, and when capacity frees up the next query comes from
the identity with the least work in flight.

- **Cost.** A query's cost is the number of compressed parquet bytes its physical plan will scan,
  estimated from the columns it reads. Footers that are in the metadata cache give an exact
  figure; otherwise the cost is the file size scaled by the share of columns read. Table
  functions whose work is invisible at plan time are charged `MICROMEGAS_ADMISSION_OPAQUE_LEAF_COST_MB`.
  Materialization that happens while the query is planned is not queued.
- **Identity.** The service account when the request delegates for a user, otherwise the user.
  Everyone has the same weight. With `--disable-auth` the identity comes from the client-asserted
  `x-user-id` header, and callers without it share one queue.
- **Cap per identity.** One identity may hold at most `MICROMEGAS_ADMISSION_MAX_PER_IDENTITY_GB`
  in flight, so the rest of the budget stays free and a query from anyone else starts at once. A
  refresh storm from one identity waits in its own queue. Leave `budget - max` large enough for a
  typical query from another caller. A query larger than the cap is charged the cap and still runs.
- **Disabled by default.** Without `MICROMEGAS_ADMISSION_BUDGET_GB` nothing is queued, but the
  estimate is still written to the [query audit log](../query-guide/query-audit-log.md).

The budget and the fairness apply per replica, so size the budget against each replica's own
`MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` pool. The budget is in scan bytes, not memory bytes: run
the estimate-versus-actual query in the audit log guide for a few days, compare
`estimated_scan_bytes` with `peak_memory_bytes`, and pick a budget whose queries together fit in
the pool. Wait time shows up as `queue_wait_ms`, and the `admission_queue_wait`,
`admission_queued`, `admission_in_flight_cost` and `admission_queued_queries` metrics are emitted
without per-identity tags.

A query that waits longer than the temporary-file expiry of about an hour may fail with an
object-not-found error if its partitions were replaced while it waited; a wait that long means the
service is badly overloaded.

## DataFusion session config

Every DataFusion session starts from DataFusion's defaults, so options such as
`datafusion.execution.target_partitions` follow the number of cores the host exposes. On a
container with a CPU quota smaller than that, set the `datafusion_config_query` [setting](authorization.md#settings)
(Admin -> Settings) to a JSON object of overrides:

```json
{"datafusion.execution.target_partitions": 2}
```

Values may be strings, numbers or booleans. Keys you do not list keep DataFusion's defaults, and an
unset setting leaves the service on pure defaults. The query and maintenance services each have their
own key (`datafusion_config_query` and `datafusion_config_maintenance`). A change is picked up within
one settings cache TTL (`MICROMEGAS_AUTH_CACHE_TTL_SECONDS`, 60 s by default) with no restart.

* `datafusion.execution.parquet.enable_page_index`, `datafusion.sql_parser.collect_spans` and
  `datafusion.catalog.information_schema` are fixed on every session and are rejected if listed.
* `datafusion.runtime.*` keys are not accepted; the memory budget stays
  `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB`.
* Merges keep their own forced optimizer settings (`repartition_file_scans`, `enable_round_robin_repartition`,
  `repartition_aggregations`, `repartition_joins`, `prefer_existing_sort`); overrides of those apply to
  ordinary queries only.
* The service logs `datafusion config (query): <key>=<value>, ...` the first time it builds a session and
  whenever the value changes (`maintenance` for the daemon).
* A FlightSQL server built with `with_isolation_config` pins its settings source, which also turns off
  the DB-backed overrides.

Candidate keys:

| Key | Effect |
|---|---|
| `datafusion.execution.target_partitions` | Parallelism of each plan. Defaults to the visible core count. |
| `datafusion.execution.planning_concurrency` | Concurrency used while planning. Defaults to the visible core count. |
| `datafusion.execution.batch_size` | Rows per record batch. |
| `datafusion.optimizer.repartition_file_min_size` | Minimum file size, in bytes, before a scan is split across partitions. |

On a quota'd container, pair `datafusion.execution.target_partitions` with `TOKIO_WORKER_THREADS` so
the async runtime does not over-fan either.

## Health and readiness

The gRPC server does not itself serve HTTP. Pass `--health-listen-addr` to start
a lightweight sidecar that serves `GET /health` (unconditional) and `GET /ready`
(probes PostgreSQL and object storage):

```bash
flight-sql-srv --health-listen-addr 0.0.0.0:8082
```

Omit the flag and no sidecar starts. See
[FlightSQL health sidecar](service-lifecycle.md#flightsql-health-sidecar) for
details.

## Scaling

FlightSQL is stateless with respect to the lake — every instance reads the same
partitions — so it scales horizontally behind a gRPC-aware load balancer. JIT
partition materialization writes to both object storage (the new partition's
Parquet file) and PostgreSQL (its metadata row); add instances to serve more
concurrent queries. Heavy or slow-object-store deployments benefit from the
[object cache](object-cache.md), which fronts the object store with a shared
read-through cache. See
[Operating in a High-Availability Environment](high-availability.md) for how
FlightSQL behaves during a database failover.
