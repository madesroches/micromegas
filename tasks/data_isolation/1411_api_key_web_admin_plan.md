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
- Admin hub: `analytics-web-app/src/routes/AdminPage.tsx` — wraps its *entire* content (the whole
  grid of `AppLink` tiles: `/admin/data-sources`, `/admin/export-screens`,
  `/admin/import-screens`, `/admin/maps`) in a single `<AuthGuard requireAdmin>`; the tiles
  themselves carry no guard. `router.tsx` applies no guard of its own to any `/admin/*` route —
  the client-side admin gate for each admin *page* instead lives inside that page's own component,
  which self-wraps in `<AuthGuard requireAdmin>` (`DataSourcesPage.tsx:143,350`, and again at
  `358-366` for its Suspense fallback; `MapsPage.tsx:163,318`; `ImportScreensPage.tsx:512`).
  `DataSourcesPage.tsx` (372 lines) is the CRUD-page
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
`AuthContext`/bearer, and to a dedicated `AnalyticsKeysState` (holding the new pool from §4)
instead of a bare `Extension<PgPool>` — required because `build_protected_routes` already
layers `Extension<PgPool>` for `app_db_pool`; axum extensions are keyed by type, so a second
bare `Extension<PgPool>` would silently resolve to the app pool instead. `AnalyticsKeysState`
also carries the "unconfigured" case (§4), same shape as `maps::MapsState`'s `Option<Arc<dyn
ObjectStore>>`:

```rust
#[derive(Clone)]
pub struct AnalyticsKeysState {
    pub pool: Option<PgPool>,   // None => routes 503, see §4
    pub auth_disabled: bool,    // true when `--disable-auth` is on, set once at startup, see §4/Security
}

pub enum AnalyticsKeyError { BadRequest(String), NotFound, Database(sqlx::Error) }
// IntoResponse: 400 / 404 / 500, same ErrorResponse{code,message} shape as data_sources.rs;
// AnalyticsKeyError also gains `NotConfigured` (503, `state.pool == None`) and `AuthDisabled`
// (503, `state.auth_disabled == true`, checked first in every handler — see §4/Security) variants.
// No `Forbidden` variant here: the admin gate is the `AdminUser` extractor (below), whose
// rejection renders as `AdminRequired`'s own 403 body, not `AnalyticsKeyError`'s.

async fn mint_key(Extension(state): Extension<AnalyticsKeysState>, AdminUser(user): AdminUser, Json(MintRequest{name})) -> ...;   // POST
async fn list_keys(Extension(state): Extension<AnalyticsKeysState>, AdminUser(user): AdminUser, Query(ListQuery{..})) -> ...;      // GET
async fn revoke_key(Extension(state): Extension<AnalyticsKeysState>, AdminUser(user): AdminUser, Path(key_id)) -> ...;             // DELETE
async fn import_key(Extension(state): Extension<AnalyticsKeysState>, AdminUser(user): AdminUser, Json(ImportRequest{name,key})) -> ...; // POST, §3

