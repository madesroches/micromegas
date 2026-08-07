# Web Admin for API Key Management + CLI Import Tool (#1411) Plan

## Overview

#1383 (merged, PR #1413) shipped `ingestion_api_keys` / `analytics_api_keys` tables and an
OIDC-admin-gated HTTP mint/list/revoke API for `ingestion_api_keys`, hosted on the ingestion
service. Everything else is still `psql`: `analytics_api_keys` has no HTTP path at all, and
carrying a legacy env-keyring key string forward into either table is a hand-written
`INSERT ... ON CONFLICT DO NOTHING`. This plan closes both gaps:

- A web admin page in `analytics-web-srv` for **ingestion** keys — a server-side **proxy** in
  front of ingestion's existing `/auth/api_keys` routes (the browser can't call them directly,
  see [Why a proxy](#why-a-proxy-not-a-direct-browser-call)).
- New mint/list/revoke/import HTTP routes **hosted in `analytics-web-srv`**, backed by a new,
  narrowly-scoped pool into the telemetry DB, for **analytics** keys — the one write path
  `flight-sql` must never gain.
- A gap neither existing route set covers: **importing a pre-existing key string.** The mint
  route only ever generates a fresh key; there is no HTTP way today to hash-and-store a legacy
  key's own string. This plan adds one `import` route per table (§[Import routes](#3-import-routes-new-capability)).
- A python CLI tool (`micromegas-import-keys`, alongside `micromegas-query` / `micromegas-screens`)
  that walks a legacy env keyring and calls the import routes above — no `psql`, no direct
  Postgres network access, an HTTP-reachable workstation is enough.
- Two new admin pages in `analytics-web-app` (`/admin/ingestion-keys`, `/admin/analytics-keys`),
  linked from the existing `/admin` hub.

Audience assignment (#1372) and query-time enforcement are explicitly out of scope, as is
`object-cache-srv`'s permanent env-only keyring.

## Current State

### What #1383 shipped (verified against the merged code, not just its plan)

- `rust/public/src/servers/api_keys.rs` — `ApiKeyError`, `require_key_admin(&AuthContext)`
  (`auth_type != Oidc` → 403 `NotOidc`; `!is_admin` → 403 `NotAdmin`), `mint_key` / `list_keys` /
  `revoke_key`, `api_keys_router(pool, config) -> Router` mounting `POST/GET /auth/api_keys` and
  `DELETE /auth/api_keys/{key_id}`. Hardcodes `ingestion_api_keys` — no parameter points at the
  other table. `actor(&AuthContext) -> String` is `email.unwrap_or(subject)`.
- `rust/auth/src/db_api_key.rs` — `ApiKeyTable::{Ingestion, Analytics}`, `hash_key`,
  `generate_key` (`mmk_` + 256 bits of `OsRng`, base64url-nopad), `DbApiKeyAuthProvider`,
  `dedicated_key_store_pool` (small pool tuned for the hot validation lookup — `SELECT`/
  `UPDATE (last_used_at)`, not the write path this plan needs).
- `rust/public/src/servers/ingestion.rs` — `serve_ingestion_with_api_key_config(..., api_key_config)`
  merges `api_keys_router(lake.db_pool.clone(), api_key_config)` into `protected_app` **before**
  `auth_middleware`, only `if let Some(provider) = &auth_provider`.
- `mkdocs/docs/admin/api-keys.md` — the full reference, including a "Minting an analytics key
  by hand" runbook (§246) this plan's analytics routes make obsolete, and a "Migration from the
  env keyring" runbook (§202) this plan's CLI tool makes obsolete. Both stay accurate until this
  plan lands; both need rewriting once it does (see [Documentation](#documentation)).
- `rust/public/tests/api_keys_tests.rs` — route-level tests for the ingestion API, no live DB
  (`sqlx::PgPool::connect_lazy`, per `firehose_tests.rs`'s precedent).

### `analytics-web-srv` today

- Connects to exactly one Postgres: its own `micromegas_app` config DB
  (`MICROMEGAS_APP_SQL_CONNECTION_STRING`, `web_server.rs:72`). It has **no** telemetry-DB pool
  and no knowledge of the ingestion service's network location.
- Admin gate precedent: `rust/analytics-web-srv/src/auth/handlers.rs:537` —
  `require_admin(&ValidatedUser) -> Result<(), AdminRequired>` (403 `{code: "FORBIDDEN", ...}`),
  used at the top of every admin handler in `data_sources.rs` (e.g. `get_data_source`,
  `create_data_source`). `ValidatedUser` (`auth/claims.rs:29`) carries `is_admin`, set from
  `AuthContext.is_admin` — itself populated only by `OidcAuthProvider` from the admin-list env var
  (`MICROMEGAS_ADMINS`, or `MICROMEGAS_ANALYTICS_ADMINS` on the monolith, resolved once in
  `monolith/src/main.rs`'s `analytics_admin_var`).
- **`cookie_auth_middleware` already accepts a bearer token, not only a cookie**
  (`auth/handlers.rs:454-477`: checks `Authorization: Bearer` first, falls back to the
  `id_token` cookie). This is exactly what the Python `WebClient`
  (`python/micromegas/micromegas/web_client.py`) already uses — `micromegas-screens`
  (`cli/screens.py:186-202`) builds an `OidcClientCredentialsProvider` or does an interactive
  `load_or_login`, then calls `analytics-web-srv`'s `/api/*` routes with a bearer token today.
  **The new analytics-key routes and the CLI import tool need no new auth mechanism on this side**
  — they reuse this existing path verbatim.
- Router wiring: `web_server.rs::build_protected_routes` — one `Router::new()` with every
  `/api/...` route, `.layer(Extension(app_db_pool))` + `.layer(Extension(data_source_cache))` +
  `.layer(Extension(maps_state))`, then the auth middleware layer. New routes/extensions are
  added the same way.
- Admin hub: `analytics-web-app/src/routes/AdminPage.tsx` — a grid of `AppLink` tiles
  (`/admin/data-sources`, `/admin/export-screens`, `/admin/import-screens`, `/admin/maps`), each
  wrapped in `<AuthGuard requireAdmin>`. `DataSourcesPage.tsx` (372 lines) is the CRUD-page
  template: `list*`/`create*`/`update*`/`delete*` calls into a `lib/*-api.ts` fetch wrapper,
  local `useState` for the list + a create/edit form + a `ConfirmDialog` for delete, no
  React Query. Router registration: `analytics-web-app/src/router.tsx:48-51`.

### Why a proxy, not a direct browser call

The issue allows either "the browser calling ingestion directly with the caller's own OIDC
token" or "a server-side proxy... forwarding on the operator's behalf". Only the second is
possible here: the `id_token` cookie `cookie_auth_middleware` reads is set with
`.http_only(true)` (`auth/cookies.rs:19,35`), so browser JS has no bearer token to attach to a
direct `fetch()` against ingestion. The ingestion-key admin page is therefore necessarily a
proxy, and per the issue's own admin-gating requirement, the proxy route must call
`require_admin` itself before forwarding — it carries its own service credential and must not
let a non-admin `analytics-web-srv` session ride it to ingestion.

### The gap neither existing route covers: importing an existing key string

`mint_key` (`api_keys.rs:115-167`) always calls `generate_key()` — there is no way to ask it to
hash-and-store a caller-supplied string instead. That is precisely what carrying a legacy key
string forward needs (the whole point is that existing clients keep presenting the *same* key).
Today this is `mkdocs/docs/admin/api-keys.md`'s hand-written
`INSERT ... ON CONFLICT (key_hash) DO NOTHING` (§202, §246). Removing the direct-Postgres
dependency for this operation therefore requires a **new** route, not merely a client of the
existing three — see [Import routes](#3-import-routes-new-capability).

## Design

### 1. Analytics key management API (new, `analytics-web-srv`)

New file `rust/analytics-web-srv/src/analytics_keys.rs`, modeled directly on
`rust/public/src/servers/api_keys.rs` but bound to `ValidatedUser`/cookie auth instead of
`AuthContext`/bearer, and to the new pool below instead of `app_db_pool`:

```rust
pub enum AnalyticsKeyError { Forbidden, BadRequest(String), NotFound, Database(sqlx::Error) }
// IntoResponse: 403 / 400 / 404 / 500, same ErrorResponse{code,message} shape as data_sources.rs

fn require_admin(user: &ValidatedUser) -> Result<(), AnalyticsKeyError>; // wraps auth::require_admin

async fn mint_key(Extension(pool), Extension(user), Json(MintRequest{name})) -> ...;   // POST
async fn list_keys(Extension(pool), Extension(user), Query(ListQuery{..})) -> ...;      // GET
async fn revoke_key(Extension(pool), Extension(user), Path(key_id)) -> ...;             // DELETE
async fn import_key(Extension(pool), Extension(user), Json(ImportRequest{name,key})) -> ...; // POST, §3

pub fn analytics_keys_router(pool: PgPool) -> Router;
```

- `hash_key` / `generate_key` are imported directly from `micromegas::auth::db_api_key` — the
  only two pieces of the crypto logic, and already `pub`, so nothing here reimplements them.
- SQL is identical in shape to `api_keys.rs`'s, with `analytics_api_keys` hardcoded (never a
  parameter) and `revoked_by`/`created_by` sourced from `user.email.clone().unwrap_or(user.subject.clone())`.
- **Routes live under `/api/analytics-api-keys`, not `/auth/api_keys`.** `analytics-web-srv`
  already has its own `/auth/*` routes (`login`/`callback`/`refresh`/`logout`/`me`) for a
  completely different concern (browser session lifecycle) — reusing ingestion's path would
  collide in spelling and confuse the two. Route table:

  | Route | Body / result |
  |---|---|
  | `POST {base_path}/api/analytics-api-keys` | `{"name"}` → 201 `{"key_id","name","created_at","key"}` |
  | `GET {base_path}/api/analytics-api-keys?limit=&offset=&include_revoked=` | 200 `[{"key_id","name","created_at","created_by","last_used_at","revoked_at","revoked_by"}]` |
  | `DELETE {base_path}/api/analytics-api-keys/{key_id}` | 200 `{"revoked_at"}` or 404 |
  | `POST {base_path}/api/analytics-api-keys/import` | §3 |

  No `effective_within_seconds` on revoke here — that field exists on the ingestion route
  because it threads the *validating* provider's `cache_ttl_secs`; nothing in `analytics-web-srv`
  runs a `DbApiKeyAuthProvider`, so there is no running cache TTL to report. The revocation
  latency is still bounded by whichever `flight-sql` process's cache TTL is validating the key —
  documented in the runbook, not echoed by this response.
- Mounted into `build_protected_routes` exactly like `data_sources`'s routes: added to the same
  `Router::new()` chain, gated by the same `cookie_auth_middleware` layer that already wraps
  every other `/api/...` route. No new middleware.

**Duplication, accepted.** This duplicates most of `api_keys.rs`'s ~200 lines (validation, SQL
shapes, error enum). Sharing it across two crates would mean a generic abstraction over two
different caller-identity types (`AuthContext` bearer vs. `ValidatedUser` cookie/bearer) for a
handful of near-identical handlers — the same shape the codebase already declines to share
between `data_sources.rs`/`screens.rs`/`folders.rs` today. Duplicating is the smaller change and
matches existing precedent; revisit only if a third caller shows up.

### 2. Ingestion key management proxy (`analytics-web-srv`)

New file `rust/analytics-web-srv/src/ingestion_keys_proxy.rs`:

```rust
/// Config for reaching ingestion's admin API. `None` when unconfigured — the
/// proxy routes are then not registered (§ wiring below), same pattern #1383
/// uses for "auth not configured => don't register api_keys_router".
pub struct IngestionProxyConfig {
    pub base_url: String,              // MICROMEGAS_INGESTION_ADMIN_URL, e.g. "http://127.0.0.1:8081"
    pub credentials: ServiceCredentials, // see below
}

/// One HTTP round trip per call: never cached at process level beyond the
/// bearer token itself (below) — this is an admin-console path, not a hot one.
async fn forward(
    Extension(cfg): Extension<Arc<IngestionProxyConfig>>,
    Extension(user): Extension<ValidatedUser>,
    method: Method, path_suffix: &str, query: Option<&str>, body: Option<Bytes>,
) -> Result<Response, ProxyError> {
    require_admin(&user)?;                       // checked here, before any forwarding
    let token = cfg.credentials.get_token().await?;
    let mut req = reqwest_client.request(method, format!("{}{}", cfg.base_url, path_suffix));
    if let Some(q) = query { req = req.query(...) }
    if let Some(b) = body { req = req.body(b) }
    let resp = req.bearer_auth(token).send().await?;
    // Forward ingestion's status + JSON body verbatim to the browser.
}

pub fn ingestion_keys_proxy_router(cfg: IngestionProxyConfig) -> Router {
    Router::new()
        .route(&format!("{base_path}/api/ingestion-api-keys"),
            get(list).post(mint))
        .route(&format!("{base_path}/api/ingestion-api-keys/{{key_id}}"),
            delete(revoke))
        .layer(Extension(Arc::new(cfg)))
}
```

- `list`/`mint`/`revoke` are three thin wrappers around `forward`, each pinning `method` and
  `path_suffix = "/auth/api_keys"` or `"/auth/api_keys/{key_id}"`; `list` additionally forwards
  the incoming query string verbatim (`limit`/`offset`/`include_revoked`) so the frontend can
  reuse the same paging UI as the analytics-key page.
- **No import route on this proxy.** The CLI tool calls ingestion's import route
  (§3) directly with the operator's own bearer token — it doesn't need the proxy, which exists
  only because the *browser* can't hold a bearer token (see
  [Why a proxy](#why-a-proxy-not-a-direct-browser-call)). A CLI process has no such restriction.
- `require_admin` is `analytics-web-srv`'s own gate, checked **before** `get_token()` — an
  unauthorized caller never triggers a service-credential fetch, let alone a call to ingestion.

#### Service credential (`ServiceCredentials`)

New file `rust/analytics-web-srv/src/auth/service_credentials.rs`, a small, self-contained
OAuth2 client-credentials fetcher — same shape as
`rust/telemetry-sink/src/oidc_client_credentials_decorator.rs` (`fetch_token`/`get_token` with
an expiry buffer, cached behind a `tokio::sync::Mutex`), reimplemented here rather than
depending on `telemetry-sink` from a server crate (that crate's `RequestDecorator` trait and
error types are shaped for the telemetry-sink's own retry loop, and its `get_token` is private —
there is nothing public to call). ~60 lines; no new abstraction invented, an existing one copied
into the crate that needs it.

```rust
pub struct ServiceCredentials { token_endpoint: String, client_id: String, client_secret: String,
                                 audience: Option<String>, cached: Mutex<Option<CachedToken>> }
impl ServiceCredentials {
    pub fn from_env() -> Result<Option<Self>>; // None if unconfigured, see env vars below
    pub async fn get_token(&self) -> Result<String>;
}
```

**Deliberately its own, distinctly-named credential — not a reuse of the self-telemetry
client-credentials app** (`MICROMEGAS_OIDC_CLIENT_ID`/`_SECRET`/`MICROMEGAS_OIDC_TOKEN_ENDPOINT`
that `OidcClientCredentialsDecorator::from_env()` reads for `with_auth_from_env()`). Reusing that
identity would mean a credential minted for "let this process's own self-telemetry authenticate
to ingestion" doubles as "let this process mint/revoke every ingestion key" the moment it's also
added to ingestion's admin list — conflating two very different blast radii. New vars:

- `MICROMEGAS_INGESTION_PROXY_OIDC_CLIENT_ID` / `_CLIENT_SECRET` / `_TOKEN_ENDPOINT` / `_AUDIENCE`
  (optional) — this service credential's subject must be added to ingestion's `MICROMEGAS_ADMINS`
  (or `MICROMEGAS_INGESTION_ADMINS`), documented explicitly rather than left as a silent
  precondition (mirrors #1383's own callout that `require_key_admin` needs the admin list
  populated on the *ingestion* service specifically).
- `MICROMEGAS_INGESTION_ADMIN_URL` (optional) — ingestion's base URL, e.g.
  `http://127.0.0.1:8081` for the monolith (matching its `listen_endpoint_http` default) or the
  split ingestion service's address.

`IngestionProxyConfig::from_env()` returns `None` (not an error) when either the URL or the
credential trio is unset; the proxy router is then simply not merged into `build_protected_routes`,
with a `warn!` — same non-fatal-degradation shape as `serve_ingestion`'s
"auth not configured, skip `api_keys_router`" branch. A deployment that hasn't set this up yet
keeps working; the ingestion-key admin page just isn't there (the frontend hides the tile — see
§6).

### 3. Import routes (new capability)

Same handler shape on **both** services, admin-gated identically to mint/list/revoke on each:

```
POST /auth/api_keys/import            (ingestion, api_keys.rs)
POST {base_path}/api/analytics-api-keys/import   (analytics-web-srv, analytics_keys.rs)
```

Request `{"name": "...", "key": "<the existing key string, verbatim>"}`. Response mirrors
`mint_key`'s shape minus the cleartext (the caller already has it — never echo a key back):
`{"key_id", "name", "created_at", "created_by", "imported": bool}`. `imported: true` on a fresh
`INSERT`; `false` when the hash already exists (idempotent re-run — the CLI tool can be run
against the same legacy keyring twice without side effects, matching the existing hand-written
recipe's `ON CONFLICT ... DO NOTHING` idempotency).

```sql
INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (key_hash) DO NOTHING
RETURNING key_id, name, created_at, created_by
```

`Some(row)` ⇒ `imported: true`, 201. `None` ⇒ the hash already exists; a follow-up
`SELECT key_id, name, created_at, created_by FROM ingestion_api_keys WHERE key_hash = $1`
reports the existing row, `imported: false`, 200. (Two round trips only on the conflict path —
the common "first import" path is one `INSERT ... RETURNING`.) Deliberately not the
`ON CONFLICT DO UPDATE ... RETURNING (xmax = 0) AS inserted` trick: it collapses this to one
round trip on every path, but its insert/update signal degrades under table freezing
(documented Postgres caveat) — not worth the fragility on a rarely-invoked admin path.

- 400 if `name` is empty/too long (same `MAX_NAME_BYTES` rule as mint) or `key` is empty.
- **No format validation on `key` beyond non-empty.** `hash_key` covers the whole string
  regardless of shape — this is what lets an operator-chosen legacy key of any format import
  cleanly, same reasoning `mkdocs/docs/admin/api-keys.md`'s hand-written recipe already documents
  for the `mmk_` prefix being "cosmetic to validation."
- Logs `key_id`/`name`/`created_by`/`imported` at `info!` — never the key, same as `mint_key`.

### 4. New telemetry-DB pool for `analytics-web-srv`

`WebServerConfig` gains an optional field:

```rust
pub analytics_keys_pool: Option<PgPool>,
```

- **Standalone binary** (`analytics-web-srv`): `WebServerConfig::from_cli_and_env` reads
  `MICROMEGAS_SQL_CONNECTION_STRING` — the same var name `rust/CLAUDE.md` already documents for
  "PostgreSQL connection" and that ingestion/flight-sql/monolith already use for the telemetry
  DB — `.ok()`, and if present, `sqlx::PgPool::connect(&conn_str).await` with
  `max_connections(2)` (an admin-console path, not a hot one; no need for
  `dedicated_key_store_pool`'s validation-path tuning). Absent ⇒ `None`, and the analytics-key
  routes aren't registered (`warn!`), same non-fatal pattern as §2's proxy config.
- **Monolith**: `run_web_server`'s caller (`monolith/src/main.rs`) passes
  `lake_pool.clone().filter(|_| roles.web)` — the lakehouse pool it already holds, not a second
  TCP connection. This mirrors how ingestion's own `api_keys_router` in `ingestion.rs` already
  gets `lake.db_pool.clone()` (the full lake pool, not `dedicated_key_store_pool`) — the
  narrowing to "just enough to write `analytics_api_keys`" is a Postgres-grants concern for
  deployments that separate DB roles, documented as a grant-recipe extension (§[Security](#security)),
  not something the pool object itself needs to enforce when every role shares one connection
  string, exactly as `api-keys.md`'s existing grant-recipe section already admits for the
  ingestion side.
- `analytics_keys_router(pool)` is merged into `build_protected_routes` only
  `if let Some(pool) = config.analytics_keys_pool`.

### 5. Frontend

Two new pages, one per key table — kept separate rather than tabs on one page, mirroring how
`DataSourcesPage`/`MapsPage`/screens pages are already one route each, not a shared shell:

- `analytics-web-app/src/routes/IngestionApiKeysPage.tsx` — same CRUD shape as
  `DataSourcesPage.tsx` (list table + "Mint" form + revoke `ConfirmDialog`), backed by
  `lib/ingestion-api-keys-api.ts` calling `/api/ingestion-api-keys[...]`. The mint response's
  `key` field is shown **once**, in a dismissable banner with a copy-to-clipboard button, exactly
  like the ingestion API's own doc callout ("the cleartext, returned exactly once") — never
  persisted client-side, never refetchable.
- `analytics-web-app/src/routes/AnalyticsApiKeysPage.tsx` — identical shape, calling
  `/api/analytics-api-keys[...]`.
- No frontend UI for the import routes — they exist for the CLI tool only (§6). A stray "Import"
  button inviting an operator to paste a legacy key into a browser form is the wrong shape for a
  bulk one-shot migration and reintroduces the "key transits a browser" exposure #1383 already
  avoided for mint.
- `AdminPage.tsx`: two new `AppLink` tiles ("Ingestion API Keys", "Analytics API Keys"). The
  ingestion tile is hidden when `GET /api/ingestion-api-keys` 404s at the router level (proxy not
  configured) — checked the same way `DataSourcesPage` already probes for FlightSQL availability
  (best-effort `HEAD`/`GET` on mount, hide on failure), not via a new capability-flag endpoint.
- `router.tsx`: two new `<Route>` entries under `/admin/ingestion-keys` and `/admin/analytics-keys`.

### 6. CLI import tool (python)

New `python/micromegas/micromegas/cli/import_keys.py`, registered as `micromegas-import-keys`
in `pyproject.toml`'s `[tool.poetry.scripts]`, alongside `micromegas-query`/`-screens`/`-logout`.
Follows `screens.py`'s auth-setup precedent (`OidcClientCredentialsProvider.from_env()` or
interactive `load_or_login`, `--profile` support via `micromegas.cli.config`) and `WebClient`'s
thin-`requests`-wrapper shape:

```
micromegas-import-keys --table ingestion --source env --var MICROMEGAS_API_KEYS --url http://ingestion:8081
micromegas-import-keys --table analytics --source env --var MICROMEGAS_ANALYTICS_API_KEYS --url https://analytics.example.com
```

- `--table {ingestion,analytics}` (required) — selects the import route and, for `analytics`,
  goes through the *analytics-web-srv* base URL (`--url`, same "where's the server" flag
  `micromegas-screens init` already takes); for `ingestion`, `--url` points at the ingestion
  service directly (**not** through the `analytics-web-srv` proxy — see §2's note that the CLI
  needs no proxy, it holds its own bearer token).
- `--source env --var NAME` (default `MICROMEGAS_API_KEYS` / the table-appropriate prefixed
  name) parses the same JSON keyring shape `parse_key_ring` already reads
  (`{"name": "key string"}` map) from the named env var; `--source file --path ...` as an
  alternative for a keyring saved to disk.
- For each `(name, key)` pair: `POST {import route}` with `{"name", "key"}`. Prints one line per
  key — `imported` / `already present (key_id=...)` / the error message on a 4xx — and continues
  past individual failures rather than aborting the batch (an operator migrating dozens of keys
  should see every result, not stop at the first name collision). Exit code is non-zero if any
  key failed to import.
- New `WebClient` methods (`web_client.py`) for the analytics-web-srv side:
  `import_analytics_api_key(name, key)`, `list_analytics_api_keys(...)`,
  `revoke_analytics_api_key(key_id)`, `mint_analytics_api_key(name)` — same request/response
  shape as the existing `list_screens`/`create_screen` methods. A parallel, small
  `IngestionClient` (new `python/micromegas/micromegas/ingestion_client.py`, since ingestion's
  API is a different service with a different base path (`/auth/api_keys`, not `/api/...`) and a
  different `WebClient` would be a misnomer) with the same four operations against ingestion
  directly.

## Implementation Steps

### Phase 1 — Import routes (both services)

1. `rust/public/src/servers/api_keys.rs`: add `import_key` handler + `ImportRequest`/
   `ImportResponse`, mount `POST /auth/api_keys/import` in `api_keys_router`.
2. `rust/public/tests/api_keys_tests.rs`: import happy path, re-import idempotency
   (`imported: false`, same `key_id`), 400 on empty `key`/`name`, 403 for a non-admin/non-OIDC
   caller.
3. `mkdocs/docs/admin/api-keys.md`: document the new route in the HTTP-routes table.

### Phase 2 — Analytics key API (`analytics-web-srv`)

4. `rust/analytics-web-srv/src/analytics_keys.rs` (new): `AnalyticsKeyError`, `require_admin`
   wrapper, `mint_key`/`list_keys`/`revoke_key`/`import_key`, `analytics_keys_router(pool)`.
   Export from `rust/analytics-web-srv/src/lib.rs`.
5. `rust/analytics-web-srv/src/web_server.rs`: `WebServerConfig.analytics_keys_pool: Option<PgPool>`;
   `from_cli_and_env` reads `MICROMEGAS_SQL_CONNECTION_STRING` (`.ok()`) and connects with
   `max_connections(2)`; `build_protected_routes` takes the pool and merges
   `analytics_keys_router` when `Some`, `warn!` when `None`.
6. `rust/monolith/src/main.rs`: pass `lake_pool.clone()` into `WebServerConfig` when `roles.web`
   (reusing the pool already opened for the ingestion/flightsql roles, not a second connection).
7. New tests: `rust/analytics-web-srv/tests/analytics_keys_tests.rs`, modeled on
   `data_source_tests.rs` (`ValidatedUser` extension, lazy pool for route-shape tests; `#[ignore]`d
   live-DB tests per `folders_tests.rs`'s precedent for mint/list/revoke/import round trips).

### Phase 3 — Ingestion key proxy (`analytics-web-srv`)

8. `rust/analytics-web-srv/src/auth/service_credentials.rs` (new): `ServiceCredentials`,
   `from_env()` reading the four `MICROMEGAS_INGESTION_PROXY_OIDC_*` vars, `get_token()` with the
   cache/buffer shape from `oidc_client_credentials_decorator.rs`.
9. `rust/analytics-web-srv/src/ingestion_keys_proxy.rs` (new): `IngestionProxyConfig::from_env()`,
   `forward`, `list`/`mint`/`revoke` wrappers, `ingestion_keys_proxy_router(cfg)`.
10. `web_server.rs`: merge the proxy router into `build_protected_routes` when
    `IngestionProxyConfig::from_env()` returns `Some`.
11. Tests: a mock ingestion server (`axum` router bound to a loopback port, or `wiremock` if
    already a dev-dependency anywhere in the workspace — check before adding a new one) verifying
    the proxy forwards method/path/query/body and status/body correctly, and that `require_admin`
    rejects before any outbound call (assert via a counter/mock never being hit).

### Phase 4 — Frontend

12. `lib/ingestion-api-keys-api.ts`, `lib/analytics-api-keys-api.ts` (new), modeled on
    `lib/data-sources-api.ts`'s `handleResponse`/error-class shape.
13. `routes/IngestionApiKeysPage.tsx`, `routes/AnalyticsApiKeysPage.tsx` (new), modeled on
    `DataSourcesPage.tsx`.
14. `router.tsx`: two new routes. `AdminPage.tsx`: two new tiles (ingestion tile
    availability-checked per §5).
15. `yarn lint && yarn type-check && yarn test` (per `analytics-web-app/CLAUDE.md`).

### Phase 5 — CLI import tool

16. `python/micromegas/micromegas/web_client.py`: `import_analytics_api_key`,
    `list_analytics_api_keys`, `mint_analytics_api_key`, `revoke_analytics_api_key`.
17. `python/micromegas/micromegas/ingestion_client.py` (new): same four operations against
    ingestion's `/auth/api_keys*` directly.
18. `python/micromegas/micromegas/cli/import_keys.py` (new): argument parsing, env/file keyring
    source, per-key import loop with per-key error reporting, non-zero exit on any failure.
19. `python/micromegas/pyproject.toml`: `micromegas-import-keys = "micromegas.cli.import_keys:main"`.
20. `python/micromegas/tests/cli/test_import_keys.py` (new), modeled on `tests/cli/test_logout.py`.

### Phase 6 — Documentation

21. `mkdocs/docs/admin/api-keys.md`: replace "Minting an analytics key by hand" (§246) with the
    new HTTP routes; replace "Migration from the env keyring" (§202) with the CLI tool's usage;
    extend the "Grant recipe" table with the two new grants below.

## Files to Modify

- `rust/public/src/servers/api_keys.rs`, `rust/public/tests/api_keys_tests.rs`
- `rust/analytics-web-srv/src/analytics_keys.rs` (new), `ingestion_keys_proxy.rs` (new),
  `auth/service_credentials.rs` (new), `lib.rs`, `web_server.rs`
- `rust/analytics-web-srv/tests/analytics_keys_tests.rs` (new), `ingestion_keys_proxy_tests.rs` (new)
- `rust/monolith/src/main.rs`
- `analytics-web-app/src/lib/ingestion-api-keys-api.ts` (new), `analytics-api-keys-api.ts` (new)
- `analytics-web-app/src/routes/IngestionApiKeysPage.tsx` (new), `AnalyticsApiKeysPage.tsx` (new),
  `AdminPage.tsx`, `router.tsx`
- `python/micromegas/micromegas/web_client.py`, `ingestion_client.py` (new),
  `cli/import_keys.py` (new)
- `python/micromegas/pyproject.toml`
- `python/micromegas/tests/cli/test_import_keys.py` (new)
- `mkdocs/docs/admin/api-keys.md`

## Security

- **The proxy checks `require_admin` before fetching a service-credential token or forwarding
  anything.** A non-admin `analytics-web-srv` session never causes an outbound call to ingestion,
  let alone one carrying a privileged credential.
- **The proxy's service credential is distinct from the self-telemetry one**, so a compromise of
  either doesn't automatically grant the other's privilege (see §2). Its subject must be added to
  ingestion's admin allowlist deliberately, not incidentally.
- **`analytics-web-srv` still never gains write access to `ingestion_api_keys`.** All ingestion
  writes go through ingestion's own HTTP API; the new telemetry-DB pool (§4) only ever touches
  `analytics_api_keys`, exactly the asymmetry #1383 built the two-table split to preserve.
- **Grant recipe extension** for `api-keys.md`'s separated-DB-roles table:
  ```sql
  -- analytics-web-srv's new role: write + touch only, on analytics_api_keys alone
  GRANT SELECT, INSERT ON analytics_api_keys TO micromegas_web;
  GRANT UPDATE (revoked_at, revoked_by) ON analytics_api_keys TO micromegas_web;
  -- and no grant of any kind on ingestion_api_keys
  ```
  Note this is now **two** distinct roles both writing `analytics_api_keys` in a fully
  separated-role deployment — `micromegas_web` (mint/import/revoke) and `micromegas_analytics`
  (read + `last_used_at` touch, from #1383) — with no overlap in their column grants.
- **Import never logs the key**, matching mint (§3).
- **No new secret-scanning surface**: imported keys keep whatever shape they already had; the
  route hashes and discards the cleartext identically to mint.

## Trade-offs

- **Two separate admin pages, not one tabbed page.** A single `/admin/api-keys` page with an
  ingestion/analytics tab switcher would need one page component juggling two independent
  backends (proxy vs. direct) and two independent "is this configured" checks; two routes/pages
  keep each backend's page as simple as `DataSourcesPage.tsx`, at the cost of one more router
  entry and hub tile.
- **Duplicated handler logic between `api_keys.rs` and `analytics_keys.rs`** (§1) — accepted, see
  the note there.
- **Reimplementing `ServiceCredentials` instead of depending on `telemetry-sink`** — the
  alternative (making `oidc_client_credentials_decorator.rs`'s internals `pub` and generic enough
  for a non-decorator caller) touches a crate this plan otherwise has no reason to modify, for a
  ~60-line gain.
- **Import is two round trips on the conflict path** — rejected the single-round-trip
  `ON CONFLICT DO UPDATE ... RETURNING (xmax = 0)` idiom as too fragile for the gain (see §3).

## Documentation

- `mkdocs/docs/admin/api-keys.md`: new HTTP-routes rows (import, analytics mint/list/revoke/import),
  replace the two manual-SQL runbooks, extend the grant-recipe table, note the two new env-var
  groups (`MICROMEGAS_SQL_CONNECTION_STRING` read by `analytics-web-srv`,
  `MICROMEGAS_INGESTION_PROXY_OIDC_*` / `MICROMEGAS_INGESTION_ADMIN_URL`) and their optionality.
- `mkdocs/docs/admin/monolith.md`: note that the web role now optionally shares the lake pool for
  analytics-key management.
- `CHANGELOG.md`: new CLI entry point `micromegas-import-keys`; new public
  `analytics_keys_router`/`ingestion_keys_proxy_router` if `analytics-web-srv` is ever published
  (check `Cargo.toml`'s `publish` flag before deciding whether this needs a bullet at all).

## Testing Strategy

- Rust: route-shape tests with a lazy pool (no live DB) for every new handler's validation/gating
  branches, per `firehose_tests.rs`'s precedent; `#[ignore]`d live-DB tests for the actual SQL,
  per `folders_tests.rs`'s precedent, run manually against a local Postgres.
- Proxy: a loopback mock ingestion router asserting exact forwarding of method/path/query/body
  and response passthrough, plus a "rejected before forwarding" assertion for non-admins.
- Python: `pytest` unit tests for `import_keys.py`'s per-key result classification (imported /
  already-present / errored) against a mocked `requests` session, per `test_logout.py`'s
  lightweight-mocking style; an end-to-end run against local services
  (`local_test_env/ai_scripts/start_services.py`) importing a small test keyring into both tables
  and confirming the imported keys authenticate.
- Frontend: `yarn test` for the two new pages (list render, mint form submit + one-time-key
  banner, revoke confirm flow), `yarn type-check`, `yarn lint`.
- Manual: run through the two new admin pages end-to-end against local services with
  `--disable-auth` off (OIDC required for `require_admin` to mean anything), confirming a
  non-admin OIDC session gets 403 on every new route.

## Open Questions

- Should the ingestion-key admin page be reachable at all when the proxy isn't configured, or
  should the tile disappear entirely (current plan) vs. show a "not configured, see docs" state?
  Leaning toward the current plan (hide) for a cleaner default, but worth confirming before
  building the frontend probe logic.
- Is `max_connections(2)` enough headroom for the new analytics-web-srv → telemetry-DB pool, or
  should it match `dedicated_key_store_pool`'s `max_connections(4)` for consistency even though
  the traffic shape is different (admin console, not per-request validation)? Low-stakes, easy to
  tune later; flagging so it isn't picked silently.
