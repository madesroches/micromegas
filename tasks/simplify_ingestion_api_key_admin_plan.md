# Simplify Ingestion API Key Admin: Direct DB Writes Instead of Proxy Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1458

## Overview

#1411 added a web admin page for managing `ingestion_api_keys` from `analytics-web-srv`, but
because the browser's `id_token` cookie is `http_only`, the browser has no bearer token to call
ingestion directly. `analytics-web-srv` therefore proxies mint/list/revoke calls to ingestion's own
`/auth/api_keys*` HTTP routes under a second, dedicated OIDC client-credentials identity
(`MICROMEGAS_INGESTION_PROXY_OIDC_*` / `MICROMEGAS_INGESTION_ADMIN_URL`).

The issue proposes replacing that proxy hop with direct Postgres writes from `analytics-web-srv`,
mirroring how `analytics_api_keys` already works (`analytics_keys.rs`), while leaving ingestion's
own `/auth/api_keys*` admin routes in place for the CLI tool's `--table ingestion` path. **This
plan goes one step further, per explicit direction**: ingestion should expose no admin HTTP surface
at all — it should only do ingestion. So this plan:

- Adds a new `ingestion_keys.rs` in `analytics-web-srv` (mint/list/revoke/import against
  `ingestion_api_keys`, direct Postgres writes, reusing the telemetry-DB pool already established
  for `analytics_api_keys`), replacing the proxy.
- **Removes ingestion's own `/auth/api_keys*` admin routes entirely** (`rust/public/src/servers/api_keys.rs`)
  — `analytics-web-srv` becomes the single, sole HTTP admin surface for **both** key tables.
  Ingestion keeps validating incoming API keys (`DbApiKeyAuthProvider`, unaffected) but no longer
  exposes any way to mint/list/revoke/import them over HTTP.
- **Changes the `micromegas-import-keys` CLI tool to talk to `analytics-web-srv` for both tables**,
  not ingestion directly — `--table ingestion` now calls `analytics-web-srv`'s
  `/api/ingestion-api-keys/import` route via `WebClient`, the same way `--table analytics` already
  does. The `IngestionClient` Python class (a direct-to-ingestion HTTP client) is removed.
- Fixes a real attribution bug along the way: proxied mint/revoke calls currently attribute
  `created_by`/`revoked_by` to the proxy's own service identity, not the human admin who acted.
  Direct writes gated by `analytics-web-srv`'s own `AdminUser` extractor always record the acting
  admin's real OIDC identity.
- Removes the second OIDC service credential (`MICROMEGAS_INGESTION_PROXY_OIDC_*`) and the
  ingestion-side admin allowlist it had to stay in sync with — there is now exactly **one** admin
  list to manage (`analytics-web-srv`'s own `MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS`),
  eliminating the "de-facto ingestion-key-admin list" foot-gun entirely rather than just documenting
  it.

## Current State

### The proxy being removed

`rust/analytics-web-srv/src/ingestion_keys_proxy.rs` (319 lines):
- `IngestionProxyConfig::from_env()` reads `MICROMEGAS_INGESTION_ADMIN_URL` and the
  `MICROMEGAS_INGESTION_PROXY_OIDC_{CLIENT_ID,CLIENT_SECRET,TOKEN_ENDPOINT,AUDIENCE}` quartet,
  builds an `OidcClientCredentialsDecorator` wrapped in `Arc<dyn RequestDecorator>`, and a
  `reqwest::Client` with a 10s timeout. Returns `None` (not an error) when unconfigured.
- `IngestionProxyState { config: Option<Arc<IngestionProxyConfig>> }`, layered as an `Extension`.
- `forward()` — the core proxy helper: builds the outbound request, decorates it with a bearer
  token, executes it, and forwards ingestion's status/body verbatim back to the browser.
- `list`/`mint`/`revoke` — thin `AdminUser`-gated wrappers around `forward`, mounted at
  `GET`/`POST {base_path}/api/ingestion-api-keys` and `DELETE {base_path}/api/ingestion-api-keys/{key_id}`.
  **No import route** — the CLI tool calls ingestion's import route directly instead (this plan
  changes that too, see below).
- Documented limitation: every proxied mint/revoke records the proxy's own service identity as
  `created_by`/`revoked_by`, not the acting admin (module doc comment,
  `mkdocs/docs/admin/api-keys.md:303-318`).

Tests: `rust/analytics-web-srv/tests/ingestion_keys_proxy_tests.rs` — `AdminUser` gating, a
`wiremock` stand-in for ingestion verifying forwarding, a token-fetch timeout case, and the
empty-vs-JSON-body 404 distinction.

### The pattern being mirrored: `analytics_keys.rs`

`rust/analytics-web-srv/src/analytics_keys.rs` (451 lines) already writes directly to
`analytics_api_keys` from `analytics-web-srv`:
- `AnalyticsKeysState { pool: Option<PgPool> }` — `None` when `MICROMEGAS_SQL_CONNECTION_STRING`
  is unset; routes stay registered and return 503 per-request either way (`require_pool`).
- `AnalyticsKeyError` (`BadRequest`, `NotFound`, `Database`, `NotConfigured`) → `IntoResponse`
  mapping to 400/404/500/503 with a `{code, message}` `ErrorResponse` body. No `Forbidden` variant:
  the `AdminUser` extractor (`auth/handlers.rs:553-568`) rejects before any handler body runs.
