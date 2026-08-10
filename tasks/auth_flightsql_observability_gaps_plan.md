# Auth & FlightSQL Observability Gaps Plan

GitHub issue: [#1459](https://github.com/madesroches/micromegas/issues/1459)

## Overview

Three related observability gaps make it harder to reconstruct "who did what from where"
during an auth-related investigation:

1. `analytics-web-srv`'s `/auth/*` routes (`login`, `callback`, `refresh`, `logout`, `me`) never
   get `client_ip`-tagged request/response logging, because the router that serves them is
   assembled and merged separately from the routers that do carry that middleware.
2. `analytics-web-srv`'s `[auth_success]` audit lines on login/refresh log only an opaque `sub`,
   even though the email claim is already sitting in the same JWT and is logged everywhere else
   in the module.
3. `flight-sql-srv`'s per-query audit trail (`QueryAuditRecord`, `execute_query`'s start-of-query
   log) carries rich client-reported attribution (`client`, `agent`, `user`, `email`, ...) but no
   network-level identifier — there's no peer IP anywhere in that trail.

All three are pure logging/observability changes: no behavior, request handling, or API surface
changes for any client.

## Current State

### 1. `/auth/*` routes bypass `observability_middleware`

`rust/analytics-web-srv/src/web_server.rs`:

- `build_protected_routes` (`:291-392`) and `build_protected_maps_blob_route` (`:394-422`) both
  end their router construction with `.layer(middleware::from_fn(observability_middleware))`
  (`:375`, `:405`) *before* branching on `auth_state` to add either `cookie_auth_middleware`
  (real auth) or the hardcoded anonymous `ValidatedUser` extensions (`--disable-auth`). Because
  `observability_middleware` is applied first and the auth-checking layer is applied after (Tower
  layers added later become the *outer* layer, so they run first on the request path),
  `cookie_auth_middleware` today actually runs before `observability_middleware` for those two
  routers — meaning a request rejected by `cookie_auth_middleware` is never logged with
  `client_ip` either. That's a pre-existing quirk of `build_protected_routes`, out of scope here
  (see Trade-offs), but it does *not* apply to `/auth/*`: none of `auth_login` /
  `auth_callback` / `auth_refresh` / `auth_logout` / `auth_me` sit behind `cookie_auth_middleware`
  — each does its own token handling internally and turns a failure into an `AuthApiError`
  response. So wrapping the whole `/auth/*` router in `observability_middleware` (regardless of
  where in the chain) is sufficient to log every request to it, success or failure.
- `build_auth_routes` (`:141-170`) builds its own `Router` (one branch per `auth_state.is_some()`)
  and never applies `observability_middleware` to either branch.
- `run_web_server` (`:654-665`) assembles the app as three independently-built routers merged
  together: `build_public_routes`, `build_protected_routes` (has the middleware), then
  `build_auth_routes` (does not). `build_protected_maps_blob_route` is merged in later (`:676-680`)
  and also has the middleware.
- `observability_middleware` itself (`rust/public/src/servers/axum_utils.rs:18-36`) logs a
  `request method=... uri=... client_ip=...` line before calling the handler and a
  `response status=... uri=... client_ip=...` line after — `client_ip` comes from
  `get_client_ip(&parts.headers, &parts.extensions)` (`rust/public/src/servers/http_utils.rs:11-40`),
  which checks `X-Forwarded-For`, then `X-Real-IP`, then falls back to the
  `ConnectInfo<SocketAddr>` extension axum populates via
  `into_make_service_with_connect_info::<SocketAddr>` (already wired up in `run_web_server`,
  `web_server.rs:701-703`).

### 2. `auth_callback`/`auth_refresh` log only `sub`, not `email`

`rust/analytics-web-srv/src/auth/handlers.rs`:

- `auth_callback` (`:104-235`) logs `info!("[auth_success] event=login sub={sub} issuer={}", ...)`
  at `:205-210`, gated on `extract_subject_from_token(&id_token)` returning `Some`.
- `auth_refresh` (`:239-342`) logs `info!("[auth_success] event=token_refresh sub={sub}")` at
  `:319-321`, same gating.
