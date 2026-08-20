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
| `MICROMEGAS_SQL_CONNECTION_STRING` | Yes (lake roles) | PostgreSQL for the data lake. Also read by the `web` role to open its own small pool backing **all three** key/grant-management route groups (`/api/ingestion-api-keys*`, `/api/analytics-api-keys*`, `/api/audience-grants*` — the same pool serves all three tables, see [API Keys](api-keys.md)) — a `--roles web`-only monolith never runs the migrations itself, so the target telemetry DB must already have had ingestion or a lakehouse-role monolith run against it at least once (reaching schema v7, which these routes require since #1372's `ingestion_api_keys.audience` column and #1489's `audience_grants` table), or those routes fail at request time with an opaque `500` |
| `MICROMEGAS_OBJECT_STORE_URI` | Yes (lake roles) | Object store URI (`file:///path` or `s3://…`) |
| `MICROMEGAS_APP_SQL_CONNECTION_STRING` | Yes (web role) | PostgreSQL for the web app |
| `MICROMEGAS_WEB_CORS_ORIGIN` | Yes (web role) | Allowed CORS origin (e.g. `http://localhost:3000`) |
| `MICROMEGAS_BASE_PATH` | Yes (web role) | URL prefix (e.g. `/` or `/micromegas`) |
| `MICROMEGAS_MONOLITH_ROLES` | No | Comma-separated roles or `all` (default: `all`) |
| `MICROMEGAS_PORT` | No | Web server port (default: `3000`) |
| `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS` | No | Drain timeout on `SIGTERM` (default: `25`) |
| `MICROMEGAS_INGESTION_REQUIRE_WRITE_AUDIENCE` | No | Reject ingestion from a credential carrying no write audience (falls back to unprefixed `MICROMEGAS_REQUIRE_WRITE_AUDIENCE`); off by default. Resolved under the **ingestion** role's own prefix — note this is `MICROMEGAS_INGESTION_*`, not `MICROMEGAS_ANALYTICS_*` like the read-side knobs below, since stamping is a write-side (ingestion) concern. See [Ingestion → What gets stamped](ingestion.md#what-gets-stamped) |
| `MICROMEGAS_ANALYTICS_AUDIENCE_GRANTS` | No | JSON object keyed by audience name, granting read/mint access to selectors (`*`/`user:<email>`/`group:<g>`) for FlightSQL callers (falls back to unprefixed `MICROMEGAS_AUDIENCE_GRANTS`) — see [Audiences and Grants](authentication.md#audiences-and-grants) |
| `MICROMEGAS_ANALYTICS_UNSTAMPED_AUDIENCE` | No | Audience name (opaque label, e.g. `public`) a process with no `micromegas.audience` property falls back to for `OwnershipRewrite`'s query-time audience filtering (falls back to unprefixed `MICROMEGAS_UNSTAMPED_AUDIENCE`); unset means such processes stay invisible to every restricted caller. With write-side stamping now live (#1373), "no `micromegas.audience` property" means either never-stamped legacy data or a process ingested under a credential with no bound audience — see [Ingestion → What gets stamped](ingestion.md#what-gets-stamped). **Required to keep legacy data visible when enabling auth on a deployment with unstamped data** (e.g. `public`) — not sufficient by itself, a maintenance-role pass must have materialized the process first; see the CHANGELOG's AbAC Stage 2 upgrade note for the full explanation. **Upgrade note**: a previously-recommended `MICROMEGAS_ANALYTICS_UNSTAMPED_AUDIENCE=group:everyone` now fails startup — `:` is outside the `[A-Za-z0-9_-]` audience charset (#1372) — set an opaque name like `public` instead |
| `MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` | No | Comma-separated view-set names `OwnershipRewrite` skips entirely (no audience filtering; falls back to unprefixed `MICROMEGAS_PUBLIC_VIEW_SETS`) — an operator-responsibility allowlist for genuinely aggregated/non-PII view sets only; unset (empty) by default |
| `MICROMEGAS_DEFAULT_KEY_AUDIENCE` | No | Audience the `web` role's ingestion-key mint/import routes fall back to when a request supplies none (falling back further to `public` for `import` only; `mint` is a **400** if neither this knob nor the request resolves one) — see [What audience does a key carry](api-keys.md#what-audience-does-a-key-carry) |
| `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` | No | Query engine memory budget in MB; unset means an unbounded pool (the local-development default). This **is** set in real deployments — each FlightSQL query gets its own `ScopedMemoryPool` wrapper over this shared budget, and its peak usage is reported per query as `peak_memory_bytes` in the [query audit log](../query-guide/query-audit-log.md). Merge scans (running under the monolith's maintenance role) open one reader per source file group by design (#1491) -- one reader total for the concatenating path, or one per input partition for the ordered sort-merge path -- so merge memory does not scale with host core count |
| `MICROMEGAS_DATAFUSION_MAX_TEMP_DIRECTORY_MB` | No | Cap on total spill-file bytes across all concurrent queries, in MB; default 100 GB (DataFusion's own default), far larger than a typical container's local disk. Exceeding the cap fails whichever query's spill write pushes past it — not necessarily the query that consumed most of the budget |

!!! note "One prefix asymmetry, pre-existing"
    Inside the monolith, `MICROMEGAS_INGESTION_REQUIRE_WRITE_AUDIENCE` resolves under the
    ingestion role's own prefix, while `MICROMEGAS_DEFAULT_KEY_AUDIENCE` (the mint-side default
    above) is always resolved **unprefixed**, even in-process. So one monolith reads
    `MICROMEGAS_INGESTION_REQUIRE_WRITE_AUDIENCE` for stamping enforcement but only
    `MICROMEGAS_DEFAULT_KEY_AUDIENCE` for mint defaults — not
    `MICROMEGAS_INGESTION_DEFAULT_KEY_AUDIENCE`. Pre-existing and out of scope for #1373; noted
    here so it doesn't surprise an operator reaching for a `MICROMEGAS_INGESTION_` prefix on both.

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
the `web` role's own `/api/ingestion-api-keys*` / `/api/analytics-api-keys*` /
`/api/audience-grants*` HTTP routes instead (a separate `analytics-web-srv`
process, or the monolith's own `web` role) — see
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
