# Issue #1139 — Single-process (monolith) deployment for low-cost setups

## Overview

Provide a way to run Micromegas as **one process** that hosts the roles currently split
across four binaries — `telemetry-ingestion-srv`, `flight-sql-srv`, `telemetry-admin crond`,
and `analytics-web-srv` — so a small/demo/cost-sensitive deployment can run the whole stack
on a single small instance instead of four. The split deployment must remain fully supported.

The work is **mostly a refactor-to-extract-then-compose**, not new functionality. Each role's
serve logic is lifted out of its binary `main` into a reusable function in a library crate, and
a new `micromegas-monolith` binary wires them onto one tokio runtime, one data-lake connection,
one auth provider, and one SIGTERM fanout. The existing per-service binaries become thin
wrappers around the same extracted functions (DRY: split and monolith deployments share one set
of building blocks; open/closed: no behavioral change to the standalone services).

## Current State

Four independent binaries, each with its own `#[micromegas_main]`, its own `#[global_allocator]`,
its own SIGTERM handling, and its own resource setup:

| Role | Binary | Entry point | Serve logic location | Reusable today? |
|---|---|---|---|---|
| Ingestion (HTTP) | `telemetry-ingestion-srv` | `telemetry-ingestion-srv/src/main.rs:122` | `serve_http()` **inside the binary** (`:60`) | ❌ not callable from another crate |
| FlightSQL (gRPC) | `flight-sql-srv` | `flight-sql-srv/src/flight_sql_srv.rs:27` | `FlightSqlServer::builder().build_and_serve()` (`public/src/servers/flight_sql_server.rs:148`) | ⚠️ partially — owns its own `LakehouseContext::from_env` and `wait_for_sigterm` |
| Maintenance daemon | `telemetry-admin crond` | `telemetry-admin-cli/src/telemetry_admin.rs:136` | `servers::maintenance::daemon(...)` (`public/src/servers/maintenance.rs:291`) | ✅ fully — takes `lakehouse`, `views`, `shutdown`, `grace` |
| Analytics web app | `analytics-web-srv` | `analytics-web-srv/src/main.rs:373` | router build + serve **inside the binary `main`** | ❌ binary has a lib (`src/lib.rs`) but `main` is not extracted |

### Shared building blocks that already exist

- **Data lake connection.** Two near-identical helpers produce a `DataLakeConnection`
  (`pool` + `blob_storage`, `ingestion/src/data_lake_connection.rs:9`):
  - `connect_to_data_lake` (`data_lake_connection.rs:24`) — connect only.
  - `connect_to_remote_data_lake` (`remote_data_lake.rs:43`) — connect **and run `migrate_db`**
    (the ingestion/telemetry schema migration).
  - `LakehouseContext::from_env` (`analytics/src/lakehouse/lakehouse_context.rs:39`) uses
    `connect_to_data_lake`, then runs `migrate_lakehouse`. It bundles the lake + metadata cache +
    file cache + DataFusion runtime (`:27`), and is the object every query/maintenance path needs.
  - **Implication for the monolith:** a single shared connection must run **both** `migrate_db`
    (ingestion) and `migrate_lakehouse` (lakehouse). Today no single code path does both.
- **Auth provider.** Ingestion (`main.rs:133`) and FlightSQL (`flight_sql_server.rs:189`) both call
  `micromegas::auth::default_provider::provider()`, which reads a **single fixed set of env var
  names**: `MICROMEGAS_API_KEYS` (`default_provider.rs:41`), `MICROMEGAS_OIDC_CONFIG`
  (`oidc.rs:151`), `MICROMEGAS_ADMINS` (`oidc.rs:260`). The web app uses a **separate** cookie/OIDC
  `AuthState` (`analytics-web-srv/src/main.rs:79`).
  - **Implication for the monolith — ingestion and analytics need *different* auth.** As separate
    processes they already can: each container has its own environment, so an operator gives
    ingestion API-key auth (machine-to-machine: game servers, Unreal clients) and gives FlightSQL
    OIDC auth (human/tooling queries) by setting different values for the same var names. A monolith
    runs both in **one shared environment**, so calling `provider()` once yields one config for
    both — collapsing a distinction operators rely on. The monolith must therefore build **two
    independent providers** from **role-scoped** configuration.