- `extract_subject_from_token` (`rust/analytics-web-srv/src/auth/claims.rs:98-111`) base64-decodes
  the JWT payload and pulls only `claims["sub"]`, discarding everything else — including `email`,
  which is present in the same payload because the OIDC client always requests the `email` scope
  (`handlers.rs:87`, `add_scope(Scope::new("email".to_string()))`).
- `cookie_auth_middleware` (`:454-510`) already logs both, at trace level, from the *validated*
  `AuthContext`: `trace!("[auth_success] subject={} email={:?} issuer={} admin={}",
  auth_context.subject, auth_context.email, auth_context.issuer, auth_context.is_admin);`
  (`:497-500`) — `email={:?}` is the exact `Option<String>` formatting convention to mirror.
- Neither `auth_callback` nor `auth_refresh` calls `auth_provider.validate_request()` on the
  freshly-exchanged `id_token` — they trust it as-is (same trust level `sub` extraction already
  has today: it's raw-decoded from the JWT payload, not signature-verified at that point; full
  JWKS validation happens later, per-request, in `cookie_auth_middleware`). So the natural
  minimal fix mirrors `sub`'s existing extraction, not `auth_me`'s validated-`AuthContext` path
  (which the issue references only to point out that `email` is *available*, not that this exact
  code path should be reused).

### 3. No peer IP anywhere in the FlightSQL query audit trail

`rust/public/src/servers/flight_sql_service_impl.rs`:

- `execute_query` (`:519-769`) takes `metadata: &MetadataMap` — not the full `tonic::Request`,
  so it has no access to connection-level info today. It reads `x-client-type`/`x-client-agent`/
  `x-client-entrypoint`/`x-client-session`/`x-client-notebook`/`x-client-cell` from `metadata`
  (`:571-585`) and logs them in the start-of-query `info!` (`:589-604`), and threads them into
  `QueryAuditState`/`QueryAuditRecord`. There is no `client_ip`-equivalent field anywhere in this
  path.
- The two call sites, `do_get_fallback` (`:786-795`) and `do_get_statement` (`:949-956`), each
  receive `request: Request<Ticket>` (or `Request<Ticket>` via the trait method) and currently
  only ever call `.metadata()` on it before delegating to `execute_query`.
- **The gRPC peer address *is* available, but not via `tonic::Request::remote_addr()`.** The
  server doesn't use a plain `tonic::transport::Server` TCP listener (which would populate the
  `TcpConnectInfo` extension `remote_addr()` looks for); it uses a custom `Connected` transport,
  `ConnectedIncoming`/`ConnectedStream` (`rust/public/src/servers/connect_info_layer.rs`), whose
  `Connected::ConnectInfo = SocketAddr` (`:55-61`). Tonic's own per-connection layer inserts that
  raw `SocketAddr` into every request's `http::Extensions` before any of our own tower layers or
  the generated `FlightServiceServer` dispatch run — but as a bare `SocketAddr`, not wrapped in
  `TcpConnectInfo`, so `Request::remote_addr()` (which specifically looks for `TcpConnectInfo`)
  returns `None` here. **Confirmed by tracing tonic 0.14.6's own source.**
- This is exactly the mechanism `get_client_ip`'s extension-fallback branch already targets
  (`rust/public/src/servers/http_utils.rs:35-37`, `extensions.get::<std::net::SocketAddr>()`), and
  it's already proven to work against this exact server: `LogUriService`
  (`rust/public/src/servers/log_uri_service.rs`), a tower layer wrapped around the whole
  `FlightServiceServer` in `flight_sql_server.rs:251-258`, calls
  `get_client_ip(request.headers(), request.extensions())` on every incoming gRPC call and logs
  `uri=... client_ip=...`. **So a form of client-IP logging already exists for FlightSQL today** —
  but it's a generic, per-RPC-call line logged before the request body is even decoded, with no
  `query_id`, SQL, or user attribution, and it never reaches the structured
  `flightsql_query_audit` record. It doesn't satisfy the issue's ask (correlatable, per-query
  attribution), but it does confirm the mechanism works and that `request.extensions()` still
  carries the `SocketAddr` all the way down to `execute_query`'s call sites (tonic's generated
  per-method dispatch preserves the underlying `http::Extensions`).
- `QueryAuditRecord` (`rust/public/src/servers/query_audit.rs:79-129`) is a plain
  `#[derive(serde::Serialize)]` struct with no client-IP field.
