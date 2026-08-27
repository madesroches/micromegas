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
| `MICROMEGAS_DEFAULT_AUDIENCE` | No | The deployment's default audience (default: `public`) — what `analytics-web-srv`'s key mint/import routes fall back to ([API Keys](api-keys.md)). The ingestion role now reads it too: a process whose credential carries no audience is stamped with this value explicitly at write time, the same audience the roles that build a lakehouse ([FlightSQL](flight-sql.md), [Maintenance](maintenance.md)) apply where a legacy or replicated row's audience is read. One knob, one meaning: what anything arriving without an audience gets. Read unprefixed — see the monolith's ["one prefix asymmetry"](monolith.md#environment-variables) note. |
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

A process, stream, and block registered under a credential that carries a write audience are
each stamped with their own `audience` **column** — server-written from that
credential, never trusted from the client payload. A block's or a stream's own stamp is the
credential that wrote *that* row, never derived from the `process_id`/`stream_id` it points at.
This is what makes the analytics-side audience filter ([Authentication](authentication.md)) a
real security boundary instead of a client-asserted label.

- **DB-backed ingestion keys** (`ingestion_api_keys`) each carry exactly one immutable write
  audience. Every process, stream, and block a key writes is stamped with that audience.
- **Env-keyring keys** (`MICROMEGAS_API_KEYS`) and **OIDC** credentials carry no bound audience
  of their own. A row registered under one is stamped with the resolved deployment default.
- **No auth provider configured** (`--disable-auth`): stamped with the deployment default too,
  for the same reason.

Every row registered through this HTTP ingestion path is stamped: a credential with no bound
audience resolves to `MICROMEGAS_DEFAULT_AUDIENCE` (default `public`) at the write edge and is
stamped with it explicitly, exactly like any other audience — see
[Authentication → The default audience](authentication.md#audience-stamping-and-the-default).
There is still no startup backfill and no retro-stamping: a **pre-existing** row with a NULL
`audience` column (registered before its ingestion binary reached schema v8) keeps that absence
permanently, and is resolved to the same deployment default on the **read** side instead — so it
is materialized and enforced under that label without anything being written back to it. Admin
`bulk_ingest`/replication now hard-fails on a missing `audience` column rather than ever writing
one with none, so an unstamped row can only be a genuinely pre-v8 one.

!!! warning "Deploy order matters in a split deployment"
    Schema v8 migrations only run from ingestion's (or the monolith's) startup path — FlightSQL
    and maintenance never migrate the database. Upgrade and restart ingestion (or the monolith)
    *before* flight-sql/maintenance: a v8 analytics or maintenance binary reading against a
    pre-v8 database fails every query that touches the new `audience` columns with an "undefined
    column" error, until ingestion has migrated it. A pre-v8 ingestion binary against an
    already-migrated v8 database is fine — writes just leave the column NULL, which reads as the
    deployment default. Also run `regenerate_partitions` over `log_stats` for the retention
    window as part of this rollout, since its `GROUP BY` now includes `audience`.

The reserved `micromegas.*` property namespace is server-written only: any `micromegas.*`
property a client sends is dropped at ingestion and logged (`warn!`), naming the key. In
particular, a native client that used to self-stamp `micromegas.audience` directly no longer has
any effect — there is no property to re-assert any more, and its data gets the deployment default
instead, unless its credential is switched to a DB ingestion key bound to the audience it wants
to keep.

**OTLP `process_id` churn on this upgrade is narrower than "starts stamping."** Because every
audience gets its own OTLP id namespace and the deployment default keeps the pre-existing,
un-salted namespace, traffic that carries no bound audience keeps deriving the *exact same*
`process_id`/`stream_id`/`block_id` across this change — there is no churn for it. The only
population that re-derives is a DB-backed ingestion key **explicitly bound to a label equal to
the deployment default**: it moves out of its own salted namespace into the un-salted one, once,
at upgrade. In practice that is every DB-backed key today, since no deployment sets
`MICROMEGAS_DEFAULT_AUDIENCE` and every existing audience is `public`. This is a tolerated,
one-time consequence, not something to plan a migration around — see
[Authentication → Audience stamping and the default](authentication.md#audience-stamping-and-the-default)
for the full mechanism and the separate, still-relevant churn case (rotating a key to a
*different* audience).

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
