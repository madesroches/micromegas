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
| `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS` | No | Drain timeout on `SIGTERM` (default: `25`) |

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

If neither `MICROMEGAS_API_KEYS` nor `MICROMEGAS_OIDC_CONFIG` is set, the server
refuses to start unless `--disable-auth` is passed. Admin users (via
`MICROMEGAS_ADMINS`) gain access to administrative SQL functions — see
[Admin SQL Functions](functions-reference.md). For provider configuration and
precedence, see [Authentication](authentication.md).

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