pub fn analytics_keys_router(base_path: &str) -> Router; // routes only; state is layered separately in build_protected_routes
```

- Every handler takes `AdminUser(user): AdminUser` (`auth/handlers.rs:553-568`), not
  `Extension(user): Extension<ValidatedUser>` plus an in-handler `require_admin` call — the same
  resolution §2's `forward` uses, and for the same reason. `AdminUser` is `FromRequestParts`, so
  its rejection runs before any body extractor and renders as `AdminRequired`'s own 403 body
  (`{code: "FORBIDDEN", ...}`), not `AnalyticsKeyError`'s — so there is no `require_admin` wrapper
  and no `AnalyticsKeyError::Forbidden` variant in this file; keeping either alongside the
  extractor would leave it dead, since the extractor's rejection fires first on every request.
  `mint_key`/`import_key`'s bodies are a bounded `Json<...>`, not a raw upload, so the buffering
  concern `AdminUser`'s doc comment calls out is smaller here than in §2 — but the extractor still
  rejects a non-admin one step earlier, with no downside, so it's applied uniformly to every
  handler in this file, not just the two with a request body.
- `hash_key` / `generate_key` are imported directly from `micromegas::auth::db_api_key` — the
  only two pieces of the crypto logic, and already `pub`, so nothing here reimplements them.
- Handlers bind `Path(key_id): Path<Uuid>` and call `Uuid::new_v4()`, exactly as
  `api_keys.rs` does. `analytics-web-srv` has no `uuid` dependency today, so
  `rust/analytics-web-srv/Cargo.toml` needs `uuid.workspace = true` added (the workspace already
  declares `uuid` at `rust/Cargo.toml:102`).
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
- `analytics_keys_router(base_path)` is `.merge()`d into `build_protected_routes`'s `routes`
  chain *before* its `.layer(Extension(...))` / `.layer(middleware::from_fn(observability_middleware))`
  calls and the auth-layer that follows them (§4), so it's covered by the same
  `cookie_auth_middleware` layer that already wraps every other `/api/...` route. No new
  middleware. Routes are registered unconditionally with respect to *pool/proxy configuration*
  (503 when unconfigured) — see §4's "always register, 503 when unconfigured" rule, same as
  `/api/maps/*`. **This is narrowed by one exception: when `analytics-web-srv` itself is run with
  `--disable-auth`.** In that mode `build_protected_routes` has no `auth_state` and layers a
  hardcoded `ValidatedUser { is_admin: true, .. }` on every request instead of running
  `cookie_auth_middleware` (`web_server.rs:292-301`), so `require_admin` would unconditionally pass
  for any unauthenticated caller reaching the port. Both `analytics_keys_router` and
  `ingestion_keys_proxy_router` (§2) are therefore **still merged — never skipped —** with
  `auth_disabled: true` set on their state; every handler checks `state.auth_disabled` *first*,
  ahead of `require_admin` and any pool/config check, and returns a fixed 503 ("key management is
  unavailable when auth is disabled") when it's set, so an unauthenticated caller never reaches
  real mint/revoke/forward logic — preserving the security property `ingestion.rs`'s precedent
  (`ingestion.rs:176-196`, skipping registration of `api_keys_router` entirely) protects, without
  leaving the routes unregistered: an unregistered `/api/...` path instead falls through to
  `build_frontend`'s SPA fallback (`web_server.rs:374-383`), a `200 text/html` `index.html`
  response that would make the always-visible tiles' pages (§5) surface a JSON-parse error instead
  of a meaningful one. Logged with a `warn!` once at startup, same style as `ingestion.rs`'s
  precedent. `--disable-auth` on `analytics-web-srv` therefore still functionally disables both new
  key-management route groups, not just cookie auth on the rest of the API — called out explicitly
  in Security.

**Duplication, accepted.** This duplicates most of `api_keys.rs`'s ~200 lines (validation, SQL
shapes, error enum). Sharing it across two crates would mean a generic abstraction over two
different caller-identity types (`AuthContext` bearer vs. `ValidatedUser` cookie/bearer) for a
handful of near-identical handlers — the same shape the codebase already declines to share
between `data_sources.rs`/`screens.rs`/`folders.rs` today. Duplicating is the smaller change and
matches existing precedent; revisit only if a third caller shows up.

### 2. Ingestion key management proxy (`analytics-web-srv`)

New file `rust/analytics-web-srv/src/ingestion_keys_proxy.rs`:

```rust
/// Config for reaching ingestion's admin API, plus the client that reaches it.
/// Held as `Extension<IngestionProxyState>` (`state.config: Option<IngestionProxyConfig>`,
/// `None` when unconfigured) so the proxy routes can be registered unconditionally and
/// return 503 per-request instead of being conditionally merged — see §4/§5's "always
/// register, 503 when unconfigured" rule.
pub struct IngestionProxyConfig {
    pub base_url: String,              // MICROMEGAS_INGESTION_ADMIN_URL, e.g. "http://127.0.0.1:8081"
    pub credentials: Arc<dyn RequestDecorator>, // trait object, not the concrete decorator — see below
    pub client: reqwest::Client,        // single client, built once with an explicit timeout
}

#[derive(Clone)]
pub struct IngestionProxyState {
    pub config: Option<Arc<IngestionProxyConfig>>,
    pub auth_disabled: bool, // true when `--disable-auth` is on, set once at startup, see §4/Security
}

/// One HTTP round trip per call: never cached at process level beyond the
/// bearer token itself (below) — this is an admin-console path, not a hot one.
///
/// Not an axum handler — it's a plain helper the `list`/`mint`/`revoke` wrappers below call
/// with already-extracted values, so it performs no extraction itself and takes no `user`
/// parameter (the wrappers only need `AdminUser` to gate access; forwarding never depends on
/// *who* the admin is). Each wrapper declares `Extension<IngestionProxyState>`, then
/// `AdminUser` (`auth/handlers.rs:553`), then its body extractor (`Bytes` for `mint`, `Path` for
/// `revoke`) in that order — `AdminUser` is `FromRequestParts`, so its rejection runs *before*
/// the `Bytes` body extractor only because the wrapper signature orders it first, exactly the
/// ordering its own doc comment exists for (`auth/handlers.rs:545-552`, already used this way by
/// `maps.rs:260-262,376` for its `Bytes`-body handlers) — a non-admin's body is never buffered.
/// This still composes with the `auth_disabled` 503: that check stays first in `forward`'s body,
/// ahead of any forwarding, and under `--disable-auth` the layered
/// `ValidatedUser { is_admin: true, .. }` (§4/§1) satisfies `AdminUser`'s extractor and reaches
/// the wrapper (and thus `forward`) regardless, so the fixed 503 below still fires on every call
/// — the extractor only ever narrows who reaches the handler, it never bypasses the
/// `auth_disabled` check inside it.
async fn forward(
    state: IngestionProxyState,
    method: Method, path_suffix: &str, query: Option<&str>, body: Option<Bytes>,
) -> Result<Response, ProxyError> {
    if state.auth_disabled { return Err(ProxyError::AuthDisabled) } // 503, checked first, see §4/Security
    let Some(cfg) = state.config else { return Err(ProxyError::NotConfigured) }; // 503
    let mut req = cfg.client.request(method, format!("{}{}", cfg.base_url, path_suffix));
    if let Some(q) = query { req = req.query(...) }
    if let Some(b) = body {
        // Required: mint_key's `Json<MintRequest>` extractor 415s (`MissingJsonContentType`)
        // on a request with no Content-Type at all. Every body this proxy relays is JSON.
        req = req.body(b).header(CONTENT_TYPE, "application/json");
    }
    let mut built = req.build()?;
    cfg.credentials.decorate(&mut built).await?;  // sets the Bearer header, see below
    let resp = cfg.client.execute(built).await?;
    // Forward ingestion's status + JSON body verbatim to the browser.
}

// list/mint/revoke each extract `Extension<IngestionProxyState>` and `AdminUser` (in that
// order, ahead of any body extractor) themselves, then call `forward` with the already-extracted
// state/method/path/query/body — e.g.:
async fn mint(
    Extension(state): Extension<IngestionProxyState>,
    AdminUser(_user): AdminUser,
    body: Bytes,
) -> Result<Response, ProxyError> {
    forward(state, Method::POST, "/auth/api_keys", None, Some(body)).await
}
// `list` (GET, forwards the incoming query string, no body) and `revoke` (DELETE
// `/auth/api_keys/{key_id}`, `Path<Uuid>` extracted ahead of the call, no body) follow the same
// shape.

pub fn ingestion_keys_proxy_router(base_path: &str) -> Router {
    Router::new()
        .route(&format!("{base_path}/api/ingestion-api-keys"),
            get(list).post(mint))
        .route(&format!("{base_path}/api/ingestion-api-keys/{{key_id}}"),
            delete(revoke))
}
```

`cfg.client` is built once in `IngestionProxyConfig::from_env()` via
`reqwest::Client::builder().timeout(Duration::from_secs(10)).build()` — an explicit, bounded
timeout on every outbound admin-console call rather than an unbounded default. Requires adding
`reqwest.workspace = true` to `rust/analytics-web-srv/Cargo.toml` (the workspace already
declares `reqwest = { version = "0.12.23", ... }` at `rust/Cargo.toml:78`; `analytics-web-srv`
does not yet depend on it).

- `list`/`mint`/`revoke` are three thin wrappers around `forward`, each pinning `method` and
  `path_suffix = "/auth/api_keys"` or `"/auth/api_keys/{key_id}"`; `list` additionally forwards
  the incoming query string verbatim (`limit`/`offset`/`include_revoked`) so the frontend can
  reuse the same paging UI as the analytics-key page.
- **No import route on this proxy.** The CLI tool calls ingestion's import route
  (§3) directly with the operator's own bearer token — it doesn't need the proxy, which exists
  only because the *browser* can't hold a bearer token (see
  [Why a proxy](#why-a-proxy-not-a-direct-browser-call)). A CLI process has no such restriction.
- The `AdminUser` extractor is `analytics-web-srv`'s own gate, and it runs **before**
  `decorate()` is ever called (indeed before `forward`'s body runs at all) — an unauthorized
  caller never triggers a service-credential token fetch, let alone a call to ingestion.

#### Service credential (reusing `telemetry-sink`'s `OidcClientCredentialsDecorator`)

**No new file, no reimplementation.** `rust/telemetry-sink/src/lib.rs:25` declares
`pub mod oidc_client_credentials_decorator`, and the crate is reachable as
`micromegas::telemetry_sink::*` (`rust/public/src/lib.rs:126`, not feature-gated) — `analytics-web-srv`
already depends on `micromegas` (the `rust/public` crate). `OidcClientCredentialsDecorator::new
(token_endpoint, client_id, client_secret, audience, buffer_seconds)` is `pub`
(`oidc_client_credentials_decorator.rs:81-97`) and takes exactly the values the proxy's own
`from_env()` reads from `MICROMEGAS_INGESTION_PROXY_OIDC_*` (below); `pub trait RequestDecorator`'s
`async fn decorate(&self, request: &mut reqwest::Request) -> Result<()>`
(`request_decorator.rs:53`, impl at `oidc_client_credentials_decorator.rs:187-201`) sets the
`Authorization: Bearer` header via the same cached, expiry-buffered `get_token()` — `get_token`
itself stays private, but nothing here needs to call it directly. Zero modification to
`telemetry-sink` is required.

**`IngestionProxyConfig.credentials` is typed `Arc<dyn RequestDecorator>`, not the concrete
`OidcClientCredentialsDecorator`.** Every field on the concrete decorator (`token_endpoint`,
`client_id`, `client_secret`, `cached_token`, …) is private and `new()` is its only constructor
— there is no way for a test to pre-seed a cached token, so a test using the concrete type would
always perform a real OIDC client-credentials token fetch. `RequestDecorator` and
`TrivialRequestDecorator` (a no-op impl) are both `pub` in
`micromegas::telemetry_sink::request_decorator`, so the trait object is the injection seam:
production builds `Arc::new(OidcClientCredentialsDecorator::new(...))` in
`IngestionProxyConfig::from_env()`; tests build `Arc::new(TrivialRequestDecorator {})` directly,
skipping the token fetch entirely and needing no second wiremock stub for it. `forward` is
unchanged either way — it only ever calls the trait method, `.decorate(&mut request)`, on the
built `reqwest::Request` before `client.execute(request)`.

```rust
use micromegas::telemetry_sink::oidc_client_credentials_decorator::OidcClientCredentialsDecorator;
use std::sync::Arc;

// IngestionProxyConfig::from_env() builds this directly from the four env vars below —
// no `from_env()` on the decorator itself is used, since that reads the *different*,
// deliberately-distinct MICROMEGAS_OIDC_* self-telemetry vars (see below) — and wraps it in
// the trait object the config field expects:
let credentials: Arc<dyn RequestDecorator> = Arc::new(
    OidcClientCredentialsDecorator::new(token_endpoint, client_id, client_secret, audience, 180)
);
```

**Deliberately its own, distinctly-named credential — not a reuse of the self-telemetry
client-credentials app** (`MICROMEGAS_OIDC_CLIENT_ID`/`_SECRET`/`MICROMEGAS_OIDC_TOKEN_ENDPOINT`
that `OidcClientCredentialsDecorator::from_env()` reads for `with_auth_from_env()`). Reusing that
identity would mean a credential minted for "let this process's own self-telemetry authenticate
to ingestion" doubles as "let this process mint/revoke every ingestion key" the moment it's also
added to ingestion's admin list — conflating two very different blast radii. The type is shared;
the *instance* and its env vars are not — `IngestionProxyConfig::from_env()` constructs its own
`OidcClientCredentialsDecorator::new(...)` from `MICROMEGAS_INGESTION_PROXY_OIDC_*` rather than
calling the decorator's own `from_env()`. New vars:

- `MICROMEGAS_INGESTION_PROXY_OIDC_CLIENT_ID` / `_CLIENT_SECRET` / `_TOKEN_ENDPOINT` / `_AUDIENCE`
  (optional) — this service credential's subject must be added to ingestion's `MICROMEGAS_ADMINS`
  (or `MICROMEGAS_INGESTION_ADMINS`), documented explicitly rather than left as a silent
  precondition (mirrors #1383's own callout that `require_key_admin` needs the admin list
  populated on the *ingestion* service specifically).
- `MICROMEGAS_INGESTION_ADMIN_URL` (optional) — ingestion's base URL, e.g.
  `http://127.0.0.1:8081` for the monolith (matching its `listen_endpoint_http` default) or the
  split ingestion service's address.
- **Precondition: ingestion auth must be enabled on the target ingestion service.**
  `ingestion.rs`'s `api_keys_router` is only merged `if let Some(provider) = &auth_provider`
  (`ingestion.rs:176`) — with `--disable-auth` / `--disable-ingestion-auth` (the `local_test_env`
  default), ingestion's `/auth/api_keys*` routes don't exist at all, so a forwarded request gets a
  bare, bodyless 404 from the axum router itself, not a "not configured" signal. This is distinct
  from `revoke_key`'s own legitimate 404 (`ApiKeyError::NotFound`, a JSON `{"message": "key not
  found"}` body for an unknown `key_id`, `rust/public/src/servers/api_keys.rs:62,280`) — a normal,
  expected proxied outcome that must pass through verbatim per the "status + body verbatim" rule
  above. `forward` distinguishes the two by checking whether ingestion's 404 body parses as JSON
  with a `message` field: if it does, forward it unchanged; only a 404 with an empty/non-JSON body
  (no matching axum route, never `ApiKeyError`'s response shape) is mapped to a clearer
  `ProxyError` ("ingestion returned 404 for {path} — is ingestion auth enabled on that service?").
  This also stays distinct from a *missing proxy config* (`state.config == None`, which is a 503
  raised before any outbound call is made — see below).

`IngestionProxyConfig::from_env()` returns `None` (not an error) when either the URL or the
credential trio is unset, logged with a `warn!`. The proxy *routes* are still registered
unconditionally with respect to *this* configuration in `build_protected_routes`
(`IngestionProxyState { config: None }` layered instead) — same always-register,
503-when-unconfigured shape as `/api/maps/*` (see §4/§5). A deployment that hasn't set this up yet
keeps working; the ingestion-key admin tile stays visible and its page surfaces the 503 through
its normal error path (§5). **This is independent of, and narrower than, the `--disable-auth`
exception in §4/§1**: under `--disable-auth`, `forward` returns the fixed `AuthDisabled` 503 on
every call, checked before `require_admin` and before the `config` check (no admin gate to rely on
at all), whereas under normal auth an unset `IngestionProxyConfig` still runs `require_admin` first
(auth is running normally, `require_admin` still means something) and only 503s for the
missing-config reason once that passes.

### 3. Import routes (new capability)

Same handler shape on **both** services, admin-gated identically to mint/list/revoke on each:

```
POST /auth/api_keys/import            (ingestion, api_keys.rs)
POST {base_path}/api/analytics-api-keys/import   (analytics-web-srv, analytics_keys.rs)
```

Request `{"name": "...", "key": "<the existing key string, verbatim>"}`. Response mirrors
`mint_key`'s shape minus the cleartext (the caller already has it — never echo a key back):
`{"key_id", "name", "created_at", "created_by", "revoked_at", "imported": bool}`. `imported: true`
on a fresh `INSERT`; `false` when the hash already exists (idempotent re-run — the CLI tool can be
run against the same legacy keyring twice without side effects, matching the existing hand-written
recipe's `ON CONFLICT ... DO NOTHING` idempotency). `revoked_at` is always present (`null` unless
the existing row was revoked) so a caller can distinguish "already present and usable" from
"already present but revoked" — see the CLI's reporting below and §6.

```sql
INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (key_hash) DO NOTHING
RETURNING key_id, name, created_at, created_by, revoked_at
```

`Some(row)` ⇒ `imported: true`, 201, `revoked_at: null`. `None` ⇒ the hash already exists; a
follow-up `SELECT key_id, name, created_at, created_by, revoked_at FROM ingestion_api_keys WHERE
key_hash = $1` reports the existing row (including whether it's revoked), `imported: false`, 200.
(Two round trips only on the conflict path — the common "first import" path is one
`INSERT ... RETURNING`.) Deliberately not the
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

`WebServerConfig` gains two fields — a plain connection string (read synchronously, like
`maps_uri`) plus a slot the monolith can fill directly, so the actual `connect`/`connect_lazy`
call happens once, in `run_web_server` (already `async`), not inside `from_cli_and_env` (which
stays sync — see below):

```rust
pub struct WebServerConfig {
    ...
    /// `MICROMEGAS_SQL_CONNECTION_STRING`, read but not connected by `from_cli_and_env`.
    /// Ignored when `analytics_keys_pool_override` is `Some` (monolith case, below).
    pub analytics_keys_db_string: Option<String>,
    /// Set only by the monolith via `WebCliArgs`; carries `lake_pool` straight through so
    /// `run_web_server` never opens a second connection to the telemetry DB.
    pub analytics_keys_pool_override: Option<PgPool>,
}

pub struct WebCliArgs {
    ...
    /// Filled by the monolith with its already-open `lake_pool`; left `None` by the
    /// standalone binary, which falls back to `MICROMEGAS_SQL_CONNECTION_STRING`.
    pub analytics_keys_pool: Option<PgPool>,
}
```

- **`from_cli_and_env` stays synchronous.** It only reads `MICROMEGAS_SQL_CONNECTION_STRING`
  (`.ok()`) into `analytics_keys_db_string` and passes `cli.analytics_keys_pool` straight through
  to `analytics_keys_pool_override` — no `.await` added. The new `WebCliArgs` field is still a new
  field on an exhaustively-constructed struct, so all three of its struct-literal construction
  sites need a one-line addition: `analytics-web-srv/src/main.rs` (`analytics_keys_pool: None`),
  `monolith/src/main.rs` (below), and the `cli_args()` helper in the 9 sync `#[test]`s in
  `web_server_config_tests.rs` (`analytics_keys_pool: None`). None of the 9 tests' assertions
  change.
- **Standalone binary** (`analytics-web-srv`): leaves `WebCliArgs.analytics_keys_pool` as `None`.
  In `run_web_server`, when `analytics_keys_pool_override` is `None` and `analytics_keys_db_string`
  is `Some(conn_str)`, connect with
  `PgPoolOptions::new().max_connections(2).acquire_timeout(Duration::from_secs(2)).connect_lazy(&conn_str)`
  — `max_connections(2)` because this is an admin-console path, not the hot per-request
  validation path `dedicated_key_store_pool` tunes for, but with that same function's bounded
  `acquire_timeout` and lazy connect (`db_api_key.rs:126-138`) instead of an eager
  `PgPool::connect` with sqlx's 30s default, so a briefly-unreachable telemetry DB doesn't stop
  `analytics-web-srv` from starting. Both env var absent and connection-string present resolve
  to a pool exactly once; the analytics-key routes stay registered either way (see "Registration
  is unconditional" below).
- **Monolith**: `monolith/src/main.rs`, inside its existing `if roles.web { ... }` block
  (`main.rs:323`), fills `WebCliArgs.analytics_keys_pool = lake_pool.clone()` — `lake_pool` is
  already `Option<PgPool>` (`lakehouse.as_ref().map(|lh| lh.lake().db_pool.clone())`,
  `main.rs:196`), so this passes it through directly rather than wrapping it in another `Some`.
  `Roles::needs_lakehouse()` (`self.ingestion || self.flightsql || self.maintenance`) excludes
  `web`, so a `--roles web`-only monolith has `lakehouse = None` and thus `lake_pool = None`: in
  that case `analytics_keys_pool_override` is `None` and `run_web_server` falls back to
  `analytics_keys_db_string` (`MICROMEGAS_SQL_CONNECTION_STRING`) exactly like the standalone
  binary. When any lakehouse-needing role is also active, `lake_pool` is `Some` and
  `run_web_server` never reads `analytics_keys_db_string` or opens a second pool for it. This
  mirrors how ingestion's own `api_keys_router` in `ingestion.rs` already gets
  `lake.db_pool.clone()` (the full lake pool, not `dedicated_key_store_pool`) — the narrowing to
  "just enough to write `analytics_api_keys`" is a Postgres-grants concern for deployments that
  separate DB roles, documented as a grant-recipe extension (§[Security](#security)), not
  something the pool object itself needs to enforce when every role shares one connection
  string, exactly as `api-keys.md`'s existing grant-recipe section already admits for the
  ingestion side.
- **Registration is unconditional, both with respect to pool/proxy configuration and with respect
  to `--disable-auth`.** `run_web_server` builds `analytics_keys::AnalyticsKeysState { pool: <the
  pool above, or None>, auth_disabled: auth_state.is_none() }` either way, logs `info!`/`warn!`
  accordingly ("`/api/analytics-api-keys/*` will return 503" when the pool is `None`, same wording
  style as the existing maps-store log at `web_server.rs:510`; a distinct `warn!` when
  `auth_disabled` is set), and `build_protected_routes` always `.merge()`s
  `analytics_keys_router(base_path)` and layers `Extension(analytics_keys_state)` — never an
  `if let Some(pool) = ...` conditional merge on the pool, and never conditioned on `auth_state`
  either. This is the same always-register, 503-when-unconfigured shape `/api/maps/*` already uses
  (`maps.rs:116-123`), and it's what lets both new admin tiles stay visible unconditionally (§5)
  *and* always get a meaningful JSON error rather than falling through to the SPA fallback. **When
  `auth_state` is `None` (`--disable-auth`), the router is still merged**, but every handler checks
  `state.auth_disabled` first and returns the fixed 503 for every call before any pool/`require_admin`
  check — see the `--disable-auth` exception above and in Security.

### 5. Frontend

Two new pages, one per key table — kept separate rather than tabs on one page, mirroring how
`DataSourcesPage`/`MapsPage`/screens pages are already one route each, not a shared shell:

- `analytics-web-app/src/routes/IngestionApiKeysPage.tsx` — same CRUD shape as
  `DataSourcesPage.tsx` (list table + "Mint" form + revoke `ConfirmDialog`), backed by
  `lib/ingestion-api-keys-api.ts` calling `/api/ingestion-api-keys[...]`. The mint response's
  `key` field is shown **once**, in a dismissable banner with a copy-to-clipboard button, exactly
  like the ingestion API's own doc callout ("the cleartext, returned exactly once") — never
  persisted client-side, never refetchable. Self-wraps its content — and its Suspense fallback —
  in `<AuthGuard requireAdmin>`, per `DataSourcesPage.tsx:143,350,358-366`; `AdminPage.tsx`'s own
  guard covers only the hub grid, not the pages behind its tiles (see Current State).
- `analytics-web-app/src/routes/AnalyticsApiKeysPage.tsx` — identical shape, calling
  `/api/analytics-api-keys[...]`, and identically self-wrapped in `<AuthGuard requireAdmin>`.
- No frontend UI for the import routes — they exist for the CLI tool only (§6). A stray "Import"
  button inviting an operator to paste a legacy key into a browser form is the wrong shape for a
  bulk one-shot migration and reintroduces the "key transits a browser" exposure #1383 already
  avoided for mint.
- `AdminPage.tsx`: two new `AppLink` tiles ("Ingestion API Keys", "Analytics API Keys"), both
  **always shown**, no availability probe. `AdminPage.tsx` today is a static grid with no
  fetching at all, and the routes behind both tiles are registered unconditionally, including
  under `--disable-auth` (§4) — same precedent as the always-visible `/admin/maps` tile. A
  404-based probe wouldn't work here regardless: `build_frontend`'s SPA fallback
  (`web_server.rs:374-383`) serves `index.html` with `200` for any unmatched `/api/...` path,
  so an unregistered route never actually 404s — this is precisely
  why both routers stay registered even under `--disable-auth` rather than being omitted (§4/§1):
  omitting them would route these always-visible tiles' `fetch()` calls into that same `200
  text/html` fallback, whose `response.json()` throws a confusing parse error instead of surfacing
  a real message.
- Both `IngestionApiKeysPage.tsx` and `AnalyticsApiKeysPage.tsx` render whatever their list-fetch
  returns through the existing `ErrorBanner` path (`DataSourcesPage.tsx`'s pattern). When the
  backing pool/proxy config is absent, the route responds `503` with a `{code, message}` body
  (§4/§2) and the page shows that message ("analytics key store not configured" /
  "ingestion proxy not configured") exactly like `MapsPage` does today for an unconfigured maps
  store — no separate "not configured" UI state to build.
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
  goes through the *analytics-web-srv* base URL (`--url`, mandatory on every invocation); for
  `ingestion`, `--url` points at the ingestion service directly (**not** through the
  `analytics-web-srv` proxy — see §2's note that the CLI needs no proxy, it holds its own bearer
  token). `--profile` (optional) follows `query.py`'s precedent (`cli/query.py:100`) for supplying
  OIDC issuer/client/token-file from `~/.micromegas/config.json` — but not via
  `connection.connect(profile=...)` (`cli/connection.py:4`), which resolves to a FlightSQL client
  or bare `oidc_connection.connect(...)` result, neither an auth provider this tool can pull a
  bearer token from. Instead call `config.resolve_connection(profile=...)`
  (`cli/config.py:131`) directly to get the `ConnectionConfig` (`oidc_issuer`/`oidc_client_id`/
  `oidc_client_secret`/`oidc_audience`/`oidc_scope`/`token_file`), then feed it to
  `oidc_connection.load_or_login(issuer, client_id, client_secret, token_file, audience, scope)`
  (which does accept `token_file`, so per-profile token caching still works) or, for
  non-interactive use, `OidcClientCredentialsProvider.from_env()` — either yields a provider
  whose token `WebClient`/`IngestionClient` can attach as a bearer header. A profile carries no
  HTTP base URL, so `--url` stays mandatory regardless. (`micromegas-screens` has no `--profile`
  or `--url` flag to model this on — `init` takes a positional `server_url`, and auth env vars
  are read directly in `make_client`.)
- `--source env --var NAME` (default `MICROMEGAS_API_KEYS` / the table-appropriate prefixed
  name) parses the legacy keyring's real shape — a JSON **array** of `{"name": ..., "key": ...}`
  objects, exactly what `parse_key_ring` reads (`rust/auth/src/api_key.rs:56-64`,
  `KeyRingEntry { name, key }`; example shape at `rust/auth/src/multi.rs:28`:
  `[{"name": "test", "key": "secret"}]`) — from the named env var; `--source file --path ...` as
  an alternative for a keyring saved to disk.
- `--only NAME [NAME ...]` / `--exclude NAME [NAME ...]` (mutually exclusive, optional) select
  which entries in the keyring get imported on this run — required because not every entry
  should go anywhere: a key meant to stay `object-cache-srv`-client-only must never be imported
  at all, and a key that's valid on *both* ingestion and flight-sql today must be split into two
  distinct key strings (one per table) before either import, per
  `mkdocs/docs/admin/api-keys.md`'s migration runbook (§202). Without `--only`/`--exclude`, running
  the tool once per table against the same unmodified keyring would reproduce the shared-key
  situation the migration exists to eliminate.
- **No cross-table duplicate guard in the tool itself.** A local state file keyed by a default
  path "alongside the source" is undefined for `--source env` (both example invocations above use
  it), and even a well-defined state file would only catch duplicates recorded by *this* file —
  it misses a key shared with another workstation, or imported before the file existed. Keeping a
  shared key out of both tables is instead the documented job of `--only`/`--exclude` (above) plus
  `api-keys.md`'s pre-split step: an operator splits a dual-purpose key into two distinct strings
  first, then imports each half with `--only`/`--exclude` selecting disjoint entries per table run.
- For each selected `(name, key)` pair: `POST {import route}` with `{"name", "key"}`. Prints one
  line per key — `imported` / `already present (key_id=...)` / `already present (revoked)`
  (from the response's `revoked_at`, §3 — distinct from a usable duplicate) / the error message
  on a 4xx — and continues past individual failures rather than aborting the batch (an operator
  migrating dozens of keys should see every result, not stop at the first name collision). Exit
  code is non-zero if any key failed to import or came back `already present (revoked)`.
- New `WebClient` method (`web_client.py`) for the analytics-web-srv side:
  `import_analytics_api_key(name, key)` — same request/response shape as the existing
  `list_screens`/`create_screen` methods. Mint/list/revoke stay browser-only (§5) and get no
  Python client method — the tool's only per-key action is the import call. A parallel, small
  `IngestionClient` (new `python/micromegas/micromegas/ingestion_client.py`, since ingestion's
  API is a different service with a different base path (`/auth/api_keys`, not `/api/...`) and a
  different `WebClient` would be a misnomer) carries the single matching
  `import_ingestion_api_key(name, key)` operation against ingestion directly.

## Implementation Steps

### Phase 1 — Import routes (both services)

1. `rust/public/src/servers/api_keys.rs`: add `import_key` handler + `ImportRequest`/
   `ImportResponse`, mount `POST /auth/api_keys/import` in `api_keys_router`.
2. `rust/public/tests/api_keys_tests.rs`: import happy path, re-import idempotency
   (`imported: false`, same `key_id`), 400 on empty `key`/`name`, 403 for a non-admin/non-OIDC
   caller.
3. `mkdocs/docs/admin/api-keys.md`: document the new route in the HTTP-routes table.

### Phase 2 — Analytics key API (`analytics-web-srv`)

4. `rust/analytics-web-srv/src/analytics_keys.rs` (new): `AnalyticsKeysState { pool: Option<PgPool> }`,
   `AnalyticsKeyError` (incl. `NotConfigured` → 503, no `Forbidden` variant — see §1), every handler
   gated by the `AdminUser` extractor (`auth/handlers.rs:553-568`), not an in-handler
   `require_admin` call — `mint_key`/`list_keys`/`revoke_key`/`import_key`,
   `analytics_keys_router(base_path: &str)`.
   Export from `rust/analytics-web-srv/src/lib.rs`. `rust/analytics-web-srv/Cargo.toml`: add
   `uuid.workspace = true` (needed for `Path<Uuid>` / `Uuid::new_v4()`, not currently a dependency
   of this crate).
5. `rust/analytics-web-srv/src/web_server.rs`: `WebServerConfig.analytics_keys_db_string: Option<String>`
   + `analytics_keys_pool_override: Option<PgPool>`; `WebCliArgs.analytics_keys_pool: Option<PgPool>`.
   `rust/analytics-web-srv/src/main.rs` and the `cli_args()` helper in
   `rust/analytics-web-srv/tests/web_server_config_tests.rs` each add `analytics_keys_pool: None`
   to their `WebCliArgs` struct literal (the field is otherwise exhaustively constructed).
   `from_cli_and_env` stays sync — reads `MICROMEGAS_SQL_CONNECTION_STRING` (`.ok()`) into the
   string field and passes `cli.analytics_keys_pool` through untouched. `run_web_server` resolves
   the pool (override if `Some`, else `PgPoolOptions::new().max_connections(2).acquire_timeout(Duration::from_secs(2)).connect_lazy(&conn_str)`
   if the string is `Some`, else `None`), builds `AnalyticsKeysState { pool, auth_disabled:
   auth_state.is_none() }`, and unconditionally — regardless of `auth_state` —
   `build_protected_routes` `.merge()`s `analytics_keys_router(base_path)` and layers
   `Extension(analytics_keys_state)` before its existing
   `.layer(middleware::from_fn(observability_middleware))` call, `warn!` when the resolved pool is
   `None`. Handlers check `state.auth_disabled` first and return a fixed 503 for every call when
   it's set (i.e. `--disable-auth` is on — see §4/Security), before any pool/`require_admin` check.
6. `rust/monolith/src/main.rs`: inside the existing `if roles.web { ... }` block, set
   `WebCliArgs.analytics_keys_pool = lake_pool.clone()` (`lake_pool` is already `Option<PgPool>`,
   reusing the pool already opened for the ingestion/flightsql/maintenance roles when one of
   those is active — `analytics_keys_pool_override` being `Some` means `run_web_server` never
   reads `MICROMEGAS_SQL_CONNECTION_STRING` for this, so no second connection is opened and
   discarded). A `--roles web`-only monolith has no lakehouse (`needs_lakehouse()` excludes
   `web`), so `lake_pool` is `None` there and `run_web_server` falls back to
   `MICROMEGAS_SQL_CONNECTION_STRING` like the standalone binary.
7. New tests: `rust/analytics-web-srv/tests/analytics_keys_tests.rs`, modeled on
   `folders_tests.rs` (`sqlx::PgPool::connect_lazy` for route-shape/guard tests that never touch
   the DB) and `screens_tests.rs` (`ValidatedUser` extension injection); `#[ignore]`d live-DB
   tests per `folders_tests.rs`'s precedent for mint/list/revoke/import round trips. Also: with
   `AnalyticsKeysState.auth_disabled: true` and an admin `ValidatedUser` injected (per
   `screens_tests.rs:49-50`) — under real `--disable-auth`, `build_protected_routes` layers exactly
   this synthetic admin `ValidatedUser`, so it satisfies the `AdminUser` extractor and reaches the
   handler regardless — every route still returns 503, asserting that the in-handler
   `auth_disabled` check fires even past a passing admin extractor and ahead of the pool check.
   Also: with
   `AnalyticsKeysState { pool: None, auth_disabled: false }` and an admin `ValidatedUser`
   injected, every route returns the `NotConfigured` 503 — this needs no live DB either (the
   handler returns before ever touching `state.pool`), so it uses the same `connect_lazy`
   harness as every other route-shape test here, per `folders_tests.rs:21`'s precedent.

### Phase 3 — Ingestion key proxy (`analytics-web-srv`)

8. `rust/analytics-web-srv/src/ingestion_keys_proxy.rs` (new): `IngestionProxyConfig` (holding a
   `reqwest::Client` built with an explicit timeout, plus `credentials: Arc<dyn RequestDecorator>`
   — production's `from_env()` wraps an
   `micromegas::telemetry_sink::oidc_client_credentials_decorator::OidcClientCredentialsDecorator`
   built from the four `MICROMEGAS_INGESTION_PROXY_OIDC_*` vars in `Arc::new(...)`; no new
   credential type, see §2) + `IngestionProxyState { config: Option<Arc<IngestionProxyConfig>> }`,
   `IngestionProxyConfig::from_env()`, `forward` (a plain helper, not a handler — takes an
   already-resolved `IngestionProxyState` plus method/path/query/body), `list`/`mint`/`revoke`
   wrappers (each declaring `Extension<IngestionProxyState>` then `AdminUser` then any body
   extractor, in that order, and calling `forward` — see §2),
   `ingestion_keys_proxy_router(base_path: &str)`. Export from
   `rust/analytics-web-srv/src/lib.rs`. `rust/analytics-web-srv/Cargo.toml`:
   add `reqwest.workspace = true`.
9. `web_server.rs`: unconditionally — regardless of `auth_state` — `.merge()` the proxy router
    into `build_protected_routes`'s chain (before its layer calls) and layer
    `Extension(ingestion_proxy_state)`, where `ingestion_proxy_state.config` is `None` when
    `IngestionProxyConfig::from_env()` returns `None` (routes stay registered either way with
    respect to that config; `forward` returns 503 when `config` is `None`) and
    `ingestion_proxy_state.auth_disabled = auth_state.is_none()`. `forward` checks
    `state.auth_disabled` first and returns the fixed `AuthDisabled` 503 for every call when it's
    set (§4/§1/Security), ahead of `require_admin` and the `config` check.
10. Tests: a mock ingestion server using `wiremock` (already a workspace dependency —
    `rust/Cargo.toml:105`, already used by `rust/public/Cargo.toml` and `rust/auth/Cargo.toml`;
    add `wiremock.workspace = true` as an `analytics-web-srv` dev-dependency) verifying
    the proxy forwards method/path/query/body/`Content-Type` and status/body correctly (assert the
    mock receives `Content-Type: application/json` on the mint request). These tests build
    `IngestionProxyConfig.credentials` from `Arc::new(TrivialRequestDecorator {})`
    (`micromegas::telemetry_sink::request_decorator`, both `pub`) instead of a real
    `OidcClientCredentialsDecorator`, so no token endpoint needs stubbing and `forward`'s
    `.decorate()` call is a no-op — see §2's "Service credential" section. And that a non-admin's
    `AdminUser` rejection happens before any outbound call (assert via a counter/mock never being
    hit). Also: with `IngestionProxyState { config: None, auth_disabled: false }` and an admin
    `ValidatedUser` injected, every route returns the `NotConfigured` 503 and the wiremock server
    is never hit (no live ingestion, no live DB — this is a pure route-shape assertion). Also: with
    `IngestionProxyState.auth_disabled: true` and an admin `ValidatedUser` injected, every route
    returns 503 and the wiremock server is never hit — asserting the `auth_disabled` check
    pre-empts both `require_admin` and the outbound call.

### Phase 4 — Frontend

11. `lib/ingestion-api-keys-api.ts`, `lib/analytics-api-keys-api.ts` (new), modeled on
    `lib/data-sources-api.ts`'s `handleResponse`/error-class shape.
12. `routes/IngestionApiKeysPage.tsx`, `routes/AnalyticsApiKeysPage.tsx` (new), modeled on
    `DataSourcesPage.tsx`, each self-wrapping its content and Suspense fallback in
    `<AuthGuard requireAdmin>` (`DataSourcesPage.tsx:143,350,358-366`) — `AdminPage.tsx`'s guard
    covers only the hub grid, not these pages (§5/Current State).
13. `router.tsx`: two new routes. `AdminPage.tsx`: two new tiles, both always visible, no
    availability probe (§5).
14. `yarn lint && yarn type-check && yarn test` (per `analytics-web-app/CLAUDE.md`).

### Phase 5 — CLI import tool

15. `python/micromegas/micromegas/web_client.py`: `import_analytics_api_key(name, key)`.
16. `python/micromegas/micromegas/ingestion_client.py` (new): `import_ingestion_api_key(name, key)`
    against ingestion's `/auth/api_keys/import` directly.
17. `python/micromegas/micromegas/cli/import_keys.py` (new): argument parsing, env/file keyring
    source, per-key import loop with per-key error reporting, non-zero exit on any failure.
18. `python/micromegas/pyproject.toml`: `micromegas-import-keys = "micromegas.cli.import_keys:main"`.
19. `python/micromegas/tests/cli/test_import_keys.py` (new), modeled on `tests/cli/test_logout.py`.

### Phase 6 — Documentation

20. `mkdocs/docs/admin/api-keys.md`: replace "Minting an analytics key by hand" (§246) with the
    new HTTP routes; replace "Migration from the env keyring" (§202) with the CLI tool's usage;
    extend the "Grant recipe" table with the two new grants below. Also fix the four in-page links
    that dangle once those two sections are replaced (lines 12 and 30 link to
    `#migration-from-the-env-keyring`; lines 107 and 229 link to
    `#minting-an-analytics-key-by-hand`) by retargeting them at the new section headings; update
    the intro (lines 7-8, "three OIDC-authenticated, admin-gated HTTP routes on the ingestion
    [service]") and the HTTP-routes preamble (line 75, "All three routes live on the
    **ingestion** service") to reflect the added import route and the new analytics-key routes
    living on `analytics-web-srv`. Also rewrite lines 105-107 ("**Analytics keys are not mintable
    through this route or any other HTTP path.** They are few, manually issued, and stay out of
    every ingestion-service write path"), directly contradicted by the new
    `POST {base_path}/api/analytics-api-keys` route (§1) — replace with a pointer to
    `analytics-web-srv`'s own analytics-key routes instead of the "not mintable" claim. Also
    update `rust/public/src/servers/api_keys.rs`'s module doc comment (lines 5-9), which still
    says analytics keys "are not mintable through this API... manually issued (direct SQL by an
    operator with DB access)" and points at the runbook this plan deletes — it needs to instead
    say analytics keys are minted via `analytics-web-srv`'s own routes (§1), not through this
    ingestion-hosted API. The section replacing "Minting an analytics key by hand" gets a stable
    new heading, "Minting an analytics key over HTTP" (anchor `#minting-an-analytics-key-over-http`),
    so the other docs retargeted in step 21 below have a fixed target to link to.
21. Five more places outside `api-keys.md` still assert the now-false "analytics keys are never
    mintable over HTTP / issued by hand" claim and need the same correction — a repo-wide grep for
    "issued by hand"/"mints nothing over HTTP"/"never mintable" finds them:
    - `mkdocs/docs/admin/monolith.md:103-105` ("FlightSQL validates `analytics_api_keys` the same
      way but mints nothing over HTTP: analytics keys are issued by hand...") — rewrite to say
      minting/listing/revoking analytics keys happens through `analytics-web-srv`'s own HTTP
      routes (§1), linking `api-keys.md`. Also update this same file's env-var table
      (`monolith.md:42`, `MICROMEGAS_SQL_CONNECTION_STRING` currently marked "Yes (lake roles)")
      to note it's also read by a `--roles web`-only monolith as the fallback source for the
      analytics-key pool (§4) when no lakehouse-needing role is active. Also fold in the former
      step 22's note here, in the same edit to the same file: the web role now optionally shares
      the lake pool for analytics-key management instead of opening this second connection (§4).
    - `mkdocs/docs/admin/flight-sql.md:57-59` ("mints nothing over HTTP — it has no
      key-management routes. Analytics keys are issued by hand...") — same rewrite, pointing at
      `analytics-web-srv`'s routes instead of the by-hand runbook.
    - `mkdocs/docs/admin/authentication.md:112-115` ("...the analytics-key runbook (analytics
      keys are never mintable over HTTP; they're issued by hand)") — same rewrite.
    - `mkdocs/docs/grafana/authentication.md:21-31` (prose at 21-24, "including the analytics-key
      mint-by-hand runbook (flight-sql mints nothing over HTTP)", plus the numbered step at
      29-31, "**Mint a key by hand**... see the runbook in
      [API Keys](../admin/api-keys.md#minting-an-analytics-key-by-hand)") — same rewrite, and
      retarget the dangling anchor at `api-keys.md#minting-an-analytics-key-over-http` (the new
      heading from step 20), since step 20 deletes `#minting-an-analytics-key-by-hand`.
    - `rust/auth/src/db_api_key.rs:32-33` (`ApiKeyTable::Analytics` doc comment: "read
      credentials, issued only by hand (see the admin runbook); never mintable through the HTTP
      API") — rewrite to say analytics keys are minted/listed/revoked through
      `analytics-web-srv`'s own HTTP routes, not through this crate's ingestion-hosted API.
22. `mkdocs/docs/admin/web-app.md`: add the new optional env vars and the new API routes
    (see Documentation) to the "Environment Variables" and "API Routes" sections respectively.
    Note next to `MICROMEGAS_INGESTION_PROXY_OIDC_*`/`MICROMEGAS_INGESTION_ADMIN_URL` that
    configuring the proxy makes `analytics-web-srv`'s own admin list a de-facto
    ingestion-key-admin list (see Security), so operators who deliberately keep
    `MICROMEGAS_INGESTION_ADMINS` distinct from `MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS`
    must keep them aligned or leave the proxy unconfigured.
23. `CHANGELOG.md`: add `## Unreleased` bullets for the ingestion import route, the analytics-key
    mint/list/revoke/import routes and their admin pages, the `micromegas-import-keys` CLI entry
    point, and the new env vars; and **amend** the existing `## Unreleased` → `**Auth:**` bullet
    for #1383 (`CHANGELOG.md:13`) — drop the "analytics keys are never mintable over HTTP"
    parenthetical, add `POST /auth/api_keys/import` to its route list, and repoint the runbook
    reference at the new analytics-key HTTP routes instead of the by-hand runbook step 20 deletes
    (see [Documentation](#documentation)).

## Files to Modify

- `rust/public/src/servers/api_keys.rs`, `rust/public/tests/api_keys_tests.rs`
- `rust/auth/src/db_api_key.rs` (`ApiKeyTable::Analytics` doc comment, see Phase 6 step 21)
- `rust/analytics-web-srv/src/analytics_keys.rs` (new), `ingestion_keys_proxy.rs` (new),
  `lib.rs`, `web_server.rs`, `main.rs`, `Cargo.toml`
  (`reqwest.workspace = true`, `uuid.workspace = true`, `wiremock.workspace = true` dev-dependency)
- `rust/analytics-web-srv/tests/analytics_keys_tests.rs` (new), `ingestion_keys_proxy_tests.rs` (new),
  `web_server_config_tests.rs` (`cli_args()` helper)
- `rust/monolith/src/main.rs`
- `analytics-web-app/src/lib/ingestion-api-keys-api.ts` (new), `analytics-api-keys-api.ts` (new)
- `analytics-web-app/src/routes/IngestionApiKeysPage.tsx` (new), `AnalyticsApiKeysPage.tsx` (new),
  `AdminPage.tsx`, `router.tsx`
- `python/micromegas/micromegas/web_client.py`, `ingestion_client.py` (new),
  `cli/import_keys.py` (new)
- `python/micromegas/pyproject.toml`
- `python/micromegas/tests/cli/test_import_keys.py` (new)
- `mkdocs/docs/admin/api-keys.md`, `mkdocs/docs/admin/web-app.md`, `mkdocs/docs/admin/monolith.md`,
  `mkdocs/docs/admin/flight-sql.md`, `mkdocs/docs/admin/authentication.md`,
  `mkdocs/docs/grafana/authentication.md` (see Phase 6 step 21 for the last four)
- `CHANGELOG.md`

## Security

- **Both new route groups return a fixed 503 for every request when `analytics-web-srv` runs with
  `--disable-auth`, rather than being omitted from registration.** In that mode
  `build_protected_routes` layers a hardcoded `ValidatedUser { is_admin: true, .. }` on every
  request instead of running `cookie_auth_middleware` (`web_server.rs:292-301`), so `require_admin`
  would unconditionally pass for any unauthenticated caller. Left to their normal logic,
  `analytics_keys_router` would let such a caller mint real `analytics_api_keys` rows (valid on a
  `flight-sql` that may have auth enabled independently), and `ingestion_keys_proxy_router` would
  let one drive `forward` → `get_token()` → mint/revoke against ingestion's real admin API, using
  the proxy's own privileged service credential. Both routers are therefore still merged — unlike
  `ingestion.rs`'s own precedent of skipping registration for `api_keys_router`
  (`ingestion.rs:176-196`) — but every handler checks `state.auth_disabled` first and returns 503
  unconditionally, before `require_admin`, before touching `state.pool`/`state.config`, and before
  any outbound call: an unauthenticated caller can never reach the mint/revoke/forward logic at
  all. Registering the routers rather than omitting them also keeps the admin pages' error
  surfacing meaningful (§5) instead of falling through to the SPA fallback's `200 text/html`
  (`web_server.rs:374-383`).
- **The proxy's `forward` takes the `AdminUser` extractor (§2), not an in-handler `require_admin`
  call, so the admin check runs before fetching a service-credential token, before forwarding
  anything, and before the request body is even buffered.** A non-admin `analytics-web-srv`
  session never causes an outbound call to ingestion, let alone one carrying a privileged
  credential.
- **The proxy's service credential is distinct from the self-telemetry one**, so a compromise of
  either doesn't automatically grant the other's privilege (see §2). Its subject must be added to
  ingestion's admin allowlist deliberately, not incidentally.
- **Enabling the proxy makes `analytics-web-srv`'s admin list a de-facto ingestion-key-admin
  list.** Ingestion resolves its own admin list from `MICROMEGAS_INGESTION_ADMINS`, falling back
  to `MICROMEGAS_ADMINS` (`rust/auth/src/default_provider.rs:77-83`, via
  `ProviderBuilder::new("MICROMEGAS_INGESTION")` in `rust/monolith/src/main.rs:205`).
  `analytics-web-srv` resolves a *different* list — `MICROMEGAS_ADMINS` directly
  (`rust/analytics-web-srv/src/main.rs:37`) or `MICROMEGAS_ANALYTICS_ADMINS` → `MICROMEGAS_ADMINS`
  on the monolith (`analytics_admin_var`). The proxy gates only on its own `require_admin` and
  then forwards under its privileged service credential, so anyone in the analytics admin list
  gets mint/list/revoke on `ingestion_api_keys` even if deliberately excluded from
  `MICROMEGAS_INGESTION_ADMINS`. Operators who intentionally keep the two admin lists separate
  must either keep them aligned or leave the proxy unconfigured (`IngestionProxyConfig::from_env()`
  returning `None`) — configuring the proxy is itself the decision to unify them.
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
- **Inherits #1383's TLS-terminating-ingress prerequisite.** `POST {base_path}/api/analytics-api-keys`
  returns a one-time cleartext key over whatever transport the request arrives on, exactly like
  ingestion's `mint_key` (`mkdocs/docs/admin/api-keys.md:14-21`); `analytics-web-srv` binds plain
  HTTP with no TLS acceptor of its own (`web_server.rs:553,568`), same as ingestion. The import
  routes on both services are the mirror case: they carry a legacy key's cleartext *inbound* in
  the request body. None of this is new to this plan — deploy behind a TLS-terminating
  reverse proxy, as `api-keys.md` already documents for ingestion's mint route.

## Trade-offs

- **Two separate admin pages, not one tabbed page.** A single `/admin/api-keys` page with an
  ingestion/analytics tab switcher would need one page component juggling two independent
  backends (proxy vs. direct) and two independent "is this configured" checks; two routes/pages
  keep each backend's page as simple as `DataSourcesPage.tsx`, at the cost of one more router
  entry and hub tile.
- **Duplicated handler logic between `api_keys.rs` and `analytics_keys.rs`** (§1) — accepted, see
  the note there.
- **Reusing `telemetry-sink`'s `OidcClientCredentialsDecorator` directly, rather than a new
  proxy-local credential type** — its `new()`/`decorate()` are already `pub` and reachable via
  `micromegas::telemetry_sink::*`, so no modification to that crate is needed; the proxy only
  supplies its own env-sourced arguments and a distinct instance (see §2).
- **Import is two round trips on the conflict path** — rejected the single-round-trip
  `ON CONFLICT DO UPDATE ... RETURNING (xmax = 0)` idiom as too fragile for the gain (see §3).

## Documentation

- `mkdocs/docs/admin/api-keys.md`: new HTTP-routes rows (import, analytics mint/list/revoke/import),
  replace the two manual-SQL runbooks, extend the grant-recipe table, note the two new env-var
  groups (`MICROMEGAS_SQL_CONNECTION_STRING` read by `analytics-web-srv`,
  `MICROMEGAS_INGESTION_PROXY_OIDC_*` / `MICROMEGAS_INGESTION_ADMIN_URL`) and their optionality.
- `mkdocs/docs/admin/monolith.md`: note that the web role now optionally shares the lake pool for
  analytics-key management; rewrite its "analytics keys are issued by hand" sentence (§103-105);
  update its `MICROMEGAS_SQL_CONNECTION_STRING` env-var row to cover a `--roles web`-only monolith
  (see Phase 6 step 21).
- `mkdocs/docs/admin/flight-sql.md`, `mkdocs/docs/admin/authentication.md`,
  `mkdocs/docs/grafana/authentication.md`, `rust/auth/src/db_api_key.rs`: same "issued by hand /
  never mintable over HTTP" claim, each rewritten to point at `analytics-web-srv`'s own
  analytics-key routes; `grafana/authentication.md`'s dangling
  `#minting-an-analytics-key-by-hand` link is retargeted at `api-keys.md`'s new
  `#minting-an-analytics-key-over-http` heading (see Phase 6 step 21).
- `mkdocs/docs/admin/api-keys.md`: extend the existing TLS-terminating-ingress warning
  (currently scoped to ingestion's mint route) to cover the new analytics mint route and both
  import routes' inbound cleartext (see Security).
- `mkdocs/docs/admin/web-app.md`: this is the canonical env-var/route reference for
  `analytics-web-srv` specifically (its "Environment Variables → Required / Optional" table,
  lines 14-57, and its enumerated "API Routes" list, lines 111-129) — add the five new optional
  env vars this service now reads (`MICROMEGAS_SQL_CONNECTION_STRING`,
  `MICROMEGAS_INGESTION_PROXY_OIDC_CLIENT_ID`/`_CLIENT_SECRET`/`_TOKEN_ENDPOINT`/`_AUDIENCE`,
  `MICROMEGAS_INGESTION_ADMIN_URL`) to "Optional" alongside `MICROMEGAS_MAPS_OBJECT_STORE_URI`'s
  existing 503-when-absent pattern — noting that setting the ingestion-proxy vars makes
  `analytics-web-srv`'s admin list a de-facto ingestion-key-admin list (see Security) — and add
  the new routes
  (`GET`/`POST /api/analytics-api-keys`, `POST /api/analytics-api-keys/import`,
  `DELETE /api/analytics-api-keys/{key_id}`, `GET`/`POST /api/ingestion-api-keys`,
  `DELETE /api/ingestion-api-keys/{key_id}`) to the "API Routes" list.
- `CHANGELOG.md`: `build/release.py` publishes an explicit crate list that does not include
  `analytics-web-srv`, so `analytics_keys_router`/`ingestion_keys_proxy_router` are never
  published API and get no bullet on that basis. `rust/public` (the published `micromegas`
  crate) does gain `POST /auth/api_keys/import` in `micromegas::servers::api_keys`, so the
  Unreleased section (which already carries #1383's key-store entries) gets new bullets for: the
  ingestion import route, the analytics-key mint/list/revoke/import routes and their admin pages,
  the `micromegas-import-keys` CLI entry point, and the new env vars
  (`MICROMEGAS_SQL_CONNECTION_STRING` read by `analytics-web-srv`,
  `MICROMEGAS_INGESTION_PROXY_OIDC_*`, `MICROMEGAS_INGESTION_ADMIN_URL`). This same doc step also
  **amends** the existing `## Unreleased` → `**Auth:**` bullet for #1383 (currently: "Three new
  OIDC-authenticated, admin-gated HTTP routes on the ingestion service — `POST`/`GET`/`DELETE
  /auth/api_keys` ... (analytics keys are never mintable over HTTP; see
  `mkdocs/docs/admin/api-keys.md` for the by-hand runbook)"), since this plan makes both clauses
  false in the same release it ships into and the by-hand runbook it points at is deleted by step
  20: drop the "analytics keys are never mintable over HTTP" parenthetical, add the fourth
  ingestion route (`POST /auth/api_keys/import`) to the route list, and repoint the runbook
  reference at the new analytics-key HTTP routes instead.

## Testing Strategy

- Rust: route-shape tests with a lazy pool (no live DB) for every new handler's validation/gating
  branches, per `firehose_tests.rs`'s precedent; `#[ignore]`d live-DB tests for the actual SQL,
  per `folders_tests.rs`'s precedent, run manually against a local Postgres. Named gating branches
  covered: 403 non-admin (the `AdminUser` extractor's rejection, not an in-handler `require_admin`
  call — see §1/§2), 400 empty `key`/`name`, `imported: false` idempotency, the fixed 503
  when `auth_disabled` is set (asserted even past a passing admin `AdminUser`/`ValidatedUser` and
  ahead of the pool/config check — steps 7 and 10),
  and the `NotConfigured` 503 for `AnalyticsKeysState { pool: None, .. }` /
  `IngestionProxyState { config: None, .. }` (neither ever touches the pool/makes an outbound
  call, so both are covered by the same no-live-DB harness — steps 7 and 10).
- Proxy: a loopback mock ingestion router asserting exact forwarding of method/path/query/body/
  `Content-Type` and response passthrough — these tests build `IngestionProxyConfig.credentials`
  from `Arc::new(TrivialRequestDecorator {})` rather than a real `OidcClientCredentialsDecorator`
  (see §2/step 10), so no second stub for the OIDC token fetch is needed — plus a "rejected before
  forwarding" assertion for non-admins, for `auth_disabled`, and for `config: None` (mock never hit
  in any of the three cases; these three need no credential injection since `forward` never reaches
  `.decorate()`).
- Python: `pytest` unit tests for `import_keys.py`'s per-key result classification (imported /
  already-present / errored) against a mocked `requests` session, per `test_logout.py`'s
  lightweight-mocking style; an end-to-end run importing a small test keyring into both tables
  and confirming the imported keys authenticate. **Not runnable via
  `local_test_env/ai_scripts/start_services.py` as-is**: it hardcodes `--disable-auth` for
  ingestion (`start_services.py:174`) and flight-sql (`:192`) in split mode, and picks
  `--disable-auth`/`--disable-ingestion-auth` based on OIDC config in monolith mode (`:290`) — in
  every one of those paths `api_keys_router` is never merged (`ingestion.rs:176`), so
  `POST /auth/api_keys/import` doesn't exist. This test instead requires a hand-launched ingestion
  (or monolith) with `MICROMEGAS_OIDC_CONFIG` set and neither `--disable-auth` nor
  `--disable-ingestion-auth` passed, plus the caller's OIDC identity present in
  `MICROMEGAS_ADMINS`/`MICROMEGAS_INGESTION_ADMINS` — not something `start_services.py` produces
  today; run it against a manually configured local instance instead.
- Frontend: `yarn test` for the two new pages (list render, mint form submit + one-time-key
  banner, revoke confirm flow), `yarn type-check`, `yarn lint`.
- Manual: run through the two new admin pages end-to-end against local services with
  `--disable-auth` off (OIDC required for `require_admin` to mean anything — with it on, per
  §4/Security, both route groups return a fixed 503 instead), confirming a non-admin OIDC session
  gets 403 on every new route. The ingestion-keys page additionally requires ingestion auth itself to be
  enabled (`--disable-ingestion-auth` must be off) — with it disabled, ingestion's
  `/auth/api_keys*` routes aren't mounted and the proxy surfaces the translated 404 described in
  §2. **`start_services.py` cannot reach this configuration in either mode** (see the Python
  bullet above for the exact flags it hardcodes) — this manual pass requires hand-launching
  ingestion/flight-sql/`analytics-web-srv` (or the monolith) with `MICROMEGAS_OIDC_CONFIG` set and
  both disable-auth flags off.
