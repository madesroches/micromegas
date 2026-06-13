# Monolith Deployment

The `micromegas-monolith` binary runs all four roles — ingestion, FlightSQL, maintenance, and web app — in a single process. It shares one Tokio runtime, one data-lake connection, and one LakehouseContext across all roles, and shuts everything down cleanly on `SIGTERM`.

This deployment mode targets workstations, laptops, CI, and any single-machine setup where you want observability without running four separate services.

## Quick start with Docker Compose

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
| `MICROMEGAS_SQL_CONNECTION_STRING` | Yes (lake roles) | PostgreSQL for the data lake |
| `MICROMEGAS_OBJECT_STORE_URI` | Yes (lake roles) | Object store URI (`file:///path` or `s3://…`) |
| `MICROMEGAS_APP_SQL_CONNECTION_STRING` | Yes (web role) | PostgreSQL for the web app |
| `MICROMEGAS_WEB_CORS_ORIGIN` | Yes (web role) | Allowed CORS origin (e.g. `http://localhost:3000`) |
| `MICROMEGAS_BASE_PATH` | Yes (web role) | URL prefix (e.g. `/` or `/micromegas`) |
| `MICROMEGAS_MONOLITH_ROLES` | No | Comma-separated roles or `all` (default: `all`) |
| `MICROMEGAS_PORT` | No | Web server port (default: `3000`) |
| `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS` | No | Drain timeout on `SIGTERM` (default: `25`) |

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
export MICROMEGAS_INGESTION_API_KEYS='["key1","key2"]'
export MICROMEGAS_ANALYTICS_OIDC_CONFIG='{"issuer_url":"...","client_id":"...","audience":"..."}'
```

The prefix fallback means `MICROMEGAS_API_KEYS` works for ingestion when `MICROMEGAS_INGESTION_API_KEYS` is not set, and `MICROMEGAS_OIDC_CONFIG` works for analytics when `MICROMEGAS_ANALYTICS_OIDC_CONFIG` is not set.

### Full OIDC (web + analytics, open ingestion)

```bash
export MICROMEGAS_OIDC_CONFIG='{"issuer_url":"...","client_id":"...","audience":"..."}'
export MICROMEGAS_STATE_SECRET="<random-secret>"
export MICROMEGAS_AUTH_REDIRECT_URI="http://localhost:3000/api/auth/callback"
micromegas-monolith --disable-ingestion-auth
```

Admin users are controlled by `MICROMEGAS_ANALYTICS_ADMINS` (falls back to `MICROMEGAS_ADMINS`).

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