- `mint_key` / `list_keys` / `revoke_key` / `import_key`, each `AdminUser`-gated, using
  `micromegas::auth::db_api_key::{generate_key, hash_key}` and plain `sqlx::query`/`query_as`
  calls with `analytics_api_keys` hardcoded in every SQL string (never a parameter).
  `created_by`/`revoked_by` resolve as `user.email.clone().unwrap_or_else(|| user.subject.clone())`
  from the `AdminUser`-extracted caller.
- `analytics_keys_router(base_path) -> Router` mounts `POST`/`GET {base_path}/api/analytics-api-keys`,
  `DELETE {base_path}/api/analytics-api-keys/{key_id}`, `POST {base_path}/api/analytics-api-keys/import`.
- Module doc comment explicitly accepts duplicating `rust/public/src/servers/api_keys.rs`'s
  validation/SQL/error shape rather than sharing an abstraction across the two different
  caller-identity types (`AuthContext` bearer vs. `ValidatedUser` cookie/bearer).

Tests: `rust/analytics-web-srv/tests/analytics_keys_tests.rs` — `build_handler_router_with_user`
layers a synthetic `ValidatedUser` extension (the same shape `--disable-auth` uses) ahead of a
lazily-connected pool (`sqlx::PgPool::connect_lazy`, never touches a real DB in the default run),
exercising the real `AdminUser` extractor for 403s and validation for 400s; live-DB round trips are
`#[ignore]`d.

### Wiring in `analytics-web-srv`'s `web_server.rs`

- `WebServerConfig.analytics_keys_db_string: Option<String>` — read from
  `MICROMEGAS_SQL_CONNECTION_STRING` by `from_cli_and_env` (sync, no connection made yet).
- `run_web_server` (`web_server.rs:607-643`) resolves `analytics_keys_pool` via
  `PgPoolOptions::new().max_connections(2).acquire_timeout(Duration::from_secs(2)).connect_lazy(conn_str)`
  when the string is `Some`, builds `AnalyticsKeysState { pool: analytics_keys_pool }`. Separately
  builds `ingestion_proxy_config`/`ingestion_proxy_state` for the proxy.
- `build_protected_routes` (`web_server.rs:291-369`) takes both states. When `auth_state.is_some()`
  it `.merge()`s `analytics_keys_router` and `ingestion_keys_proxy_router` and layers both
  `Extension`s. When `auth_state` is `None` (`--disable-auth`), it merges
  `key_management_disabled_router` instead — a static 503 responder covering
  `{base_path}/api/analytics-api-keys[/{*rest}]` and `{base_path}/api/ingestion-api-keys[/{*rest}]`.
  **This mechanism does not change**: the new `ingestion_keys.rs` module merges into the same
  `auth_state.is_some()` branch, at the same path prefix, so `key_management_disabled_router` needs
  no changes.
- `lib.rs`: `pub mod ingestion_keys_proxy;` alongside `pub mod analytics_keys;`.

### `analytics-web-srv`'s telemetry-DB pool already covers `ingestion_api_keys`

`analytics_keys_pool` connects to whatever `MICROMEGAS_SQL_CONNECTION_STRING` points at — the same
telemetry DB that ingestion's own `DataLakeConfig::from_env()` hard-requires and that the v5
migration (`upgrade_data_lake_schema_v5`, `rust/ingestion/src/sql_migration.rs:100-140`) creates
both `ingestion_api_keys` and `analytics_api_keys` tables in. No new pool or connection string is
needed for the new module — same DB, same role, same connection string already resolved for
`analytics_api_keys`.

### Ingestion's own admin routes, being removed entirely

`rust/public/src/servers/api_keys.rs` (~430 lines) — `ApiKeyError` (`NotOidc`, `NotAdmin`,
`BadRequest`, `NotFound`, `Database`), `require_key_admin(&AuthContext)` (`auth_type != Oidc` → 403
`NotOidc`; `!is_admin` → 403 `NotAdmin`), `mint_key`/`list_keys`/`revoke_key`/`import_key` against
`ingestion_api_keys`, `api_keys_router(pool, config) -> Router` mounting `POST`/`GET /auth/api_keys`,
`DELETE /auth/api_keys/{key_id}`, `POST /auth/api_keys/import`. This is the only consumer of
`DbApiKeyConfig`'s `effective_within_seconds` field outside of `DbApiKeyAuthProvider` itself
(confirmed: `grep -rln "servers::api_keys\|api_keys_router\|require_key_admin\|ApiKeyError" rust/`
returns only this file, `ingestion.rs`, its own test file, and the proxy files being removed
above).

Wired into `rust/public/src/servers/ingestion.rs`:
- `serve_ingestion(listen_addr, lake, auth_provider, shutdown, grace)` — thin wrapper that builds
  `DbApiKeyConfig::from_env_with_prefix("")` and calls `serve_ingestion_with_api_key_config`. Used
  by `telemetry-ingestion-srv` (split deployment).
- `serve_ingestion_with_api_key_config(..., api_key_config: DbApiKeyConfig)` — the real
  implementation. When `auth_provider` is `Some`, merges
  `super::api_keys::api_keys_router(key_store_pool, api_key_config)` into `protected_app` **before**
  the `auth_middleware` layer (`ingestion.rs:175-196`), so handlers can read `Extension<AuthContext>`.
  Used directly by the monolith (`rust/monolith/src/main.rs:270-281`), which builds
  `DbApiKeyConfig::from_env_with_prefix("MICROMEGAS_INGESTION")` solely to pass in here. The
  `api_key_config` parameter's only purpose is threading the running provider's `cache_ttl_secs`
  into the admin route's `DELETE` response (`effective_within_seconds`) — once the admin route is
  gone, nothing else needs this split.