- **Graceful shutdown.** `servers::shutdown` (`public/src/servers/shutdown.rs`) provides
  `wait_for_sigterm()`, `ShutdownFanout` (one signal → N subscribers via a `watch` channel), and
  `serve_axum_with_graceful_shutdown(...)`. The FlightSQL builder and the maintenance daemon already
  build their own `ShutdownFanout` internally from a `shutdown: impl Future` parameter. This is the
  exact primitive the monolith needs to drive all roles from one signal.
- **`#[micromegas_main]`** (`micromegas-proc-macros/src/lib.rs:49`) sets up the telemetry guard
  *before* building a multi-thread tokio runtime, and installs a SIGINT (Ctrl-C) flush handler. The
  monolith uses this **once**.

### Web app ↔ query engine coupling

The web app does **not** embed the query engine. `stream_query` builds a **FlightSQL client** to a
per-data-source `url` (`analytics-web-srv/src/stream_query.rs:231-249`, via
`BearerFlightSQLClientFactory`), forwarding the user's bearer token. Data sources are stored in the
web app's own Postgres DB (`MICROMEGAS_APP_SQL_CONNECTION_STRING`) and managed in the UI.

**Implication:** in the monolith the web app keeps talking to FlightSQL over the network — just
pointed at the in-process loopback listener (`http://127.0.0.1:50051`). No change to the query path.

### Two databases

- `MICROMEGAS_SQL_CONNECTION_STRING` — telemetry **data lake** DB (ingestion, FlightSQL, daemon).
- `MICROMEGAS_APP_SQL_CONNECTION_STRING` — web app DB (screens, data sources;
  `analytics-web-srv/src/main.rs:380`).

Both can live on the same Postgres server (different database names) for a low-cost deployment — no
code change needed, just configuration.

### Three listeners

Ingestion HTTP (`--listen-endpoint-http`, default `127.0.0.1:8081`), FlightSQL gRPC
(`0.0.0.0:50051`), web HTTP (`--port`, default `3000`). The monolith keeps all three distinct
listeners on the one process (single-port fronting is out of scope — see Trade-offs).

## Design

### New crate: `rust/monolith/`

A binary crate `micromegas-monolith` (bin name `micromegas-monolith`) depending on `micromegas`
(`server` feature) and `analytics-web-srv` (as a library). It owns:

- the single `#[global_allocator]` (jemalloc, non-Windows) and single `#[micromegas_main]`,
- CLI / env parsing including **role selection**,
- one-time construction of shared resources,
- composition of the selected roles' serve futures onto one runtime with one shutdown fanout.

```text
                       micromegas-monolith (1 process, 1 tokio runtime)
  ┌───────────────────────────────────────────────────────────────────────────┐
  │  shared: DataLakeConnection (migrate_db + migrate_lakehouse)                 │
  │          LakehouseContext (caches + DataFusion runtime)                      │
  │          AuthProvider (API-key/OIDC)            ShutdownFanout(SIGTERM)       │
  │                                                                              │
  │   ┌──────────┐   ┌───────────┐   ┌──────────────┐   ┌────────────────────┐  │
  │   │ingestion │   │ flightsql │   │  maintenance │   │  web app (own DB,   │  │
  │   │ :8081 H  │   │ :50051 g  │◀──│   daemon     │   │  :3000) ── FlightSQL │  │
  │   └──────────┘   └───────────┘   └──────────────┘   │  client → loopback  │  │
  │        ▲              ▲                ▲             └────────────────────┘  │
  │        └── each role gets fanout.subscribe(); global deadline arm           │
  └───────────────────────────────────────────────────────────────────────────┘
```

### Refactor existing serve logic into reusable functions

Goal: each role is callable as `async fn run(... , shutdown: impl Future, grace: Duration)`,
accepting **injected** shared resources rather than building its own from env.

1. **Ingestion** — extract `serve_http` out of `telemetry-ingestion-srv/src/main.rs` into the
   library, e.g. `public::servers::ingestion::serve_ingestion(lake, auth_provider, listen_addr,
   shutdown, grace)`. The binary `main` becomes: parse CLI → build lake + auth from env → call it.
   (`register_routes` already lives in `servers::ingestion`; this just moves the listener/middleware
   assembly next to it.)