- Unrelated existing mechanism, **not wired up, out of scope**: the HTTP gateway
  (`rust/public/src/servers/http_gateway.rs`) already computes an `x-client-ip` gRPC metadata
  header from the *real* HTTP connection before forwarding a `/gateway/query` REST call to
  FlightSQL (`build_origin_metadata`, `:183-219`), specifically to avoid the gateway's own peer
  address (which is what a naive peer-address read would see for gateway-proxied traffic)
  overwriting the original caller's IP — and it explicitly blocks a caller from spoofing that
  same header directly (`blocked_headers: ["X-Client-IP"]`, `:64`). Nothing on the FlightSQL
  server reads `x-client-ip` today, so this value is computed and sent but currently dropped on
  the floor. See Trade-offs/Open Questions for why this plan doesn't wire it up.

## Design

### 1. Wrap `/auth/*` in a query-string-redacting observability middleware

**Redaction rule**: `/auth/callback` carries the OAuth authorization `code` and the signed
`state` (which embeds the PKCE `code_verifier`, see Current State §3/`oauth_state.rs`) in its
query string. `observability_middleware` logs `parts.uri` verbatim, and `http::Uri`'s `Display`
includes the query string, so reusing it unmodified on `/auth/*` would put a live auth code and
PKCE verifier into `log_entries`. Instead of the plain `observability_middleware`, `/auth/*` gets
its own copy that logs only `uri.path()` (dropping the query string entirely — no route under
`/auth/*` needs its query params for correlation; `client_ip`/method/status/duration are enough):

```rust
/// Like `observability_middleware`, but logs only the request path, never the query string --
/// `/auth/callback`'s query carries the OAuth authorization code and the PKCE verifier
/// (embedded in the signed `state` param) and must never be written to the telemetry log.
pub async fn auth_observability_middleware(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path().to_string();
    let client_ip = get_client_ip(&parts.headers, &parts.extensions);
    info!(
        "request method={} path={path} client_ip={client_ip}",
        parts.method
    );
    let begin_ticks = now();
    let response = next.run(Request::from_parts(parts, body)).await;
    let end_ticks = now();
    let duration = end_ticks - begin_ticks;
    imetric!("request_duration", "ticks", duration as u64);
    info!(
        "response status={} path={path} client_ip={client_ip}",
        response.status()
    );
    response
}
```

This lives next to `observability_middleware` in `rust/public/src/servers/axum_utils.rs` (both
are generic, reusable across servers, not `analytics-web-srv`-specific). `build_auth_routes`
(`web_server.rs:141-170`) gains one `.layer(middleware::from_fn(auth_observability_middleware))`
call on each branch's router:

```rust
fn build_auth_routes(base_path: &str, auth_state: &Option<AuthState>) -> Router {
    if let Some(state) = auth_state {
        Router::new()
            .route(&format!("{base_path}/auth/login"), get(crate::auth::auth_login))
            .route(&format!("{base_path}/auth/callback"), get(crate::auth::auth_callback))
            .route(&format!("{base_path}/auth/refresh"), post(crate::auth::auth_refresh))
            .route(&format!("{base_path}/auth/logout"), post(crate::auth::auth_logout))
            .route(&format!("{base_path}/auth/me"), get(crate::auth::auth_me))
            .with_state(state.clone())
            .layer(middleware::from_fn(auth_observability_middleware))
    } else {
        Router::new()
            .route(&format!("{base_path}/auth/me"), get(auth_me_no_auth))
            .route(&format!("{base_path}/auth/logout"), post(auth_logout_no_auth))
            .layer(middleware::from_fn(auth_observability_middleware))
    }
}
```

The `--disable-auth` branch gets it too, for consistency with every other router in this file —
harmless there since there's no real auth to investigate, but it keeps `/auth/*` request logging
uniform regardless of mode. No changes needed in `run_web_server`: the merge at `:665` picks up
the now-instrumented router automatically.

### 2. Extend raw-claim extraction to include `email`

`claims.rs`'s `extract_subject_from_token` is replaced by a small struct + function that decodes
the JWT payload once and pulls both fields, used by both `auth_callback` and `auth_refresh`. Both
are `pub` (not `pub(crate)`) and re-exported from `auth/mod.rs`, so the integration-test crate
`rust/analytics-web-srv/tests/auth_unit_tests.rs` (which only sees the crate's public API via
`analytics_web_srv::auth::{...}`) can reach them:

```rust
/// Claims read directly from an unverified JWT payload, for audit logging in
/// `auth_callback`/`auth_refresh` where the token has not yet been through JWKS
/// signature validation (that happens later, per-request, via `cookie_auth_middleware`) --
/// same trust level the old `sub`-only extraction already had.
pub struct AuditClaims {
    pub sub: Option<String>,
    pub email: Option<String>,
}

/// Extract the 'sub' and 'email' claims from a JWT payload for audit logging.
pub fn extract_audit_claims_from_token(token: &str) -> AuditClaims {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return AuditClaims { sub: None, email: None };
    }
    let claims: Option<serde_json::Value> = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    AuditClaims {
        sub: claims.as_ref().and_then(|c| c["sub"].as_str()).map(str::to_string),
        email: claims.as_ref().and_then(|c| c["email"].as_str()).map(str::to_string),
    }
}
```

`handlers.rs` call sites become:

```rust
// auth_callback (:204-210)
let audit_claims = extract_audit_claims_from_token(&id_token);
if let Some(sub) = &audit_claims.sub {
    info!(
        "[auth_success] event=login sub={sub} email={:?} issuer={}",
        audit_claims.email, state.config.issuer
    );
}
```

```rust
// auth_refresh (:318-321)
let audit_claims = extract_audit_claims_from_token(&id_token);
if let Some(sub) = &audit_claims.sub {
    info!("[auth_success] event=token_refresh sub={sub} email={:?}", audit_claims.email);
}
```

`email={:?}` (rendering `Some("alice@example.com")` / `None`) matches `cookie_auth_middleware`'s
existing convention exactly (`:497-500`), so the two success-path formats stay consistent.
`extract_name_from_token` is untouched (still used by `auth_me`, `:394`).

### 3. Add `client_ip` to the FlightSQL query audit trail

**New helper**, `rust/public/src/servers/http_utils.rs`, alongside `get_client_ip`:

```rust
/// Extracts the client IP from the gRPC connection's peer socket address.
///
/// Unlike `get_client_ip` (the HTTP/axum path, which also honors `X-Forwarded-For`/`X-Real-IP`
/// for requests behind a reverse proxy), FlightSQL's tonic transport has no such header-based
/// override applied at this layer -- the only source available at `execute_query`'s call sites
/// is the raw peer `SocketAddr` the tower `Connected` transport
/// (`connect_info_layer::ConnectedStream`) inserts into request extensions.
pub fn get_grpc_peer_ip(extensions: &http::Extensions) -> String {
    extensions
        .get::<std::net::SocketAddr>()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
```

**Call sites** (`flight_sql_service_impl.rs`), resolved once per RPC before the request is
consumed, then threaded into `execute_query`:

```rust
async fn do_get_fallback(&self, request: Request<Ticket>, _message: Any) -> ... {
    let client_ip = get_grpc_peer_ip(request.extensions());
    let ticket_stmt = TicketStatementQuery::decode(request.get_ref().ticket.clone())
        .map_err(|e| status!("Could not read ticket", e))?;
    self.execute_query(ticket_stmt, request.metadata(), &client_ip).await
}
```

```rust
async fn do_get_statement(&self, ticket: TicketStatementQuery, request: Request<Ticket>) -> ... {
    let client_ip = get_grpc_peer_ip(request.extensions());
    self.execute_query(ticket, request.metadata(), &client_ip).await
}
```

**`execute_query`** gains a `client_ip: &str` parameter (`:519`), used in:
- Both start-of-query `info!` lines (`:589-604`): add `client_ip={client_ip}`.
- `QueryAuditState` (`:262-297`): new `client_ip: String` field, populated at construction
  (`:619-643`) from the parameter.
- `QueryAuditState::emit` (`:309-357`): copies `client_ip` into the `QueryAuditRecord` literal.

**`QueryAuditRecord`** (`query_audit.rs:79-129`) gains:

```rust
pub client_ip: String,
```

placed right after `query_id` — the peer address is the one field in this record that comes from
the network layer rather than from client-controlled attribution headers, so it's grouped with
the record's other "how do I trust/correlate this" identifier rather than with `client`/`agent`/
`entrypoint` (which are all self-reported and spoofable). Always present (never `Option`), same
convention as `client`: `"unknown"` on the (practically unreachable, since every accepted
connection has a peer address) case where the extension is missing, mirroring `get_client_ip`'s
own fallback string.