- `rust/public/src/servers/mod.rs:57`: `pub mod api_keys;`.

Tests: `rust/public/tests/api_keys_tests.rs` — route-level tests for `mint`/`list`/`revoke`/`import`
against a `sqlx::PgPool::connect_lazy` pool (no live DB in the default run), plus `#[ignore]`d
live-DB tests.

**Not affected by this removal**: `DbApiKeyConfig` itself, `DbApiKeyAuthProvider`, `ApiKeyTable`,
`dedicated_key_store_pool` (`rust/auth/src/db_api_key.rs`) — these back **key validation**
(checking an incoming `Authorization` header against the DB), a completely separate code path from
the admin HTTP routes being removed. Ingestion keeps validating API keys exactly as before; it just
stops exposing a way to manage them. Likewise, `is_admin`/`AuthContext` and the
`MICROMEGAS_ADMINS`/`MICROMEGAS_INGESTION_ADMINS` admin-list resolution in
`rust/auth/src/oidc.rs`/`default_provider.rs` are generic `OidcAuthProvider` plumbing shared with
FlightSQL's own admin-gated features (`flight_sql_service_impl.rs`, `tonic_auth_interceptor.rs`) —
out of scope; nothing here changes how `is_admin` is computed, only that ingestion no longer has
any handler left that reads it.

### The CLI tool, being changed to always target `analytics-web-srv`

`python/micromegas/micromegas/ingestion_client.py` — `IngestionClient`, a small `requests`-based
client calling ingestion's `/auth/api_keys/import` directly with a bearer token
(`import_ingestion_api_key(name, key)`), explicitly built to bypass the proxy (module doc: "never
through `analytics-web-srv`'s proxy... a CLI process has no such restriction").

`python/micromegas/micromegas/cli/import_keys.py`'s `make_client(args, parser)`
(`import_keys.py:160-170`):
```python
def make_client(args, parser):
    auth_provider = build_auth_provider(args, parser)
    if args.table == "ingestion":
        return IngestionClient(args.url, auth_provider=auth_provider)
    return WebClient(args.url, auth_provider=auth_provider)
```
and `import_one` (`import_keys.py:173-182`) branches the method name (`import_ingestion_api_key`
vs. `import_analytics_api_key`) but not the client type. `--table ingestion`'s `--url` help text
says "ingestion's own base URL... called directly, never through analytics-web-srv's proxy"
(`import_keys.py:161-166,225-229`).

`python/micromegas/micromegas/web_client.py`'s `WebClient` already has `import_analytics_api_key`
(`web_client.py:99-116`), POSTing to `{base_url}/api/analytics-api-keys/import`. It has no
`import_ingestion_api_key` method today.

Test fixture `FakeClient` in `python/micromegas/tests/cli/test_import_keys.py` already implements
**both** `import_ingestion_api_key` and `import_analytics_api_key` identically (§28-32) — the test
suite doesn't actually assert which concrete client class `make_client` returns, only that
`import_one` calls the right method name on whatever `make_client` returns. No test asserts
`IngestionClient` is instantiated.

### Frontend is unaffected

`analytics-web-app/src/lib/ingestion-api-keys-api.ts` and
`analytics-web-app/src/routes/IngestionApiKeysPage.tsx` already call
`/api/ingestion-api-keys[/...]` — the same paths the new direct-write module will keep serving. No
frontend *behavior* change is needed — only `ingestion-api-keys-api.ts`'s header comment (lines
1-11) is stale: it currently describes calling "`analytics-web-srv`'s server-side proxy
(`/api/ingestion-api-keys`), which forwards to ingestion's own `/auth/api_keys` routes under this
service's privileged service credential" and cites `rust/public/src/servers/api_keys.rs` (line 19,
being deleted by §4) as the source of the `MAX_LIMIT` constant. Both need rewriting to describe
direct-write behavior; see Documentation.

### Documentation referencing the removed pieces

- `mkdocs/docs/admin/api-keys.md` — describes ingestion's own `/auth/api_keys*` routes (§87-207),
  the proxy (intro §7-16, TLS warning §22-34, env-var table §242-248, `Web app admin pages`
  §274-325 including the de-facto-admin-list foot-gun and attribution limitation), the CLI's
  `--table ingestion` pointing at ingestion directly (§420-471), and the Grant recipe's framing of
  `analytics-web-srv` never gaining write access to `ingestion_api_keys` (§356-390 — this framing is
  now simply wrong and needs correcting, not just caveating).
- `mkdocs/docs/admin/web-app.md` — the `MICROMEGAS_INGESTION_ADMIN_URL` /
  `MICROMEGAS_INGESTION_PROXY_OIDC_*` env var block (§61-71) and the ingestion-proxy row in the
  routes table (§146).

## Design

### 1. New module: `ingestion_keys.rs` (`analytics-web-srv`)

New file `rust/analytics-web-srv/src/ingestion_keys.rs`, modeled directly on `analytics_keys.rs`,
targeting `ingestion_api_keys` instead of `analytics_api_keys`:

```rust
#[derive(Clone)]
pub struct IngestionKeysState {
    pub pool: Option<PgPool>,   // None => routes 503, same as AnalyticsKeysState
}

pub enum IngestionKeyError { BadRequest(String), NotFound, Database(sqlx::Error), NotConfigured }
// IntoResponse: 400 / 404 / 500 / 503, same ErrorResponse{code,message} shape as analytics_keys.rs.
// No `Forbidden` variant — the AdminUser extractor's own rejection runs first.

async fn mint_key(Extension(state): Extension<IngestionKeysState>, AdminUser(user): AdminUser, Json(MintRequest{name})) -> ...;   // POST
async fn list_keys(Extension(state): Extension<IngestionKeysState>, AdminUser(_user): AdminUser, Query(ListQuery{..})) -> ...;    // GET
async fn revoke_key(Extension(state): Extension<IngestionKeysState>, AdminUser(user): AdminUser, Path(key_id)) -> ...;            // DELETE
async fn import_key(Extension(state): Extension<IngestionKeysState>, AdminUser(user): AdminUser, Json(ImportRequest{name,key})) -> ...; // POST

pub fn ingestion_keys_router(base_path: &str) -> Router;
```

- Every SQL statement hardcodes `ingestion_api_keys` — never a parameter.
- `created_by`/`revoked_by` resolve from the `AdminUser`-extracted `ValidatedUser`
  (`user.email.clone().unwrap_or_else(|| user.subject.clone())`) — this is the attribution fix:
  every mint/revoke/import now records the actual admin, not a service identity.
- Routes: `POST`/`GET {base_path}/api/ingestion-api-keys`,
  `DELETE {base_path}/api/ingestion-api-keys/{key_id}` (same paths the proxy used — no frontend
  change needed), plus **`POST {base_path}/api/ingestion-api-keys/import`** — a route the proxy
  never had, now required since the CLI's `--table ingestion` path (§4 below) needs an HTTP import
  route on `analytics-web-srv`, and ingestion no longer has one of its own to fall back on.
- Response/request JSON shapes copy `analytics_keys.rs`'s (`MintRequest`, `MintResponse`,
  `ListQuery`, `KeyListEntry`, `RevokeResponse`, `ImportRequest`, `ImportResponse`, `ImportedRow`) —
  duplicated, not shared, per the "duplication, accepted" policy both modules state.
- `MAX_NAME_BYTES`/`DEFAULT_LIMIT`/`MAX_LIMIT` constants duplicated locally (same values).

