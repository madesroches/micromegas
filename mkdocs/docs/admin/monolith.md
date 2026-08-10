# Monolith Deployment

The `micromegas-monolith` binary runs all four roles — ingestion, FlightSQL, maintenance, and web app — in a single process. It shares one Tokio runtime, one data-lake connection, and one LakehouseContext across all roles, and shuts everything down cleanly on `SIGTERM`.

This deployment mode targets workstations, laptops, CI, and any single-machine setup where you want observability without running four separate services.

## Quick start with Docker Compose

!!! note "Compose version"
    Requires Docker Compose v2.23.1+ (for the compose file's inline `configs.content` DB-init block).

```bash
# from the docker/ directory
docker compose -f docker-compose.monolith.yaml up
```

The compose file starts PostgreSQL and the monolith. The web app is at `http://localhost:3000`, the ingestion endpoint at `http://localhost:9000`, and FlightSQL at `localhost:50051`.

## Quick start with the local start script

```bash
python3 local_test_env/ai_scripts/start_services.py --monolith
```

Builds `micromegas-monolith` from source and starts it together with PostgreSQL. Logs are written to `/tmp/monolith.log`.

## Running the binary directly

```bash
# from the rust/ directory
cargo run --bin micromegas-monolith -- \
  --roles all \
  --listen-endpoint-http 127.0.0.1:9000 \
  --frontend-dir ../analytics-web-app/dist \
  --disable-auth
```

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `MICROMEGAS_SQL_CONNECTION_STRING` | Yes (lake roles) | PostgreSQL for the data lake. Also read by the `web` role to open its own small pool backing **both** key-management route groups (`/api/ingestion-api-keys*`, `/api/analytics-api-keys*` — the same pool serves both tables, see [API Keys](api-keys.md)) — a `--roles web`-only monolith never runs the v5 migration itself, so the target telemetry DB must already have had ingestion or a lakehouse-role monolith run against it at least once, or those routes fail at request time with an opaque `500` |
| `MICROMEGAS_OBJECT_STORE_URI` | Yes (lake roles) | Object store URI (`file:///path` or `s3://…`) |
| `MICROMEGAS_APP_SQL_CONNECTION_STRING` | Yes (web role) | PostgreSQL for the web app |
| `MICROMEGAS_WEB_CORS_ORIGIN` | Yes (web role) | Allowed CORS origin (e.g. `http://localhost:3000`) |
| `MICROMEGAS_BASE_PATH` | Yes (web role) | URL prefix (e.g. `/` or `/micromegas`) |
| `MICROMEGAS_MONOLITH_ROLES` | No | Comma-separated roles or `all` (default: `all`) |
| `MICROMEGAS_PORT` | No | Web server port (default: `3000`) |
| `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS` | No | Drain timeout on `SIGTERM` (default: `25`) |
| `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` | No | Query engine memory budget in MB; unset means an unbounded pool (the local-development default). This **is** set in real deployments — each FlightSQL query gets its own `ScopedMemoryPool` wrapper over this shared budget, and its peak usage is reported per query as `peak_memory_bytes` in the [query audit log](../query-guide/query-audit-log.md) |
| `MICROMEGAS_DATAFUSION_MAX_TEMP_DIRECTORY_MB` | No | Cap on total spill-file bytes across all concurrent queries, in MB; default 100 GB (DataFusion's own default), far larger than a typical container's local disk. Exceeding the cap fails whichever query's spill write pushes past it — not necessarily the query that consumed most of the budget |

## CLI flags

| Flag | Default | Description |
|---|---|---|
| `--roles` | `all` | Enable specific roles: `ingestion`, `flightsql`, `maintenance`, `web`, or `all` |
| `--listen-endpoint-http` | `127.0.0.1:8081` | Ingestion HTTP bind address |
| `--port` | `3000` | Web server port |
| `--frontend-dir` | `/app/frontend` | Path to the built analytics web app |
| `--disable-auth` | off | Disable authentication for all roles |
| `--disable-ingestion-auth` | off | Disable auth for ingestion only (useful with OIDC on web) |
| `--no-seed-data-source` | off | Skip auto-seeding the local FlightSQL data source |
| `--shutdown-grace-period-seconds` | `25` | Seconds to drain before hard exit on `SIGTERM` |

## Authentication

The monolith supports **per-role auth**. Ingestion (machine-to-machine) and analytics (FlightSQL + web) can be configured independently.

### No auth (development)

```bash
micromegas-monolith --disable-auth
```

### API keys for ingestion only, OIDC for analytics

```bash
export MICROMEGAS_INGESTION_API_KEYS='[{"name":"service-a","key":"key1"},{"name":"service-b","key":"key2"}]'
export MICROMEGAS_ANALYTICS_OIDC_CONFIG='{"issuers":[{"issuer":"https://your-idp.example.com","audience":"your-client-id"}]}'
```

The prefix fallback means `MICROMEGAS_API_KEYS` works for ingestion when `MICROMEGAS_INGESTION_API_KEYS` is not set, and `MICROMEGAS_OIDC_CONFIG` works for analytics when `MICROMEGAS_ANALYTICS_OIDC_CONFIG` is not set.

### Full OIDC (web + analytics, open ingestion)

```bash
export MICROMEGAS_OIDC_CONFIG='{"issuers":[{"issuer":"https://your-idp.example.com","audience":"your-client-id"}]}'
export MICROMEGAS_STATE_SECRET="<random-secret>"
export MICROMEGAS_AUTH_REDIRECT_URI="http://localhost:3000/auth/callback"
micromegas-monolith --disable-ingestion-auth
```

Admin users are controlled by `MICROMEGAS_ANALYTICS_ADMINS` (falls back to `MICROMEGAS_ADMINS`).
The ingestion role has no admin-gated route of its own — `MICROMEGAS_INGESTION_ADMINS`
no longer gates anything on this role (see [API Keys](api-keys.md)).

### Key management

The ingestion role always attaches a DB-backed key store (`ingestion_api_keys`)
built from the shared lake connection, for *validating* incoming API keys —
but exposes no HTTP routes of its own to mint, list, revoke, or import them.
FlightSQL validates `analytics_api_keys` the same way. Minting, listing,
revoking, and importing keys for **both** tables happens exclusively through
the `web` role's own `/api/ingestion-api-keys*` / `/api/analytics-api-keys*`
HTTP routes instead (a separate `analytics-web-srv` process, or the
monolith's own `web` role) — see
[API Keys](api-keys.md#minting-an-analytics-key-over-http).

## Role selection

Run only a subset of roles with `--roles` or `MICROMEGAS_MONOLITH_ROLES`:

```bash
# Ingestion + maintenance only (no web app, no FlightSQL)
micromegas-monolith --roles ingestion,maintenance

# Web + FlightSQL only (point at an existing data lake)
micromegas-monolith --roles web,flightsql
```

Valid role names: `ingestion`, `flightsql`, `maintenance`, `web`.

## Compared to the split deployment

| | Monolith | Split services |
|---|---|---|
| Processes | 1 | 4 |
| Memory | Lower (shared lake + cache) | Higher (duplicated per role) |
| CPU scheduling | Adaptive (work-stealing across roles) | Fixed partition per service |
| Role isolation | None — shared fate | Hard — separate processes |
| HA / scale-out | No | Yes |
| Setup complexity | Low | Higher |

The monolith is the dev / personal / single-machine rung. The split deployment is the production / HA rung; both remain fully supported.

## Self-telemetry

When started with `MICROMEGAS_TELEMETRY_URL` pointing at itself, the monolith ingests its own traces and logs. The docker-compose file does this by default.

```yaml
MICROMEGAS_TELEMETRY_URL: "http://micromegas:9000"
MICROMEGAS_FLUSH_PERIOD: "5"
```
