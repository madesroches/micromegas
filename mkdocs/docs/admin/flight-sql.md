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

The gRPC listener binds to `0.0.0.0:50051`. The Docker image
(`flight-sql.Dockerfile`) exposes that port as its entrypoint.

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `MICROMEGAS_SQL_CONNECTION_STRING` | Yes | PostgreSQL connection for lake metadata |
| `MICROMEGAS_OBJECT_STORE_URI` | Yes | Object store holding the partitions |
| `MICROMEGAS_API_KEYS` | No | JSON array of API keys (see [Authentication](authentication.md)) |
| `MICROMEGAS_OIDC_CONFIG` | No | OIDC configuration JSON |
| `MICROMEGAS_ADMINS` | No | JSON array of admin user emails/subjects |
| `MICROMEGAS_STATIC_TABLES_URL` | No | Location of static lookup tables to load at startup |
| `MICROMEGAS_AUDIENCE_GRANTS` | No | JSON object keyed by audience name, granting read/mint access to selectors (`*`/`user:<email>`/`group:<g>`) — see [Audiences and Grants](authentication.md#audiences-and-grants) (`{prefix}_AUDIENCE_GRANTS` falls back to this unprefixed form) |
| `MICROMEGAS_PUBLIC_VIEW_SETS` | No | Comma-separated view-set names `OwnershipRewrite` skips entirely (no audience filtering) — an operator-responsibility allowlist for genuinely aggregated/non-PII view sets only; unset (empty) by default |
| `MICROMEGAS_DEFAULT_AUDIENCE` | No | The audience a credential with no bound ingestion audience is **stamped** with explicitly at write time, and that a legacy row with no stamp is **read** as (default `public`). Set it identically on every role that builds a lakehouse — this one, the [Maintenance](maintenance.md) daemon, the [monolith](monolith.md), and [ingestion](ingestion.md) — since the maintenance role is what bakes the value into partitions and the ingestion role is what stamps new rows with it. Changing it does not relabel already-written partitions; regenerate the six views over any range that should reflect the new value. See [Audience stamping and the default](authentication.md#audience-stamping-and-the-default) |
| `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS` | No | Drain timeout on `SIGTERM` (default: `25`) |
| `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` | No | Query engine memory budget in MB; unset means an unbounded pool (the local-development default). This **is** set in real deployments — each FlightSQL query gets its own `ScopedMemoryPool` wrapper over this shared budget, and its peak usage is reported per query as `peak_memory_bytes` in the [query audit log](../query-guide/query-audit-log.md) |
| `MICROMEGAS_DATAFUSION_MAX_TEMP_DIRECTORY_MB` | No | Cap on total spill-file bytes across all concurrent queries, in MB; default 100 GB (DataFusion's own default), far larger than a typical Fargate container's local disk. Exceeding the cap fails whichever query's spill write pushes past it — not necessarily the query that consumed most of the budget |
| `MICROMEGAS_QUERY_DENY_REFRESH_SECONDS` | No | [Query deny list](functions-reference.md#query-deny-list) snapshot refresh / `last_hit_at` flush interval; default `10`. Also the bound on cross-replica propagation of a newly created or removed rule — the inserting replica applies its own rule immediately, other replicas within one tick |
| `MICROMEGAS_QUERY_DENY_MAX_RULES` | No | [Query deny list](functions-reference.md#query-deny-list) rule cap; default `100`. Bounds the per-query evaluation cost (~3.4 µs at one rule, ~45 µs at the cap) |

## CLI flags

| Flag | Default | Description |
|---|---|---|
| `--disable-auth` | off | Disable authentication (development only) |
| `--health-listen-addr` | none | Address for the HTTP health/readiness sidecar (e.g. `0.0.0.0:8082`) |
| `--shutdown-grace-period-seconds` | `25` | Seconds to drain in-flight RPCs on `SIGTERM` |

!!! note "Listen address is fixed"
    Unlike ingestion, the split `flight-sql-srv` binary always binds
    `0.0.0.0:50051`; there is no listen-address flag. Publish or remap the port
    at the container / load-balancer layer.

## Authentication

If none of `MICROMEGAS_API_KEYS`, `MICROMEGAS_OIDC_CONFIG`, or a non-empty
`analytics_api_keys` DB table is present, the server refuses to start unless
`--disable-auth` is passed. Admin users (via `MICROMEGAS_ADMINS`) gain access to
administrative SQL functions — see [Admin SQL Functions](functions-reference.md).
For provider configuration and precedence, see [Authentication](authentication.md).

flight-sql validates `analytics_api_keys` (see [API Keys](api-keys.md)) but
mints nothing over HTTP itself — it has no key-management routes of its own.
Analytics keys are minted, listed, revoked, and imported through
`analytics-web-srv`'s own HTTP routes instead — see
[API Keys](api-keys.md#minting-an-analytics-key-over-http). This also covers
the key-only deployment (no OIDC) some Grafana setups use — see
[Grafana Authentication](../grafana/authentication.md).

`--disable-auth` treats every FlightSQL caller as admin — it is a development-only
flag, never for production use. API-key (`MICROMEGAS_API_KEYS`) callers are never
admin, so they cannot call the [admin SQL functions](functions-reference.md); admin
access requires an OIDC identity matched against `MICROMEGAS_ADMINS`. `bulk_ingest`
(`CommandStatementIngest`) is likewise admin-gated, via a separate mechanism from
the admin SQL functions — see
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
partitions — so it scales horizontally behind a gRPC-aware load balancer. Queries
are read-only against object storage and PostgreSQL; add instances to serve more
concurrent queries. Heavy or slow-object-store deployments benefit from the
[object cache](object-cache.md), which fronts the object store with a shared
read-through cache.