**Duplication, accepted (module doc comment, same framing as `analytics_keys.rs`'s own).**

### 2. Remove the proxy

- Delete `rust/analytics-web-srv/src/ingestion_keys_proxy.rs`.
- Delete `rust/analytics-web-srv/tests/ingestion_keys_proxy_tests.rs`.
- `rust/analytics-web-srv/src/lib.rs`: replace `pub mod ingestion_keys_proxy;` with
  `pub mod ingestion_keys;`.
- `rust/analytics-web-srv/Cargo.toml`: remove `reqwest.workspace = true` — confirmed the only
  consumer of `reqwest` in this crate's `src/` is `ingestion_keys_proxy.rs`
  (`grep -rl reqwest rust/analytics-web-srv/src/`). `uuid.workspace = true` is already present
  (added for `analytics_keys.rs`) and covers the new module's needs too. Leave
  `wiremock`/`async-trait` dev-dependencies — both are still used by `tests/auth_integration.rs`.

### 3. Rewire `analytics-web-srv`'s `web_server.rs`

- Replace `use crate::ingestion_keys_proxy;` with `use crate::ingestion_keys;`.
- `build_protected_routes`'s `ingestion_proxy_state: ingestion_keys_proxy::IngestionProxyState`
  parameter becomes `ingestion_keys_state: ingestion_keys::IngestionKeysState`. The router merge and
  `Extension` layer calls swap accordingly. No other change — `key_management_disabled_router`
  needs no changes (same path prefix, same `auth_state.is_some()` branch).
- `run_web_server`: **reuse `analytics_keys_pool` for the ingestion-keys state** — both tables live
  in the same telemetry DB behind the same `MICROMEGAS_SQL_CONNECTION_STRING`:

  ```rust
  let ingestion_keys_state = ingestion_keys::IngestionKeysState {
      pool: analytics_keys_pool.clone(),
  };
  ```

  Delete the `ingestion_proxy_config`/`ingestion_proxy_state` construction block and its
  `info!`/`warn!` pair. Update the existing `analytics_keys_pool.is_some()` log to mention both
  route groups:

  ```rust
  if analytics_keys_pool.is_some() {
      info!("Telemetry-DB pool configured (analytics-api-keys, ingestion-api-keys)");
  } else {
      warn!(
          "MICROMEGAS_SQL_CONNECTION_STRING not set — /api/analytics-api-keys/* and /api/ingestion-api-keys/* will return 503"
      );
  }
  ```
- Update the `build_protected_routes(...)` call site to pass `ingestion_keys_state`.
- No change to `WebServerConfig`, `WebCliArgs`, or `from_cli_and_env`.

### 4. Remove ingestion's own admin routes

- Delete `rust/public/src/servers/api_keys.rs`.
- Delete `rust/public/tests/api_keys_tests.rs`.
- `rust/public/Cargo.toml`: remove the matching `[[test]]` block (lines 142-145: `name =
  "api_keys_tests"`, `path = "tests/api_keys_tests.rs"`, `required-features = ["server"]`) —
  Cargo errors at build time if a manifest's `[[test]]` path doesn't exist on disk, so this must
  be deleted together with `tests/api_keys_tests.rs`.
- `rust/public/src/servers/mod.rs`: remove `pub mod api_keys;` (§57, with its doc comment).
- `rust/public/src/servers/ingestion.rs`:
  - Collapse `serve_ingestion`/`serve_ingestion_with_api_key_config` into a single
    `serve_ingestion(listen_addr, lake, auth_provider, shutdown, grace)` — no `api_key_config`
    parameter, no `DbApiKeyConfig` import.
  - Remove the `super::api_keys::api_keys_router(key_store_pool, api_key_config)` merge
    (§181-184) and the now-unused `key_store_pool` binding (§157) — confirm nothing else in this
    function reads `lake.db_pool` under that name before removing it.
  - `auth_provider.is_some()` still gates whether `auth_middleware` is layered at all (ingestion
    still authenticates ingestion requests when auth is configured) — only the admin-route merge
    inside that branch goes away. Update the doc comments referencing "key-management routes" and
    "Merged before the auth_middleware layer below, so it reuses the same middleware rather than
    re-implementing auth" (no longer applicable — nothing merges there anymore).
  - Update the module/function doc comments (§95-146) that describe the two-function split and the
    `effective_within_seconds` rationale — both go away with the single collapsed function.
- `rust/monolith/src/main.rs`:
  - Remove the `api_key_config` construction and the `serve_ingestion_with_api_key_config(...)`
    call's extra argument (§270-281); call `serve_ingestion(listen_addr, lake, auth, shutdown, grace_c)`
    instead.
  - `use micromegas::servers::ingestion::serve_ingestion_with_api_key_config;` →
    `use micromegas::servers::ingestion::serve_ingestion;`.
  - `use micromegas::auth::db_api_key::{ApiKeyTable, DbApiKeyConfig, dedicated_key_store_pool};` →
    drop `DbApiKeyConfig` (confirmed unused elsewhere in this file via
    `grep -n "DbApiKeyConfig" rust/monolith/src/main.rs` — its only other appearance was the removed
    call site). `ApiKeyTable`/`dedicated_key_store_pool` stay (still used by the FlightSQL/analytics
    `ProviderBuilder::with_db_key_store` calls at §203-228, unrelated to admin routes).

### 5. CLI tool: always target `analytics-web-srv`

- `python/micromegas/micromegas/web_client.py`: add `import_ingestion_api_key(name, key)`,
  mirroring `import_analytics_api_key` exactly but POSTing to
  `self._api_url("ingestion-api-keys/import")` (i.e. `{base_url}/api/ingestion-api-keys/import`).
- `python/micromegas/micromegas/cli/import_keys.py`:
  - `make_client(args, parser)` drops the `IngestionClient` branch entirely — always
    `return WebClient(args.url, auth_provider=auth_provider)`. Its docstring's "ingestion's own base
    URL for `--table ingestion`... never through `analytics-web-srv`'s proxy" framing is now
    backwards — rewrite to say `--url` always points at `analytics-web-srv`'s base URL, for both
    tables.
  - `import_one` stays as-is: it already calls `client.import_ingestion_api_key(name, key)` /
    `client.import_analytics_api_key(name, key)` on whatever `make_client` returns — no change
    needed there since `WebClient` now has both methods.
  - `--url` argparse help text (§222-230): update to drop the "ingestion's own base URL... called
    directly" wording — both `--table` values now point at the same kind of target
    (`analytics-web-srv`).
  - Remove the now-unused `from micromegas.ingestion_client import IngestionClient` import.
- Delete `python/micromegas/micromegas/ingestion_client.py` — no remaining consumer once
  `make_client` stops branching on it (confirmed via
  `grep -rln "IngestionClient\|ingestion_client" python/micromegas --include="*.py"`: only
  `ingestion_client.py` itself and `import_keys.py`).
- No dedicated `ingestion_client` test file exists to delete (confirmed by the same grep — only
  `test_import_keys.py` references the tool, via its `FakeClient` fixture, which already
  implements both methods and needs no change).

### 6. Tests

New `rust/analytics-web-srv/tests/ingestion_keys_tests.rs`, a straight structural copy of
`analytics_keys_tests.rs` retargeted at `ingestion_keys::{IngestionKeysState, ingestion_keys_router}`
and `ingestion_api_keys`-shaped fixtures:
- `build_handler_router_with_user` wiring, `lazy_pool()`, `admin_user()`/`non_admin_user()`
  fixtures (identical — `ValidatedUser` shape doesn't change).
- 403 for a non-admin on every route (mint/list/revoke/import).
- 400 validation cases (empty/oversized `name`, empty `key` on import).
- `NotConfigured` → 503 with `IngestionKeysState { pool: None }`, no live DB touched.
- `#[ignore]`d live-DB round trips for mint/list/revoke/import, run manually against a real
  Postgres with the v5 migration applied.

Delete `rust/analytics-web-srv/tests/ingestion_keys_proxy_tests.rs` (§2) and
`rust/public/tests/api_keys_tests.rs` (§4) in full — the routes they test no longer exist.

`rust/analytics-web-srv/tests/routing_tests.rs` — update the references to
`use analytics_web_srv::ingestion_keys_proxy::IngestionProxyState;` →
`use analytics_web_srv::ingestion_keys::IngestionKeysState;`, and
`let ingestion_proxy_state = IngestionProxyState { config: None };` →
`let ingestion_keys_state = IngestionKeysState { pool: None };` (rename the local binding and its
use at the `build_protected_routes(...)` call site accordingly). The `--disable-auth` assertions
(both key-management prefixes return the static 503) need no behavioral change.

`python/micromegas/tests/cli/test_import_keys.py` — no structural change needed (`FakeClient`
already implements both methods, and `make_client` is monkeypatched out in every `main()` test), but
update the two `argv`-building tests' `--url` value from `http://ingestion:8081` to something
`analytics-web-srv`-shaped (e.g. `http://analytics:3000`) so the test data doesn't read as
misleading after this change, and add a direct unit test for `make_client` confirming it returns a
`WebClient` (not `IngestionClient`) for `--table ingestion` — no such test exists today since
`IngestionClient` used to be the correct answer for that branch.

## Files to Modify

- **New**: `rust/analytics-web-srv/src/ingestion_keys.rs`
- **New**: `rust/analytics-web-srv/tests/ingestion_keys_tests.rs`
- **Delete**: `rust/analytics-web-srv/src/ingestion_keys_proxy.rs`
- **Delete**: `rust/analytics-web-srv/tests/ingestion_keys_proxy_tests.rs`
- **Delete**: `rust/public/src/servers/api_keys.rs`
- **Delete**: `rust/public/tests/api_keys_tests.rs`
- **Delete**: `python/micromegas/micromegas/ingestion_client.py`
- `rust/analytics-web-srv/src/lib.rs` — module declaration swap
- `rust/analytics-web-srv/src/web_server.rs` — wiring swap, pool reuse, log wording
- `rust/analytics-web-srv/tests/routing_tests.rs` — import/binding rename
- `rust/analytics-web-srv/Cargo.toml` — remove unused `reqwest` dependency
- `rust/public/src/servers/mod.rs` — remove `pub mod api_keys;`
- `rust/public/src/servers/ingestion.rs` — collapse to one `serve_ingestion`, remove admin-route merge
- `rust/public/Cargo.toml` — remove the `api_keys_tests` `[[test]]` block
- `rust/monolith/src/main.rs` — drop `api_key_config`/`DbApiKeyConfig` usage, call `serve_ingestion`
- `python/micromegas/micromegas/web_client.py` — add `import_ingestion_api_key`
- `python/micromegas/micromegas/cli/import_keys.py` — `make_client` always returns `WebClient`
- `python/micromegas/tests/cli/test_import_keys.py` — URL fixture cleanup, `make_client` unit test
- `rust/auth/src/default_provider.rs` — reword the stale `POST /auth/api_keys` comment (~line 98)
- `analytics-web-app/src/lib/ingestion-api-keys-api.ts` — rewrite stale module header comment (lines 1-11, 19)
- `mkdocs/docs/admin/api-keys.md` — remove ingestion admin routes + proxy, describe single surface
- `mkdocs/docs/admin/web-app.md` — remove proxy env vars, update routes table row
- `mkdocs/docs/admin/ingestion.md` — remove the ingestion-hosted "Key management" section + admin-var row
- `mkdocs/docs/admin/monolith.md` — remove the ingestion-hosted "Key management" section + admin-gate mention
- `mkdocs/docs/admin/authentication.md` — update `POST /auth/api_keys` mint references
- `docker/README.md` — update the ingestion env-var table's mint-example reference
- `docker/docker-compose.monolith.yaml` — update the steady-state minting comment
- `mkdocs/docs/otlp/index.md` — update the `POST /auth/api_keys` mint reference
- `mkdocs/docs/grafana/authentication.md` — verify/update the `api-keys.md#minting-an-analytics-key-over-http` link
- `mkdocs/docs/admin/flight-sql.md` — verify/update the `api-keys.md#minting-an-analytics-key-over-http` link

## Trade-offs

- **Reuse `analytics_keys_pool` vs. a second dedicated pool for `ingestion_api_keys`.** Both tables
  live in the same telemetry DB behind the same connection string; a second `max_connections(2)`
  pool would double idle connections for no isolation benefit under a shared DB role. If per-service
  Postgres roles are adopted later, giving `analytics-web-srv`'s role its own separately-configured
  pool (and connection string) for `ingestion_api_keys` would be the natural point to revisit this.
- **Ingestion loses its admin HTTP surface entirely, rather than keeping it as an alternative path
  for the CLI.** This is a bigger change than the issue's original proposal (which kept ingestion's
  routes for the CLI's bearer-token path), taken per explicit direction: ingestion should only do
  ingestion. The cost is that `analytics-web-srv` is now a hard dependency for **all** ingestion-key
  administration, including the CLI migration path — there is no more "call ingestion directly, no
  extra service in the loop" option. This is judged acceptable: `analytics-web-srv` already had to
  be reachable for `--table analytics`, and it now needs to be reachable for `--table ingestion`
  too — one dependency, not a new class of one.
- **`analytics-web-srv`'s DB role permanently needs write access to `ingestion_api_keys`**, not just
  as a future separated-roles hardening concern. This was the trade-off the issue explicitly
  flagged as "worth deciding later, not a blocker now" under the narrower proposal; removing
  ingestion's admin routes entirely makes it the design, not a caveat — the Grant recipe doc section
  is updated accordingly (see Documentation).
- **This plan moves the ground the in-flight AbAC plan (`tasks/data_isolation/audience_based_access_control_plan.md`,
  not yet implemented) was designed to stand on.** That plan's Stage 0c puts the "Key-management
  API" — `POST`/`DELETE`/`GET /auth/api_keys` — "on the ingestion service (and the monolith by
  inheritance)", and its Stage 4 step 9 adds an `audience` column to `ingestion_api_keys` on the
  premise that "`resolve_audience` runs once at mint and the result is recorded on the key" via that
  same route. Once this plan ships, that route no longer exists: mint-time audience resolution will
  need to move into `analytics-web-srv`'s `ingestion_keys.rs::mint_key` (§1) instead, calling
  `MintPolicy::resolve_audience` there rather than in a handler on ingestion.
  `audience_based_access_control_plan.md` will need its Stage 0c/Stage 4 wording updated to match
  before Stage 4 is implemented. This is a forward-looking note only — reconciling that plan's text
  is not part of this plan's scope.

## Documentation

- `mkdocs/docs/admin/api-keys.md`:
  - Intro (§7-16): rewrite to state both key tables are administered exclusively through
    `analytics-web-srv`'s own routes (`/api/analytics-api-keys*`, `/api/ingestion-api-keys*`) —
    ingestion exposes no admin HTTP surface at all, only ingestion + key validation.
  - `HTTP routes (ingestion keys)` section (§87-207, describing ingestion's own `/auth/api_keys*`):
    remove entirely; fold its route/response documentation into a generalized
    `HTTP routes (key management)` section covering both `{base_path}/api/analytics-api-keys*` and
    `{base_path}/api/ingestion-api-keys*` on `analytics-web-srv`, replacing the current
    `HTTP routes (analytics keys)` section (§209-273). Keep the existing `### Minting an analytics
    key over HTTP` heading (and thus its `#minting-an-analytics-key-over-http` anchor) intact inside
    the merged section — `mkdocs/docs/grafana/authentication.md` (§30-32) and
    `mkdocs/docs/admin/flight-sql.md` (§59-61) both link to that exact anchor, and preserving it
    avoids touching either file. If the heading text changes instead, update both links accordingly.
  - TLS warning (§22-34): drop the "proxied ingestion mint/import routes... add a second hop"
    paragraph — one hop now (browser → `analytics-web-srv` → Postgres) for both tables.
  - Env-var table (§242-248): remove `MICROMEGAS_INGESTION_PROXY_OIDC_*` /
    `MICROMEGAS_INGESTION_ADMIN_URL` rows; note `MICROMEGAS_SQL_CONNECTION_STRING` now backs both
    route groups.
  - `Web app admin pages` (§274-325): rewrite the "Ingestion API Keys" bullet to describe direct
    writes; remove the de-facto-admin-list foot-gun paragraph (§295-301, no longer applies — there
    is exactly one admin list now); remove the attribution-limitation paragraph (§303-318, the bug
    this plan fixes) and instead note `created_by`/`revoked_by` always reflect the acting admin's
    own OIDC identity, for both key tables.
  - `Grant recipe (separated DB roles)` (§356-390): update the ingestion-role and
    `analytics-web-srv`-role grants — `analytics-web-srv`'s role (`micromegas_web`) now needs
    `SELECT, INSERT` on `ingestion_api_keys` too (and `UPDATE (revoked_at, revoked_by)`), matching
    its existing `analytics_api_keys` grants; narrow the `micromegas_ingestion` role's own grant
    (§370-371, currently `GRANT SELECT, INSERT` plus `UPDATE (last_used_at, revoked_at,
    revoked_by)` on `ingestion_api_keys`) down to `GRANT SELECT ON ingestion_api_keys` +
    `GRANT UPDATE (last_used_at) ON ingestion_api_keys` — mint/revoke are no longer ingestion's job
    once `DbApiKeyAuthProvider` is all that's left reading the table, mirroring the analytics role's
    read+touch-only grant shape a few lines below in the same doc; correct the framing that says
    "`analytics-web-srv` still never gains write access to `ingestion_api_keys`" (§388-390 — this is
    no longer true and must not ship as stated).
  - `Migrating from the env keyring` (§392-471): update the CLI recipe — both `--table ingestion`
    and `--table analytics` now point `--url` at `analytics-web-srv`'s base URL; remove the "ingestion's
    own base URL... not through analytics-web-srv's proxy" framing (§433-435).
- `mkdocs/docs/admin/web-app.md`:
  - Remove the `MICROMEGAS_INGESTION_ADMIN_URL` / `MICROMEGAS_INGESTION_PROXY_OIDC_*` env var block
    (§61-71) and its de-facto-admin-list comment.
  - Update the routes table row (§146) from "Proxy to ingestion's own key-management routes" to
    describe direct Postgres writes, mirroring the analytics-keys row's wording.
- `mkdocs/docs/admin/ingestion.md`:
  - `### Key management (`/auth/api_keys`)` section (lines 62-76): remove entirely — ingestion no
    longer mints/lists/revokes its own keys over HTTP; point readers at
    `analytics-web-srv`'s `/api/ingestion-api-keys*` routes in [API Keys](api-keys.md) instead.
  - Env-var table's `MICROMEGAS_ADMINS` row (line 31): drop the "required for `/auth/api_keys` to
    accept any caller" clause (no longer true — this service has no admin HTTP route left).
- `mkdocs/docs/admin/monolith.md`:
  - `### Key management (`/auth/api_keys`)` section (lines 98-108): remove entirely, same reasoning
    as `ingestion.md` — fold its "ingestion keys are validated via a DB-backed store" fact into the
    surrounding text if still relevant, but drop the route documentation and point at
    `analytics-web-srv`'s `/api/ingestion-api-keys*` routes instead.
  - The `MICROMEGAS_INGESTION_ADMINS` gate sentence (lines 95-96, "The ingestion role's
    `/auth/api_keys` gate uses `MICROMEGAS_INGESTION_ADMINS`..."): remove or rewrite — ingestion no
    longer has an admin-gated route, so this env var no longer gates anything on the ingestion role.
- `mkdocs/docs/admin/authentication.md`:
  - `### API Key Configuration` (line 112): change "Mint an ingestion key with
    `POST /auth/api_keys` on the ingestion service" to mint it via `POST /api/ingestion-api-keys` on
    `analytics-web-srv`, matching the analytics-key sentence right after it.
  - Migration walkthrough step 2 (line 675, "...or mint fresh keys via `POST /auth/api_keys` for
    callers you can update"): update the same way, to `POST /api/ingestion-api-keys` on
    `analytics-web-srv`.
- `docker/README.md`: Ingestion Server env-var table's `MICROMEGAS_API_KEYS` row (line 192,
  "...the steady state is a DB-backed key minted via `POST /auth/api_keys`..."): update to point at
  `analytics-web-srv`'s `POST /api/ingestion-api-keys` route.
- `docker/docker-compose.monolith.yaml`: the env-var comment block (line 55, "...or a non-empty
  `ingestion_api_keys` / `analytics_api_keys` DB table (see `mkdocs/docs/admin/api-keys.md` — the
  steady-state path, minted via `POST /auth/api_keys` once the ingestion role is up)"): update to
  say both tables are minted via `analytics-web-srv`'s `/api/ingestion-api-keys` /
  `/api/analytics-api-keys` routes instead — the `POST /auth/api_keys` route this comment describes
  no longer exists.
- `mkdocs/docs/otlp/index.md`: Authentication section (line 38, "mint one via
  `POST /auth/api_keys`, see [API Keys](../admin/api-keys.md)"): update to
  `POST /api/ingestion-api-keys` on `analytics-web-srv`.
- `rust/auth/src/default_provider.rs` (module doc comment on `build()`, ~line 98): the sentence "a
  deployment that mints its first key through `POST /auth/api_keys` into a previously empty table
  authenticates it on the very next request" describes a route this plan deletes. Reword to mint
  through `analytics-web-srv`'s `POST {base_path}/api/ingestion-api-keys` instead — the underlying
  point (no restart needed once the DB provider is attached) still holds, only the minting route
  changes.
- `analytics-web-app/src/lib/ingestion-api-keys-api.ts` (module header comment, lines 1-11, and the
  `MAX_LIMIT` citation at line 19): rewrite to describe `analytics-web-srv` writing directly to
  `ingestion_api_keys` (no proxy, no forwarding to ingestion, no service credential), and repoint
  the `MAX_LIMIT` citation from `rust/public/src/servers/api_keys.rs` (deleted by §4) to the new
  `rust/analytics-web-srv/src/ingestion_keys.rs`. The module's actual fetch logic/paths are
  unaffected by this rewrite — only the comment text is stale, consistent with "Frontend is
  unaffected" above.
- `CHANGELOG.md`: the `pr` skill appends an entry per its own convention; no manual edit needed here.

## Testing Strategy

- `cargo test -p analytics-web-srv` — new `ingestion_keys_tests.rs` (403/400/503 cases, no live DB),
  updated `routing_tests.rs` (`--disable-auth` still 503s both prefixes), `analytics_keys_tests.rs`
  unchanged (regression check on pool sharing).
- `cargo test -p micromegas-public` (or the crate's actual package name) — confirms
  `rust/public/tests/api_keys_tests.rs`'s removal doesn't leave a dangling reference, and that
  `ingestion.rs`/`mod.rs` compile clean after the `api_keys` module removal.
- `cargo build -p micromegas-monolith` (or `cargo build --workspace`) — confirms the
  `serve_ingestion_with_api_key_config` → `serve_ingestion` call-site update and the
  `DbApiKeyConfig` import removal compile.
- `cargo clippy --workspace -- -D warnings` and `cargo fmt` — confirms no dangling `reqwest`/
  `OidcClientCredentialsDecorator`/`RequestDecorator`/`DbApiKeyConfig` imports remain.
- `poetry run pytest` in `python/micromegas` — updated `test_import_keys.py` (new `make_client`
  unit test, URL fixture cleanup), confirms `ingestion_client.py`'s removal leaves no import error.
- Manual/`#[ignore]`d live-DB round trip: mint → list → revoke → import against a real Postgres
  with the v5 migration applied, confirming `created_by`/`revoked_by` record the test admin's own
  identity, not a service identity — for **both** tables now.
- CLI smoke test: `micromegas-import-keys --table ingestion --url http://<analytics-web-srv host>
  ...` against a running `analytics-web-srv` (or `--monolith`), confirming the import lands in
  `ingestion_api_keys` via the new route and no longer requires ingestion to be reachable at all for
  this operation.
- Frontend: manual smoke check of `Admin → Ingestion API Keys` against a `--monolith` run
  (`local_test_env/ai_scripts/start_services.py --monolith`) confirming mint/list/revoke still work
  end-to-end through the new direct-write path — the page's own fetch calls are unchanged.

## Security

- **Ingestion's attack surface shrinks**: no more OIDC-admin-gated HTTP routes on the
  fleet-facing ingestion service at all. A compromised or misconfigured `MICROMEGAS_ADMINS`/
  `MICROMEGAS_INGESTION_ADMINS` list on ingestion no longer matters for key administration, since
  nothing on ingestion reads it for that purpose anymore (the admin list is still read for
  `is_admin` resolution generically, but ingestion has no handler left that checks it).
- **Single admin list, single admin surface.** `analytics-web-srv`'s own
  `MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS` is now the only list that grants key-management
  access, for both tables — eliminating the previous two-lists-that-must-agree failure mode
  entirely (not just documenting it, as the proxy-era docs did).
- **One fewer credential to protect.** `MICROMEGAS_INGESTION_PROXY_OIDC_CLIENT_SECRET` no longer
  exists; there is no service credential capable of minting/revoking `ingestion_api_keys` on its
  own — every mint/revoke is now directly tied to the acting admin's own OIDC session.

## Open Questions

None outstanding — the CLI-targets-web-app and ingestion-admin-removal direction was clarified
during design and is reflected throughout this plan rather than left as a question.