2. **FlightSQL** — extend `FlightSqlServerBuilder` (`public/src/servers/flight_sql_server.rs`) so the
   monolith can inject shared resources instead of `from_env`:
   - `with_lakehouse(Arc<LakehouseContext>)` — skip the internal `LakehouseContext::from_env`.
   - `with_shutdown(impl Future<Output=()> + Send + 'static)` — use this instead of the internal
     `wait_for_sigterm()` so all roles share one signal.
   - The standalone binary keeps calling `from_env` + `wait_for_sigterm` defaults (unchanged
     behavior). These are additive builder methods (open/closed).

3. **Maintenance daemon** — already reusable. The monolith calls
   `servers::maintenance::daemon(lakehouse, views_to_update, shutdown, grace)` directly, building
   `views_to_update` via `get_global_views_with_update_group(&view_factory)` exactly as
   `telemetry_admin.rs:139` does.

4. **Web app** — extract the body of `analytics-web-srv/src/main.rs:373` into a library function in
   `analytics-web-srv/src/lib.rs`, e.g. `run_web_server(config: WebServerConfig, shutdown, grace)`,
   where `WebServerConfig` carries port, frontend_dir, base_path, cors_origin, app DB string, maps
   config, disable_auth. The binary `main` builds the config from CLI/env and calls it. (The crate
   already exposes a lib; the `data_sources`/`screens` modules currently declared in the *binary*
   move into the lib so `run_web_server` can use them.)

### Role selection

A single CLI flag, defaulting to all roles, mirrored by an env var for container deployments:

```text
--roles ingestion,flightsql,maintenance,web     (default: all)
MICROMEGAS_MONOLITH_ROLES=ingestion,flightsql,maintenance,web
```

Parsed into a `Roles` set. `all` is a recognized alias. The flag takes precedence over the env var
over the default — same precedence convention already documented for the grace period
(`service-lifecycle.md:36`). This lets one binary scale down to, say, `--roles web` while still
sharing the same code path as the full monolith.

### Per-role auth: ingestion and analytics configured independently

The monolith must let ingestion and FlightSQL carry **different** auth configurations even though
they share one process environment. Parameterize the provider builder by an env-var **prefix**,
with fallback to the existing unprefixed names so standalone deployments and simple monolith setups
are unchanged:

```text
ingestion auth  ← MICROMEGAS_INGESTION_API_KEYS / MICROMEGAS_INGESTION_OIDC_CONFIG / ..._ADMINS
analytics auth  ← MICROMEGAS_ANALYTICS_API_KEYS / MICROMEGAS_ANALYTICS_OIDC_CONFIG / ..._ADMINS
fallback (both) ← MICROMEGAS_API_KEYS          / MICROMEGAS_OIDC_CONFIG           / MICROMEGAS_ADMINS
```

Auth-crate changes (additive, open/closed — standalone binaries keep using the unprefixed path):
- `default_provider::provider_with_prefix(prefix: &str)` — looks up `{prefix}_API_KEYS` /
  `{prefix}_OIDC_CONFIG`, each falling back to the unprefixed name when its prefixed form is unset;
  returns `Ok(None)` when neither prefixed nor fallback config exists (auth disabled for that role).
- `OidcConfig::from_env_var(name: &str)` — generalize today's `from_env()` (which hardcodes
  `MICROMEGAS_OIDC_CONFIG`, `oidc.rs:151`) so the prefix path can target a different var.
- `parse_key_ring` already takes the JSON string, so only the var lookup needs prefixing.
- `MICROMEGAS_ADMINS` (OIDC admin list, `oidc.rs:260`) gets the same prefixed-with-fallback
  treatment so analytics can scope admins separately from ingestion.

`default_provider::provider()` becomes `provider_with_prefix` with an empty/unprefixed lookup, so the
existing callers and behavior are untouched. The two providers are injected via the serve functions'
existing seams: ingestion's `serve_ingestion(..., auth_provider)` and the FlightSQL builder's
`with_auth_provider(...)`. The web app's cookie/OIDC `AuthState` remains its own separate config —
out of scope for this unification.