## Implementation Steps

**Phase 1 — `analytics-web-srv`**
1. `rust/public/src/servers/axum_utils.rs`: add `auth_observability_middleware` (path-only
   logging, no query string), per Design §1. `web_server.rs::build_auth_routes` (`:141-170`): add
   `.layer(middleware::from_fn(auth_observability_middleware))` to both branches.
2. `claims.rs`: replace `extract_subject_from_token` (`:98-111`) with `pub` `AuditClaims` +
   `pub fn extract_audit_claims_from_token`, per Design §2.
3. `handlers.rs`: update the `use super::claims::{...}` import (`:3-6`, drop
   `extract_subject_from_token`, add `extract_audit_claims_from_token`); update `auth_callback`'s
   login-success log (`:204-210`) and `auth_refresh`'s refresh-success log (`:318-321`) per
   Design §2.
4. `auth/mod.rs` (`:31`): add `AuditClaims, extract_audit_claims_from_token` to the
   `pub use claims::{...}` line so the new test file can reach them through the crate's public
   API.

**Phase 2 — `analytics-web-srv` tests**
5. New test in `rust/analytics-web-srv/tests/auth_unit_tests.rs`: `extract_audit_claims_from_token`
   with a hand-built unsigned JWT (`base64url(header).base64url(payload).base64url(sig)`, payload
   `{"sub": "user-123", "email": "alice@example.com"}`) asserts both fields populate; a payload
   with no `email` key asserts `email: None`, `sub: Some(...)`; a malformed token (not 3
   dot-separated parts, or non-base64/non-JSON payload) asserts `AuditClaims { sub: None, email: None }`.
6. No automated test asserts the *rendered log line content* for steps 1/3 — this codebase has no
   log-content-capture test harness wired up for `analytics-web-srv` (see Testing Strategy);
   covered by manual verification instead. `cargo test -p analytics-web-srv` (existing suite)
   must still pass unchanged, confirming no routing regression from step 1.

**Phase 3 — `flight-sql-srv`**
7. `http_utils.rs`: add `get_grpc_peer_ip`, per Design §3.
8. `flight_sql_service_impl.rs`: thread `client_ip` through `do_get_fallback` (`:786-795`),
   `do_get_statement` (`:949-956`), `execute_query`'s signature and both `info!` lines
   (`:519-604`), `QueryAuditState` (`:262-297`, `:619-643`), and `QueryAuditState::emit`
   (`:309-357`), per Design §3.
9. `query_audit.rs`: add `pub client_ip: String` to `QueryAuditRecord` (`:79-129`), placed right
   after `query_id`.

**Phase 4 — `flight-sql-srv` tests**
10. New `rust/public/tests/http_utils_tests.rs`: unit tests for `get_grpc_peer_ip` — a
    `SocketAddr` extension present returns its IP (string, no port); no extension present returns
    `"unknown"`.
11. `rust/public/Cargo.toml`: register the new test file with an explicit `[[test]]` entry
    (`name = "http_utils_tests"`, `path = "tests/http_utils_tests.rs"`,
    `required-features = ["server"]`), matching every other file under `rust/public/tests/` —
    without it, the new test is auto-discovered without the `server` feature gate and fails to
    compile on a plain `cargo test -p micromegas`.
12. `rust/public/tests/query_audit_tests.rs`: add `client_ip: "203.0.113.7".to_string()` to
    `full_record` (`:187-219`) and to the literal in `query_audit_record_omits_absent_optionals`
    (`:257-287`, e.g. `"unknown".to_string()`, since that test's other always-present fields use
    the "server saw nothing distinctive" placeholder), and assert
    `value["client_ip"] == "203.0.113.7"` / `"unknown"` respectively in each test.

