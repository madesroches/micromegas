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
| `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` | No | Audience stamped onto a process whose credential carries none (default: `public`). Read unprefixed, unlike most ingestion-role knobs elsewhere — see the monolith's ["one prefix asymmetry"](monolith.md#environment-variables) note. Not to be confused with `MICROMEGAS_DEFAULT_KEY_AUDIENCE` (`analytics-web-srv`, [API Keys](api-keys.md)): that one is what audience a *newly minted key* gets; this one is what audience *data written without one* gets. |
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

## What gets stamped

Every process ingestion registers is stamped with a `micromegas.audience` property,
server-written from the authenticated credential — never trusted from the client payload. This
is what makes the analytics-side audience filter ([Authentication](authentication.md)) a real
security boundary instead of a client-asserted label. Every process gets one, unconditionally —
there is no unstamped state (#1482).

- **DB-backed ingestion keys** (`ingestion_api_keys`) each carry exactly one immutable write
  audience. Every process a key registers is stamped with that audience.
- **Env-keyring keys** (`MICROMEGAS_API_KEYS`) and **OIDC** credentials carry no bound audience
  of their own. Data ingested under them is stamped with `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE`
  (default `public`) instead.
- **No auth provider configured** (`--disable-auth`): stamped with the same default, for the
  same reason.

An idempotent backfill runs at every ingestion-service startup, appending
`MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` onto any `processes` row from before this change that has
no `micromegas.audience` property at all — safe to re-run, and what repairs a row an old replica
wrote mid-rollout during an upgrade. Set the var *before* upgrading if `public` is not the label
legacy data should carry.

**Rolling-upgrade note**: a row an old replica writes after the *last* new replica has started is
repaired only at that replica's next restart. Until then, an insert-hour containing such a row
fails its `blocks` materialization (fail-closed, retried by the maintenance daemon every tick,
visible in its logs) — restart one ingestion replica once the rollout completes if you see this.

The reserved `micromegas.*` property namespace is server-written only: any `micromegas.*`
property a client sends is dropped at ingestion and logged (`warn!`), naming the key. In
particular, a native client that used to self-stamp `micromegas.audience` directly no longer has
any effect — its data gets the deployment default instead, unless its credential is switched to a
DB ingestion key bound to the audience it wants to keep. A deployment switching from no bound
audiences to keyed ingestion (or simply adopting a non-`public` default) sees its OTLP-derived
`process_id`s churn once, the same shape as switching ingestion keys to a new audience — see
[Authentication → Write-Side Stamping](authentication.md#write-side-stamping-abac-stage-5-extended-by-1482).

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
[Telemetry Sink Configuration](telemetry-sink-tuning.md).
