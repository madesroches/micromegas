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
| `MICROMEGAS_API_KEYS` | No | JSON array of API keys — legacy/bootstrap path (see [Authentication](authentication.md)) |
| `MICROMEGAS_OIDC_CONFIG` | No | OIDC configuration JSON |
| `MICROMEGAS_ADMINS` | No | JSON array of admin user emails/subjects — used for FlightSQL's admin-gated SQL functions and `analytics-web-srv`'s admin gate; ingestion itself has no admin-gated route of its own (see [API Keys](api-keys.md)) |
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

If none of `MICROMEGAS_API_KEYS`, `MICROMEGAS_OIDC_CONFIG`, or a non-empty
`ingestion_api_keys` DB table is present, the server refuses to start unless
`--disable-auth` is passed. This prevents accidentally running an open
ingestion endpoint. For configuration details and provider precedence, see
[Authentication](authentication.md).

```bash
# API keys for machine-to-machine producers (legacy/bootstrap path)
export MICROMEGAS_API_KEYS='[{"name":"game-client","key":"…"}]'
telemetry-ingestion-srv --listen-endpoint-http 0.0.0.0:9000
```

### Key management

This service always attaches a DB-backed key store (`ingestion_api_keys`) built
from its own data-lake connection for *validating* incoming API keys, but
exposes no HTTP routes to mint, list, revoke, or import them — ingestion has
no key-management HTTP surface of its own. Those operations are handled
exclusively by `analytics-web-srv`'s own `/api/ingestion-api-keys*` routes
instead — see [API Keys](api-keys.md).

## Health and readiness

The server exposes `GET /health` (unconditional) and `GET /ready` (probes
PostgreSQL and object storage) on the same port as ingestion. Point load-balancer
health checks at `/ready`. See
[Readiness probes](service-lifecycle.md#readiness-probes) for ALB tuning.

## Scaling

Ingestion is stateless — every instance reads and writes the same lake — so it
scales horizontally behind a load balancer. Add instances to raise write
throughput; PostgreSQL and the object store are the shared backends. Writes are
idempotent: block payload objects are stored at deterministic paths with a
**create-only** write (first write wins; a colliding write is rejected, not
applied), and the row insert still uses `ON CONFLICT DO NOTHING`, so retried or
duplicated requests never double-count or corrupt a previously stored payload.

The object store backing ingestion must support conditional put
(`PutMode::Create`). AWS S3 supports it with no configuration. An S3-compatible
store explicitly configured with `aws_conditional_put=disabled` will fail every
block write rather than silently falling back to overwrite — see the CHANGELOG
entry for this behavior.

Before depending on a new S3-compatible endpoint, verify it actually enforces
conditional put: write a key, write different bytes to the same key, read it
back, and confirm either an `AlreadyExists` error on the second write, or (if
it succeeded) that the read still returns the *first* write's bytes. If
neither holds, the store does not honor conditional put and the write-once
guarantee does not hold against it.

**Caveat**: a store that *accepts* `If-None-Match: *` but doesn't enforce it
(returns 200 and overwrites regardless) will make `put_if_absent` return
`Created` on every call — no error, no log line — so the write-once invariant
silently degrades to a plain overwrite. There is no code-level way to detect
this; it must be verified operationally with the procedure above before
depending on the store.

## Producer configuration

Producers point at the ingestion endpoint with `MICROMEGAS_TELEMETRY_URL`. If the
ingestion service falls behind or becomes briefly unreachable, the Rust telemetry
sink buffers and retries; queue sizes, concurrency, and timeouts are tunable — see
[Telemetry Sink Transport Tuning](telemetry-sink-tuning.md).