**Phase 5 — docs**
13. `mkdocs/docs/query-guide/query-audit-log.md`: add a `client_ip` row to the `## Fields` table
    (always present, right after `query_id`), describing it as the gRPC peer address (network
    truth, not self-reported) and noting the caveat from Trade-offs: for FlightSQL calls proxied
    through a server-side hop — the HTTP gateway's `/gateway/query` or `analytics-web-srv`'s
    `/api/query-stream` (the web app's notebook/query-editor path) — this reports the proxy's own
    address, not the original browser/HTTP caller's.
14. `CHANGELOG.md`: `## Unreleased` → **Analytics:** entry for the `client_ip` addition
    (**minor breaking change**: `QueryAuditRecord` is published API and gains a field, matching
    the convention every prior addition to this struct used — e.g. #1436, #1437, #1406); **Auth:**
    entry (or a new bullet under the existing Auth section) for the `/auth/*` `client_ip` logging
    fix and the `email` addition to `[auth_success]` lines.

## Files to Modify

- `rust/public/src/servers/axum_utils.rs` — new `auth_observability_middleware`.
- `rust/analytics-web-srv/src/web_server.rs` — `build_auth_routes`.
- `rust/analytics-web-srv/src/auth/claims.rs` — `extract_subject_from_token` → `AuditClaims`/`extract_audit_claims_from_token`.
- `rust/analytics-web-srv/src/auth/handlers.rs` — imports, `auth_callback`, `auth_refresh`.
- `rust/analytics-web-srv/src/auth/mod.rs` — re-export `AuditClaims`, `extract_audit_claims_from_token`.
- `rust/analytics-web-srv/tests/auth_unit_tests.rs` — new `extract_audit_claims_from_token` cases.
- `rust/public/src/servers/http_utils.rs` — new `get_grpc_peer_ip`.
- `rust/public/src/servers/flight_sql_service_impl.rs` — `do_get_fallback`, `do_get_statement`,
  `execute_query`, `QueryAuditState`, `QueryAuditState::emit`.
- `rust/public/src/servers/query_audit.rs` — `QueryAuditRecord`.
- `rust/public/tests/http_utils_tests.rs` — new.
- `rust/public/Cargo.toml` — new `[[test]]` entry for `http_utils_tests`.
- `rust/public/tests/query_audit_tests.rs` — fixture updates.
- `mkdocs/docs/query-guide/query-audit-log.md` — `## Fields` table.
- `CHANGELOG.md` — `## Unreleased` entries.

## Trade-offs

- **`build_protected_routes`'s pre-existing auth-before-observability ordering is left alone.**
  As noted in Current State, `cookie_auth_middleware` runs *before* `observability_middleware`
  for `/api/*` routes today, so a rejected (401) request to a protected route currently isn't
  logged with `client_ip` either. Fixing that would mean reordering `.layer()` calls in
  `build_protected_routes`/`build_protected_maps_blob_route`, a behavior change to routes the
  issue doesn't mention and that carries its own risk (e.g. whether `cookie_auth_middleware`'s
  own `warn!("[auth_failure] ...")` lines are expected to fire before or after the generic
  request/response log). Left as a separate, pre-existing gap — worth its own issue if wanted.
- **Peer IP, not a proxy-forwarded client-IP header.** The HTTP gateway already computes and
  forwards `x-client-ip` gRPC metadata (`http_gateway.rs:209-216`) specifically so gateway-proxied
  queries could report the *original* caller's IP instead of the gateway's own; there's no
  equivalent today on `analytics-web-srv`'s `/api/query-stream` path (`stream_query.rs:271-282`),
  which builds its `BearerFlightSQLClientFactory` with only `x-client-notebook`/`x-client-cell`
  metadata and forwards no client-IP information at all. This plan doesn't wire up either as a
  trusted source of `client_ip` on the FlightSQL server side, for two reasons: (1) any such value
  is self-reported gRPC metadata like any other header, and unlike `x-client-type`/`x-client-agent`,
  trusting it blindly would let *any* direct gRPC caller (bypassing the proxy entirely, e.g. the
  Python client talking straight to `flight-sql-srv`) spoof an arbitrary IP into an audit trail
  whose whole purpose is "who did what from where" — the gateway's own spoofing protection
  (blocking a client from setting this header on its *inbound* HTTP request) only holds for
  traffic that actually goes through the gateway, and `analytics-web-srv` has no such protection
  at all; (2) safely trusting it would require the FlightSQL server to first establish that the
  immediate gRPC peer *is* the proxy (e.g. mTLS service identity, or a network-topology
  assumption), which is a real design decision this issue doesn't ask for and shouldn't be bundled
  into a straightforward logging-gap fix. This plan's `client_ip` is therefore the literal,
  non-spoofable gRPC peer address: correct and trustworthy for direct clients, but reports the
  proxy's own address — the HTTP gateway's for `/gateway/query`, or `analytics-web-srv`'s for
  every notebook/query-editor query submitted through the web app (`client="web"`, by far the
  highest-volume source of audit records) — for anything proxied. Flagged as a known limitation in
  the doc update (step 13) and as an Open Question below.
- **`AuditClaims` reads unverified claims, same as the `sub`-only extraction it replaces.**
  `auth_callback`/`auth_refresh` don't run the freshly-exchanged `id_token` through
  `auth_provider.validate_request()` before logging — they trust the raw JWT payload from the
  token endpoint's own response (reached over TLS directly), same as today. This plan doesn't
  change that trust boundary; it only extracts one more field from the same already-trusted
  payload.
- **No `x-forwarded-for`-style override for the gRPC peer IP.** `get_client_ip` (HTTP path) checks
  `X-Forwarded-For`/`X-Real-IP` headers before falling back to the connection's `SocketAddr`,
  because HTTP traffic commonly passes through a reverse proxy that sets those headers. FlightSQL
  gRPC traffic has no equivalent header convention in this codebase (the closest is the gateway's
  `x-client-ip`, addressed above) — `get_grpc_peer_ip` only ever reads the extension, and doesn't
  need `MetadataMap`-to-`HeaderMap` conversion machinery it would otherwise require to also check
  headers.

## Documentation

- `mkdocs/docs/query-guide/query-audit-log.md` — `## Fields` table (`client_ip` row) and a Notes
  bullet on the proxy-hop caveat (HTTP gateway *and* `analytics-web-srv`'s `/api/query-stream`).
- `CHANGELOG.md` — `## Unreleased` entries under **Analytics:** and **Auth:**.

## Testing Strategy

1. `cargo fmt` and `cargo clippy --workspace -- -D warnings` (per `rust/CLAUDE.md`).
2. `cargo test -p analytics-web-srv` — existing suite plus the new `extract_audit_claims_from_token`
   cases (step 5); confirms no routing regression from wrapping `/auth/*` in
   `auth_observability_middleware`.
3. `cargo test -p micromegas --features server` — covers the updated `query_audit_tests.rs` and
   new `http_utils_tests.rs`.
4. **Manual verification** (this repo has no log-content-capture test harness for
   `analytics-web-srv`/`flight-sql-srv` — see `python3 local_test_env/ai_scripts/start_services.py`):
   - Start services, hit `GET /auth/login` (or trigger any `/auth/*` request) and `tail -f
     /tmp/analytics.log` (or the monolith log), confirming a `request method=GET
     uri=.../auth/login client_ip=...` / `response status=... client_ip=...` pair now appears —
     it did not before this change.
   - Complete a real OIDC login flow and confirm the `[auth_success] event=login sub=... email=...
     issuer=...` line now includes a real email address, not just `sub`. Trigger a token refresh
     (or wait for one) and confirm `[auth_success] event=token_refresh sub=... email=...` likewise.
   - Run `micromegas-query "SELECT 1" --all` directly against `flight-sql-srv` and confirm the
     `execute_query ...` start-of-query line now includes `client_ip=<your real IP>`; query
     `flightsql_query_audit` (per `query-audit-log.md`'s pattern) and confirm the JSON record has
     `"client_ip"` set to the same value.
   - Run a query from the web app's notebook/query editor (`/api/query-stream`) and confirm
     `client_ip` in the resulting audit record is `analytics-web-srv`'s own address, not the
     browser's — this is the highest-volume (`client="web"`) case of the proxy caveat. If a
     gateway deployment is available, also run the same query through `/gateway/query` and confirm
     `client_ip` is the *gateway's* address, not the original HTTP caller's — together confirming
     the documented caveat (step 13) matches actual behavior for both proxy hops.

## Open Questions

- Should a follow-up wire up the gateway's already-computed `x-client-ip` metadata header into
  `QueryAuditRecord` (e.g. a second field, `gateway_client_ip`, populated only when a gateway is
  known to be the trusted intermediary), so gateway-proxied queries also get the original caller's
  IP? This plan deliberately leaves that as future work — see Trade-offs.