### Shared resource construction (monolith `main`)

```text
1. parse CLI + env (roles, listen addrs, grace, disable_auth, web config)
2. if any of {ingestion, flightsql, maintenance}:
     conn = connect_to_remote_data_lake(SQL_CONN, OBJECT_STORE_URI)   // runs migrate_db
     lakehouse = LakehouseContext::new(conn, make_runtime_env())      // + migrate_lakehouse
     // single connection shared by all three lake-backed roles
3. if not disable_auth:
     if ingestion enabled: ingestion_auth = provider_with_prefix("MICROMEGAS_INGESTION")?
     if flightsql enabled: analytics_auth = provider_with_prefix("MICROMEGAS_ANALYTICS")?
     // two independent providers; each fail-fast only if that role requires auth but none resolved
4. fanout = ShutdownFanout::new(wait_for_sigterm())
5. spawn selected role futures, injecting each role's own auth + fanout.subscribe() + grace
6. join all; first hard error aborts; global deadline arm bounds the drain
```

A `--disable-auth` decision may also need to be **per role** (e.g. open ingestion on a trusted
network while still requiring OIDC for analytics). Recommend a global `--disable-auth` plus optional
per-role overrides (`--disable-ingestion-auth`, `--disable-analytics-auth`); see Open Questions.

A small refactor of `LakehouseContext` may be needed so it can be built from an
**already-connected** `DataLakeConnection` that has had **both** migrations applied (today
`from_env` connects fresh and only runs `migrate_lakehouse`). Plan: add
`LakehouseContext::from_connection(Arc<DataLakeConnection>) -> Result<Arc<Self>>` (runs
`migrate_lakehouse`, builds caches + runtime), and have `from_env` call it after
`connect_to_remote_data_lake` so ingestion's `migrate_db` also runs there. Verify migration
idempotency (both are additive/`IF NOT EXISTS`-style today) so running both against one DB is safe.

### Composition & failure semantics

Use a `tokio::task::JoinSet` (or `try_join!`) over the selected role futures. Each role future
already returns `Result<()>` and already self-bounds its drain against `grace` internally (axum
helper, FlightSQL builder, daemon). The monolith adds:

- **Fail-fast on startup:** if any role's listener bind fails, abort the whole process with that
  error (don't run a half-up monolith).
- **Coordinated shutdown:** one SIGTERM → fanout → every role drains concurrently. A global deadline
  arm (`fanout.subscribe()` + `sleep(grace)`) guarantees the process exits even if a role hangs.

### Web app wiring in the monolith

- Web app keeps its own app DB pool and cookie/OIDC `AuthState` (unchanged).
- Its FlightSQL data source `url` should point at the loopback listener. **Decided:** auto-seed a
  default "local" data source row (→ `http://127.0.0.1:50051`) on first start when `web` and
  `flightsql` roles are both enabled and the app DB has no data sources — an idempotent, first-run
  insert so the demo works out of the box, with an env switch to opt out. (See Decision #7.)

### Docker / packaging

A dedicated single-process image is the natural delivery vehicle for the low-cost target.

**Existing precedent and how the monolith differs.** `docker/all-in-one.Dockerfile` already bundles
all five binaries + the built frontend into one `debian:bookworm-slim` runtime — but it is a
**toolbox** image with *no default entrypoint*: you pick a binary per `docker run` (its trailing
comment shows `docker run micromegas-all flight-sql-srv`). That is not the issue's "one process"
goal. The monolith needs its own image whose **default entrypoint runs all roles**.

**New: `docker/monolith.Dockerfile`** — mirror the proven `analytics-web.Dockerfile` 4-stage shape
(it's the only existing image that already bundles a frontend, which the monolith also needs):
1. WASM builder (`ARG WASM_IMAGE=micromegas-wasm-builder:latest`) — shared, same as today.
2. Frontend build (`node:20-alpine`, `corepack`, `yarn build`) → `/app/dist`.
3. Rust build (`rust:1-bookworm`): `cargo build --release --bin micromegas-monolith`.
4. Runtime (`debian:bookworm-slim` + `ca-certificates`): copy the monolith binary to
   `/usr/local/bin/`, the frontend to `/app/frontend`, then:
   ```dockerfile
   EXPOSE 9000 50051 3000
   ENTRYPOINT ["micromegas-monolith"]
   CMD ["--roles", "all", \
        "--listen-endpoint-http", "0.0.0.0:9000", \
        "--frontend-dir", "/app/frontend"]
   ```
   (FlightSQL already binds `0.0.0.0:50051`; the web role binds `0.0.0.0:3000`. Config — both DB
   strings, object store URI, auth env, grace — is supplied via `-e`/env, as with every other image.)

**Wire into the build system** — add one row to the `SERVICES` dict in
`build/build_docker_images.py` (`:33`):
```python
"monolith": ("monolith.Dockerfile", "Single-process monolith (all roles)"),
```
The script's `ensure_wasm_builder()` dependency applies here too (frontend stage). Update
`docker/README.md` with the new image and a run example.

**`docker-compose` example** — `docker/docker-compose.monolith.yaml`, pairing the monolith with a
Postgres container and a local-volume object store (`file:///data`). This is the genuinely
one-command low-cost stack and the headline artifact for the demo/onboarding story, so it ships as
part of this work (not optional):
```yaml
services:
  postgres: { image: postgres:16, environment: [ ... ], volumes: [pgdata:/var/lib/postgresql/data] }
  micromegas:
    image: madesroches/micromegas-monolith:latest
    depends_on: [postgres]
    ports: ["9000:9000", "50051:50051", "3000:3000"]
    environment:
      MICROMEGAS_SQL_CONNECTION_STRING: postgres://.../telemetry
      MICROMEGAS_APP_SQL_CONNECTION_STRING: postgres://.../micromegas_app
      MICROMEGAS_OBJECT_STORE_URI: file:///data
      # auth: MICROMEGAS_INGESTION_API_KEYS / MICROMEGAS_ANALYTICS_OIDC_CONFIG, or --disable-auth
    volumes: ["lake:/data"]
```

**Note on `all-in-one`:** consider adding `micromegas-monolith` to that image's binary set too (so the
toolbox stays complete), but its entrypoint-less contract stays as-is. The monolith image is the one
with the real single-process entrypoint.

## Implementation Steps

### Phase 1 — Extract reusable serve functions (no behavior change to standalone binaries)
1. Move ingestion router/listener assembly into `public::servers::ingestion::serve_ingestion(...)`;
   reduce `telemetry-ingestion-srv/src/main.rs` to env-wiring + call.
2. Add `FlightSqlServerBuilder::with_lakehouse(...)` and `with_shutdown(...)`; keep `from_env` /
   `wait_for_sigterm` as the defaults used by the standalone binary.
3. Add `LakehouseContext::from_connection(...)`; route `from_env` through it; ensure both
   `migrate_db` and `migrate_lakehouse` run on the shared path. Confirm migration idempotency.
4. Extract `analytics-web-srv` serve logic into `analytics_web_srv::run_web_server(config, shutdown,
   grace)`; move `data_sources`/`screens` modules into the lib; reduce the binary `main` to wiring.
5. Add role-scoped auth to the auth crate: `default_provider::provider_with_prefix(prefix)` (with
   unprefixed fallback), `OidcConfig::from_env_var(name)`, and prefixed `MICROMEGAS_ADMINS` lookup;
   reduce `provider()` to the unprefixed call so existing callers are unchanged.
6. Run full test suite + `cargo clippy --workspace -- -D warnings`; confirm the four standalone
   binaries behave identically.

### Phase 2 — Monolith crate
7. Create `rust/monolith/` (`micromegas-monolith` bin). Add to workspace (it matches the `*` glob;
   verify it isn't caught by an `exclude`). Dependencies alphabetized per project style.
8. Implement role parsing (`--roles` / `MICROMEGAS_MONOLITH_ROLES`, default all).
9. Implement shared-resource construction (lake + lakehouse) and **two role-scoped auth providers**
   (`MICROMEGAS_INGESTION_*` / `MICROMEGAS_ANALYTICS_*` with unprefixed fallback), gated on roles.
10. Compose role futures with one `ShutdownFanout`, fail-fast bind, and a global deadline arm.
11. Implement default-data-source seeding for the web role (first-run-empty, env opt-out; Decision #7).

### Phase 3 — Packaging, tooling, tests, docs
12. Add `docker/monolith.Dockerfile` (4-stage: WASM → frontend → rust `--bin micromegas-monolith` →
    slim runtime with default entrypoint `--roles all`). Add the `"monolith"` row to the `SERVICES`
    dict in `build/build_docker_images.py`; update `docker/README.md`. Optionally add a
    `docker/docker-compose.monolith.yaml` (monolith + Postgres + local object store volume) — the
    one-command low-cost stack.
13. Add a `--monolith` path to `local_test_env/ai_scripts/start_services.py` (and `stop_services.py`)
    that builds/runs the single binary instead of four processes.
14. Integration test: boot the monolith with `--disable-auth` against a test DB + local object store
    (`file://`), assert ingestion `/health`, a FlightSQL query, and a web `/api/health` all succeed
    in one process; assert SIGTERM drains and exits within grace.
15. Documentation (see below).

## Files to Modify / Create

**Create**
- `rust/monolith/Cargo.toml`, `rust/monolith/src/main.rs` (+ `roles.rs`, `config.rs` as needed)
- `docker/monolith.Dockerfile`, `docker/docker-compose.monolith.yaml`
- `mkdocs/docs/admin/monolith.md` (new deployment guide)
- integration test under `rust/public/tests/` or `rust/monolith/tests/`

**Modify**
- `rust/public/src/servers/ingestion.rs` — add `serve_ingestion(...)`
- `rust/telemetry-ingestion-srv/src/main.rs` — call extracted fn
- `rust/public/src/servers/flight_sql_server.rs` — `with_lakehouse`, `with_shutdown`
- `rust/auth/src/default_provider.rs` — `provider_with_prefix(prefix)` (unprefixed fallback);
  `provider()` delegates to it
- `rust/auth/src/oidc.rs` — `OidcConfig::from_env_var(name)`; prefixed `MICROMEGAS_ADMINS` lookup
- `rust/analytics/src/lakehouse/lakehouse_context.rs` — `from_connection(...)`; route `from_env`
- `rust/analytics-web-srv/src/lib.rs` + `src/main.rs` — extract `run_web_server`, move modules
- `rust/Cargo.toml` — workspace member if needed; new workspace deps if any
- `local_test_env/ai_scripts/start_services.py`, `stop_services.py` — monolith mode
- `build/build_docker_images.py` — add `"monolith"` to `SERVICES`; `docker/README.md` — document it
- `mkdocs/mkdocs.yml` — nav entry; `mkdocs/docs/admin/service-lifecycle.md` — note monolith
- `CLAUDE.md` — add monolith run command under Service Management

## Trade-offs

- **One process, multiple listeners vs. single port.** Keeping the three existing listeners is the
  minimal, low-risk change and keeps the split deployment's wire protocol identical. A single-port
  reverse-proxy/gateway in front (the repo already has `http-gateway` and `connect_info_layer`
  precedent) is a larger change and is deferred. Documented as a future option.
- **New binary crate vs. a subcommand on an existing binary.** A dedicated crate keeps the four
  standalone binaries unchanged in spirit and avoids bloating, say, `telemetry-admin` with web/HTTP
  dependencies. It also gives a clean place for role-composition logic. Cost: one more crate and the
  one-time extraction refactor — which also benefits the standalone binaries (smaller `main`s,
  testable serve fns).
- **Web app over loopback FlightSQL vs. in-process query engine.** Loopback reuses the existing,
  battle-tested client path with zero query-path changes and keeps split/monolith identical. A
  future optimization could bypass gRPC for an in-process call, but it would fork the query path and
  is not worth it for the low-cost target.
- **Shared tokio runtime.** All roles share one multi-thread runtime (from `#[micromegas_main]`).
  Simpler and lower-overhead than per-role runtimes; acceptable because the target is small
  deployments. Worker-thread count can be tuned later via env if a role starves others.
- **Self-telemetry.** The monolith can point `MICROMEGAS_TELEMETRY_URL` at its own loopback ingestion
  for dogfooding, but the default is left unchanged to avoid a startup-ordering feedback loop;
  documented, not enforced.

## Documentation

- **New:** `mkdocs/docs/admin/monolith.md` — what the monolith is, when to use it (demos / low cost),
  `--roles`, the env vars (both DB strings, object store, auth, grace), the three ports, and a
  minimal single-instance + single-Postgres example. Add to `mkdocs.yml` nav.
- **Update:** `mkdocs/docs/admin/service-lifecycle.md` — note that the monolith drains all enabled
  roles from one SIGTERM with one grace period.
- **Update:** `mkdocs/docs/getting-started.md` — mention the monolith as the quickest way to a full
  local stack.
- **Update:** `CLAUDE.md` Service Management section — add the monolith run command.
- **Consider:** `public/src/lib.rs` crate docs (the architecture diagram) — add a monolith note.

## Testing Strategy

- **Unit:** role-string parsing (each role, `all`, unknown → error, env vs flag precedence).
- **Migration idempotency:** assert `migrate_db` + `migrate_lakehouse` both run cleanly on one fresh
  DB and are safe to re-run.
- **Integration (the key test):** boot the monolith with `--disable-auth`, a throwaway Postgres, and
  a `file://` object store; then in one process:
  1. `POST` a block to ingestion, `GET /health` → 200;
  2. run a FlightSQL query that returns the just-ingested data;
  3. `GET /api/health` on the web role → 200;
  4. send SIGTERM, assert clean drain + exit within grace.
- **Role subset:** boot with `--roles web` and assert only the web listener is up (others absent).
- **Regression:** existing per-service tests must still pass after the extraction (they exercise the
  same extracted functions).
- Run `python3 build/rust_ci.py` (fmt + clippy + tests) before any commit.

## Decisions (resolved during design)

These are settled and reflected in the design above; all are low-cost to reverse before release.

1. **Role flag shape — `--roles a,b,c`** (+ `MICROMEGAS_MONOLITH_ROLES`, `all` alias). One flag clap
   parses into a set; avoids the combinatorial sprawl of per-role `--enable-x/--disable-x`.
2. **Default roles — `all`, maintenance included.** The monolith's purpose is a working stack on one
   box. Without the maintenance daemon nothing materializes the lakehouse views, so analytics queries
   return empty — shipping maintenance-off would be a broken demo. Large deployments that run the
   daemon separately simply won't use the monolith for that role.
3. **Single-port fronting — out of scope for v1.** Three listeners (HTTP ingestion, gRPC FlightSQL,
   HTTP web). They already coexist in the `public` crate on shared hyper 1 / http 1 / tower 0.5, so
   separate listeners on one runtime are zero-risk. HTTP/gRPC multiplexing onto one port is feasible
   (axum 0.8 ↔ tonic 0.14 interop) but adds protocol-dispatch + three-way auth-pipeline complexity;
   deferred (see Trade-offs).
4. **Per-role auth env scheme — `MICROMEGAS_INGESTION_*` / `MICROMEGAS_ANALYTICS_*`, falling back to
   unprefixed `MICROMEGAS_*`.** Prefixed wins; fallback keeps standalone and simple-monolith setups
   working unchanged. Consistent with existing env naming; the prefix tokens are trivially renamed.
5. **Per-role auth disable — v1 ships only the global `--disable-auth`.** Running one role open while
   the other requires OIDC needs *no* new flag: leave that role's auth unconfigured (and set no
   unprefixed fallback) → `provider_with_prefix` returns `None` → that role runs open. An explicit
   `--disable-<role>-auth` is only needed to punch a hole when a shared fallback is set — deferred
   until a deployment actually needs it.
6. **Binary name — `micromegas-monolith`.** Self-describing and avoids colliding with the
   `micromegas` library crate.
7. **Default data-source seeding — yes** *(confirmed by owner)*. When `web` + `flightsql` are both
   enabled and the app DB has no data sources, the monolith auto-creates a "local" FlightSQL data
   source pointing at the loopback `:50051` so the web UI works with zero clicks. Idempotent and
   first-run-only (just a URL row; the user's bearer token is still forwarded per query), with an env
   switch to opt out. Implemented in Phase 2, step 11.

## Open Questions (need your input)

_None outstanding._ All design decisions above are settled; remaining choices are low-stakes and
reversible before release.
