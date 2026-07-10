# Telemetry Ingestion Server

`telemetry-ingestion-srv` is the HTTP service that accepts telemetry from instrumented
processes. It writes event payloads to object storage and records the metadata
(processes, streams, blocks) in PostgreSQL. Every acknowledged write is durable
before the request returns — see [Service Lifecycle & Shutdown](service-lifecycle.md#data-durability).

This is the only service that producers talk to. It does no query or
materialization work; those belong to [FlightSQL](flight-sql.md) and the
[maintenance daemon](maintenance.md).

## Running the binary

```bash
# from the rust/ directory
cargo run --release --bin telemetry-ingestion-srv -- \
  --listen-endpoint-http 0.0.0.0:9000
```

The Docker image (`ingestion.Dockerfile`) exposes port `9000` and runs the same
binary as its entrypoint.

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `MICROMEGAS_SQL_CONNECTION_STRING` | Yes | PostgreSQL connection for lake metadata |
| `MICROMEGAS_OBJECT_STORE_URI` | Yes | Object store for payloads (`file:///path`, `s3://…`, `gs://…`) |
| `MICROMEGAS_API_KEYS` | No | JSON array of API keys (see [Authentication](authentication.md)) |
| `MICROMEGAS_OIDC_CONFIG` | No | OIDC configuration JSON |
| `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS` | No | Drain timeout on `SIGTERM` (default: `25`) |

## CLI flags

| Flag | Default | Description |
|---|---|---|
| `--listen-endpoint-http` | `127.0.0.1:8081` | HTTP bind address |
| `--disable-auth` | off | Disable authentication (development only) |
| `--shutdown-grace-period-seconds` | `25` | Seconds to drain in-flight requests on `SIGTERM` |

!!! warning "Bind address"
    The binary defaults to `127.0.0.1:8081`, which only accepts local
    connections. To accept traffic from other hosts (or from inside a
    container), bind to `0.0.0.0` and the port you intend to publish, e.g.
    `--listen-endpoint-http 0.0.0.0:9000`.

## Authentication

If neither `MICROMEGAS_API_KEYS` nor `MICROMEGAS_OIDC_CONFIG` is set, the server
refuses to start unless `--disable-auth` is passed. This prevents accidentally
running an open ingestion endpoint. For configuration details and provider
precedence, see [Authentication](authentication.md).

```bash
# API keys for machine-to-machine producers
export MICROMEGAS_API_KEYS='[{"name":"game-client","key":"…"}]'
telemetry-ingestion-srv --listen-endpoint-http 0.0.0.0:9000
```

## Health and readiness

The server exposes `GET /health` (unconditional) and `GET /ready` (probes
PostgreSQL and object storage) on the same port as ingestion. Point load-balancer
health checks at `/ready`. See
[Readiness probes](service-lifecycle.md#readiness-probes) for ALB tuning.

## Scaling

Ingestion is stateless — every instance reads and writes the same lake — so it
scales horizontally behind a load balancer. Add instances to raise write
throughput; PostgreSQL and the object store are the shared backends. Writes are
idempotent (blocks are stored at deterministic paths and recorded with
`ON CONFLICT DO NOTHING`), so retried or duplicated requests never double-count.

## Producer configuration

Producers point at the ingestion endpoint with `MICROMEGAS_TELEMETRY_URL`. If the
ingestion service falls behind or becomes briefly unreachable, the Rust telemetry
sink buffers and retries; queue sizes, concurrency, and timeouts are tunable — see
[Telemetry Sink Transport Tuning](telemetry-sink-tuning.md).
