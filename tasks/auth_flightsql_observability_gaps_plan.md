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
   network-level identifier — there's no client IP anywhere in that trail.

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
  which checks `X-Forwarded-For` (today taking its *leftmost* entry — Design §3 changes that to
  the rightmost), then `X-Real-IP`, then falls back to the
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

### 3. No client IP anywhere in the FlightSQL query audit trail

**Deployment context: every service sits behind an AWS ALB (layer 7).** In-repo evidence:
`flight_sql_server.rs`'s optional health sidecar (`with_health_addr`, `:168-175`) exists to
"[enable] plain-HTTP ALB health checks without changing the gRPC protocol";
`mkdocs/docs/admin/flight-sql.md:86-87` documents FlightSQL scaling "horizontally behind a
gRPC-aware load balancer"; `mkdocs/docs/gateway/index.md:62` documents the gateway's liveness
endpoint as being "for load balancer probes (e.g. AWS ALB)". The ALB **appends** the address it
observed to `X-Forwarded-For` — it does not overwrite (default
`routing.http.xff_header_processing.mode = append`). Three consequences for the shared
`get_client_ip` (`rust/public/src/servers/http_utils.rs:11-40`):

- It returns the **leftmost** `X-Forwarded-For` entry today (`:14-19`,
  `value.split(',').next()`), and its doc comment asserts "leftmost IP is the original client
  when behind proxies" (`:6`). With an *appending* proxy that is wrong and unsafe: everything to
  the left of the ALB's own appended entry is whatever the caller sent, so any caller can prepend
  an arbitrary address and have it win. Today's `client_ip` is therefore fully spoofable on every
  route that logs it.
- The **rightmost** entry is the address the ALB itself observed on the connection it accepted.
  A caller cannot forge it — whatever the caller writes lands to the *left* of the ALB's own
  observation. With exactly one trusted proxy hop (the ALB), rightmost is the correct read.
- The raw peer `SocketAddr` fallback (`:35-37`) is, behind the ALB, always the ALB's own address —
  never the client's. A peer-address-only design would report nothing useful in the deployed
  topology, and is only meaningful for direct (e.g. local-dev) connections.

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
- **The gRPC peer address *is* available as a fallback, but not via
  `tonic::Request::remote_addr()`.** The
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
  per-method dispatch preserves the underlying `http::Extensions`). Because this plan fixes
  `get_client_ip` itself rather than adding a second client-IP implementation (Design §3),
  `LogUriService`'s `client_ip` and the new audit-record `client_ip` are produced by the same
  function from the same request and always agree — the only difference is granularity
  (per-RPC-call line vs. per-query structured record). `LogUriService` also gains the
  spoofing fix for free.
- `QueryAuditRecord` (`rust/public/src/servers/query_audit.rs:79-129`) is a plain
  `#[derive(serde::Serialize)]` struct with no client-IP field.
- Existing adjacent mechanism, **not wired up, out of scope**: the HTTP gateway
  (`rust/public/src/servers/http_gateway.rs`) already computes an `x-client-ip` gRPC metadata
  header from its *own* inbound HTTP request before forwarding a `/gateway/query` REST call to
  FlightSQL (`build_origin_metadata`, `:183-219`), specifically so the gateway's own address
  (which is what the FlightSQL side sees for gateway-proxied traffic) doesn't stand in for the
  original caller's IP — and it explicitly blocks a caller from spoofing that same header
  directly (`blocked_headers: ["X-Client-IP"]`, `:60-66`). That value is itself computed by
  `get_client_ip` (`:213`), so it inherits this plan's fix. Nothing on the FlightSQL server reads
  `x-client-ip` today, so it is computed and sent but currently dropped on the floor.
  Separately, the gateway's header forwarding is allow-list based (`should_forward`, `:79-104`)
  and `x-forwarded-for` is **not** on the allow-list (`:44-57`), so the gateway does not pass the
  original caller's XFF chain through to FlightSQL either. See Trade-offs/Open Questions.

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
/// `client_ip` here reuses the shared `get_client_ip` (rightmost `X-Forwarded-For` entry, then
/// `X-Real-IP`, then the socket address), same as every other route in this codebase -- see
/// Design 3 and Trade-offs.
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

**One client-IP implementation, fixed in place.** The only reason this codebase resolves a client
IP is logging, so there is no case for a second implementation with different semantics. Instead
of adding a gRPC-specific peer-address helper, this plan changes the shared `get_client_ip`
(`rust/public/src/servers/http_utils.rs:11-40`) to read the **rightmost** `X-Forwarded-For` entry,
and then calls that same function — unchanged — from the FlightSQL path:

```rust
/// Extracts the client IP address from HTTP headers and extensions.
///
/// This function checks sources in order of priority:
/// 1. X-Forwarded-For (rightmost entry of the last header field line -- the address the nearest
///    trusted proxy observed)
/// 2. X-Real-IP (used by some proxies like nginx)
/// 3. Socket address from extensions (direct connection)
///
/// The *rightmost* `X-Forwarded-For` entry is used, not the leftmost, because the AWS ALB every
/// service is deployed behind *appends* the address it observed rather than overwriting the
/// header (`routing.http.xff_header_processing.mode = append`, the ALB default). Every entry to
/// the left of the last one is caller-supplied and therefore spoofable; the last entry is the
/// ALB's own observation and cannot be forged by the caller -- this holds even if the caller sends
/// its own `X-Forwarded-For` as a *separate* header field line, since `HeaderMap::get` returns
/// only the first such line and would surface a fully caller-chosen value; `get_all(...).last()`
/// is required to reach the ALB's line. This is correct for exactly one trusted proxy hop --
/// putting a second trusted proxy in front of the ALB would mean skipping one more entry from the
/// right.
///
/// Returns "unknown" if no IP can be extracted.
pub fn get_client_ip(headers: &http::HeaderMap, extensions: &http::Extensions) -> String {
    // Check X-Forwarded-For header first (for load balancers/proxies).
    // `get_all(...).last()` -- not `get(...)`, which returns only the *first* field line -- picks
    // the last field line, since the ALB appends its own observation as (or onto) the last line;
    // within that line, the rightmost comma-separated entry is what the ALB itself observed.
    // Everything else (earlier lines in full, and earlier entries within the last line) is
    // caller-supplied and spoofable.
    if let Some(forwarded_for) = headers.get_all("x-forwarded-for").iter().last()
        && let Ok(value) = forwarded_for.to_str()
        && let Some(client_ip) = value.rsplit(',').next()
        && !client_ip.trim().is_empty()
    {
        return client_ip.trim().to_string();
    }

    // ... X-Real-IP, ConnectInfo<SocketAddr>, bare SocketAddr and "unknown" branches unchanged
}
```

Three details in that first branch: `get_all("x-forwarded-for").iter().last()` selects the last
header field line (`headers.get` would silently return only the *first* line, letting a caller who
sends its own separate `X-Forwarded-For` line win outright); `rsplit(',').next()` then takes the
rightmost comma-separated element *of that last line* (mirroring the existing `split(',').next()`
for the leftmost); and the added `!client_ip.trim().is_empty()` guard makes a present-but-empty
`X-Forwarded-For` fall through to `X-Real-IP`/the socket address instead of returning `""` as it
does today.

**Call sites** (`flight_sql_service_impl.rs`), resolved once per RPC before the request is
consumed, then threaded into `execute_query`. Getting the two arguments out of a
`tonic::Request` is a plain borrow — no conversion, no allocation (verified against the vendored
tonic 0.14.6 source):

- `tonic::metadata::MetadataMap` wraps a private `http::HeaderMap` and exposes it via
  `impl AsRef<http::HeaderMap> for MetadataMap` (`src/metadata/map.rs:41-45`), so
  `request.metadata().as_ref()` yields exactly the `&http::HeaderMap` `get_client_ip` takes.
  (`MetadataMap::into_headers() -> http::HeaderMap` also exists, `map.rs:261`, but consumes the
  map and so is unusable from a `&Request`; `AsRef` is the right mechanism here.)
- `tonic::Request::extensions()` returns `&Extensions` where `tonic::Extensions` is a re-export
  of `http::Extensions` (`src/lib.rs:126`, `pub use http::Extensions;`, imported as such in
  `src/request.rs:6`) — the same type, passed straight through.

Since tonic surfaces HTTP/2 headers as gRPC metadata, an `x-forwarded-for` appended by the ALB in
front of `flight-sql-srv` is readable there; for a direct connection (local dev, in-cluster
peer) the header is absent and the existing `SocketAddr` fallback applies.

```rust
async fn do_get_fallback(&self, request: Request<Ticket>, _message: Any) -> ... {
    let client_ip = get_client_ip(request.metadata().as_ref(), request.extensions());
    let ticket_stmt = TicketStatementQuery::decode(request.get_ref().ticket.clone())
        .map_err(|e| status!("Could not read ticket", e))?;
    self.execute_query(ticket_stmt, request.metadata(), &client_ip).await
}
```

```rust
async fn do_get_statement(&self, ticket: TicketStatementQuery, request: Request<Ticket>) -> ... {
    let client_ip = get_client_ip(request.metadata().as_ref(), request.extensions());
    self.execute_query(ticket, request.metadata(), &client_ip).await
}
```

`flight_sql_service_impl.rs` gains `use super::http_utils::get_client_ip;` (it imports nothing
from `http_utils` today).

**`execute_query`** gains a `client_ip: &str` parameter (`:519`), used in:
- Both start-of-query `info!` lines (`:589-604`): add `client_ip={client_ip}`.
- `QueryAuditState` (`:262-297`): new `client_ip: String` field, populated at construction
  (`:619-643`) from the parameter.
- `QueryAuditState::emit` (`:309-357`): copies `client_ip` into the `QueryAuditRecord` literal.

**`QueryAuditRecord`** (`query_audit.rs:79-129`) gains:

```rust
pub client_ip: String,
```

placed right after `query_id` — it's the one field in this record that comes from the network /
trusted-proxy layer rather than from client-controlled attribution headers, so it's grouped with
the record's other "how do I trust/correlate this" identifier rather than with `client`/`agent`/
`entrypoint` (which are all self-reported and spoofable). Always present (never `Option`), same
convention as `client`: `"unknown"` in the (practically unreachable) case where neither a header
nor a peer address is available — `get_client_ip`'s own fallback string, returned verbatim.

**Blast radius of the `get_client_ip` change.** It has exactly three callers today, and none of
them break:

- `rust/public/src/servers/axum_utils.rs:21` (`observability_middleware`) — logs `client_ip` on
  `/api/*` and `/gateway/*` routes. Same type, same "unknown" fallback; the value logged simply
  becomes the ALB-observed address instead of a caller-chosen one. Behavior change, no API change.
- `rust/public/src/servers/log_uri_service.rs:29` — same, for the per-RPC FlightSQL line. Also the
  reason it now agrees with the new audit-record field (Current State §3).
- `rust/public/src/servers/http_gateway.rs:213` (`build_origin_metadata`) — computes the
  `x-client-ip` metadata header the gateway forwards to FlightSQL. It constructs a fresh
  `http::Extensions` holding `ConnectInfo(*addr)` and passes the *inbound* HTTP `headers`, so the
  change makes that header non-spoofable in the same way. The gateway's existing anti-spoofing
  guarantee (`blocked_headers: ["X-Client-IP"]`) is unaffected — that blocks a different header.

No existing test asserts leftmost `X-Forwarded-For` selection. The only tests that assert on a
`client_ip` value are three `build_origin_metadata` cases in
`rust/public/tests/http_gateway_tests.rs` — `test_build_origin_metadata_with_client_type`
(`:73-97`), `test_build_origin_metadata_without_client_type` (`:99-122`) and
`test_build_origin_metadata_ignores_client_ip_header` (`:152-167`) — and none of them sets an
`X-Forwarded-For` (or `X-Real-IP`) header at all, so all three exercise the
`ConnectInfo<SocketAddr>` fallback and pass unchanged. (The last one asserts that an inbound
`x-client-ip: 1.2.3.4` header does not become the computed `client_ip`, which stays true: nothing
in `get_client_ip` reads `x-client-ip`.) `get_client_ip` has no direct unit tests today; step 10
adds them.

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
6. New test, `rust/public/tests/auth_observability_tests.rs` — the middleware under test
   (`auth_observability_middleware`) lives in the `micromegas` crate (`rust/public`), so its test
   belongs there too, per `rust/CLAUDE.md`'s "unit tests ... under the tests folder of the crate":
   drive a request whose path carries a `code`/`state` query string through
   `auth_observability_middleware` under `micromegas_tracing::test_utils::init_in_memory_tracing()`
   (the same `InMemorySink`/`flush_log_buffer` harness already used elsewhere in this workspace,
   e.g. `rust/analytics/tests/log_tests.rs`; imported directly as `micromegas_tracing::...` since
   `rust/public/Cargo.toml` already depends on `micromegas-tracing` directly, not just through the
   `micromegas` facade), then inspect the captured log blocks and assert the `request`/`response`
   lines contain the path but never the query string. `rust/public/Cargo.toml` gains a
   `serial_test` dev-dependency (alphabetical, between `reqwest` and `tokio`) — `init_in_memory_tracing`
   requires it — and a `[[test]]` entry (`name = "auth_observability_tests"`,
   `path = "tests/auth_observability_tests.rs"`, `required-features = ["server"]`), matching every
   other file under `rust/public/tests/`. `cargo test -p micromegas --features server` (existing
   suite plus this new test) must still pass, confirming no routing regression from step 1.

**Phase 3 — `flight-sql-srv`**
7. `http_utils.rs`: change `get_client_ip` (`:11-40`) to select the **rightmost** entry of the
   **last** `X-Forwarded-For` header field line (`headers.get_all(...).iter().last()` +
   `rsplit` + non-empty guard — not `headers.get(...)`, which would return only the first field
   line and let a caller-sent separate line win) and rewrite its doc comment (`:3-10`, and the
   inline comment at `:12-13`), which currently claims the leftmost entry is the original client.
   Per Design §3. This is a behavior change for all three existing callers — see Design §3's
   blast-radius note; no signature or type changes, so no caller edits are needed.
8. `flight_sql_service_impl.rs`: add `use super::http_utils::get_client_ip;` and thread `client_ip`
   through `do_get_fallback` (`:786-795`, `get_client_ip(request.metadata().as_ref(),
   request.extensions())`), `do_get_statement` (`:949-956`, same), `execute_query`'s signature and
   both `info!` lines (`:519-604`), `QueryAuditState` (`:262-297`, `:619-643`), and
   `QueryAuditState::emit` (`:309-357`), per Design §3.
9. `query_audit.rs`: add `pub client_ip: String` to `QueryAuditRecord` (`:79-129`), placed right
   after `query_id`.

**Phase 4 — `flight-sql-srv` tests**
10. New `rust/public/tests/http_utils_tests.rs`: unit tests for `get_client_ip`'s selection rules —
    (a) a multi-entry `X-Forwarded-For` chain (`"1.2.3.4, 10.0.0.1, 198.51.100.9"`) returns the
    rightmost entry, whitespace-trimmed; (b) a single-entry chain returns that entry; (c) the
    spoofing case: a client-prepended value plus the ALB's appended observation
    (`"666.spoof, 198.51.100.9"` or `"203.0.113.1, 198.51.100.9"`) returns the ALB's entry, not
    the client's — the client-prepended value must be ignored; (d) **two separate
    `X-Forwarded-For` header field lines** (via `HeaderMap::append`, e.g. a caller-sent
    `"666.spoof"` line followed by the ALB's own appended `"198.51.100.9"` line) returns the ALB's
    entry from the *last* line, not the caller's line — this is the case `headers.get(...)` alone
    would get wrong (it would return the caller's line outright); (e) no `X-Forwarded-For` but an
    `X-Real-IP` header returns the `X-Real-IP` value; (f) neither header but a
    `ConnectInfo<SocketAddr>` / bare `SocketAddr` extension returns its IP (string, no port); (g)
    no header and no extension returns `"unknown"`. Optionally (h) a present-but-empty
    `X-Forwarded-For` falls through to the next source rather than returning `""`. The test builds
    `http::HeaderMap`/`http::Extensions` (and `axum::extract::ConnectInfo`) directly — both crates
    are already reachable from this package's tests under the `server` feature, as
    `http_gateway_tests.rs:1` shows for `http`.
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
    (always present, right after `query_id`, i.e. after `:126`), describing it as network-level
    truth rather than self-reported attribution — the rightmost `X-Forwarded-For` entry (the
    address the ALB in front of the service observed) falling back to the gRPC peer address for
    direct connections — and noting the caveat from Trade-offs: for FlightSQL calls proxied
    through a server-side hop — the HTTP gateway's `/gateway/query` or `analytics-web-srv`'s
    `/api/query-stream` (the web app's notebook/query-editor path) — this reports the proxy's own
    address, not the original browser/HTTP caller's, because neither proxy forwards the caller's
    `X-Forwarded-For` chain. Also note it matches the `client_ip` on `flight-sql-srv`'s generic
    per-call `uri=... client_ip=...` line (both come from the same `get_client_ip`), so the two can
    be cross-referenced.
14. `mkdocs/docs/gateway/index.md`: the "Client IP Security" bullet at `:342` currently reads
    "Uses real socket address or `X-Forwarded-For` (from trusted proxies)", which becomes stale
    once the leftmost read is gone. Restate as: uses the rightmost `X-Forwarded-For` entry (the
    address the trusted proxy/ALB observed, which a caller cannot forge), falling back to
    `X-Real-IP` and then the real socket address. The lead-in at `:339` ("The gateway always
    extracts client IP from the actual connection") is also loose — headers are consulted first —
    and should say "from the connection, or from the trusted proxy that observed it". The
    neighbouring bullets (`:341` blocks `x-client-ip` from clients, `:343` "Prevents IP spoofing in
    audit logs") stay correct and are in fact only now accurate.
    `mkdocs/docs/gateway/configuration.md:51` and
    `mkdocs/docs/gateway/index.md:128` ("Real client IP (prevents spoofing)") need no change.
15. `CHANGELOG.md`: `## Unreleased` → **Analytics:** entry for the `client_ip` addition
    (**minor breaking change**: `QueryAuditRecord` is published API and gains a field, matching
    the convention every prior addition to this struct used — e.g. #1436, #1437, #1406); **Auth:**
    entry (or a new bullet under the existing Auth section) for the `/auth/*` `client_ip` logging
    fix and the `email` addition to `[auth_success]` lines. The `get_client_ip` change must be
    called out as a **behavior change for every existing `client_ip` logger**, not just an
    addition: `observability_middleware` (`/api/*`, `/gateway/*` request/response lines),
    `LogUriService` (FlightSQL per-RPC lines) and the gateway's forwarded `x-client-ip` metadata
    header all switch from the leftmost `X-Forwarded-For` entry (caller-supplied, spoofable) to the
    rightmost (ALB-observed, non-forgeable). Deployments *not* behind exactly one appending proxy
    will see different values than before — including anyone who was relying on the old leftmost
    read behind an overwriting proxy.

## Files to Modify

- `rust/public/src/servers/axum_utils.rs` — new `auth_observability_middleware`.
- `rust/analytics-web-srv/src/web_server.rs` — `build_auth_routes`.
- `rust/analytics-web-srv/src/auth/claims.rs` — `extract_subject_from_token` → `AuditClaims`/`extract_audit_claims_from_token`.
- `rust/analytics-web-srv/src/auth/handlers.rs` — imports, `auth_callback`, `auth_refresh`.
- `rust/analytics-web-srv/src/auth/mod.rs` — re-export `AuditClaims`, `extract_audit_claims_from_token`.
- `rust/analytics-web-srv/tests/auth_unit_tests.rs` — new `extract_audit_claims_from_token` cases.
- `rust/public/tests/auth_observability_tests.rs` — new; asserts query-string redaction
  in `auth_observability_middleware`'s log lines.
- `rust/public/src/servers/http_utils.rs` — `get_client_ip` switches to the rightmost
  `X-Forwarded-For` entry (+ doc comment). No signature change; no edits needed at its three
  existing call sites (`axum_utils.rs:21`, `log_uri_service.rs:29`, `http_gateway.rs:213`).
- `rust/public/src/servers/flight_sql_service_impl.rs` — `do_get_fallback`, `do_get_statement`,
  `execute_query`, `QueryAuditState`, `QueryAuditState::emit`.
- `rust/public/src/servers/query_audit.rs` — `QueryAuditRecord`.
- `rust/public/tests/http_utils_tests.rs` — new; `get_client_ip` selection/fallback/spoofing cases.
- `rust/public/Cargo.toml` — `serial_test` dev-dependency; new `[[test]]` entries for
  `auth_observability_tests` and `http_utils_tests`.
- `rust/public/tests/query_audit_tests.rs` — fixture updates.
- `mkdocs/docs/query-guide/query-audit-log.md` — `## Fields` table.
- `mkdocs/docs/gateway/index.md` — "Client IP Security" section (`:337-343`), now-stale
  `X-Forwarded-For` wording.
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
- **One trusted proxy hop is assumed; a server-side proxy hop still hides the end user.**
  Rightmost-`X-Forwarded-For` is exactly right for one appending proxy in front of the service
  (the ALB) and nothing else. Two consequences worth stating plainly:
  - *If a second trusted proxy is ever put in front of the ALB*, the ALB will append after that
    proxy's entry and the rightmost read will report the *upstream proxy's* address instead of the
    client's. The fix at that point is to skip a fixed number of entries from the right (or make
    the hop count configurable), not to go back to the leftmost read. Not built now: there is one
    hop today, and a configurable trusted-hop count is scope this issue doesn't ask for.
  - *For FlightSQL calls that a Micromegas service proxies*, `client_ip` is that service's address
    as seen by FlightSQL's own front, not the browser's. Neither proxy forwards the caller's XFF
    chain: the gateway's header forwarding is allow-list based and `x-forwarded-for` is not on the
    allow-list (`http_gateway.rs:44-57`, `:79-104`), and `analytics-web-srv`'s `/api/query-stream`
    (`stream_query.rs:271-282`) builds its `BearerFlightSQLClientFactory` with only
    `x-client-notebook`/`x-client-cell` and forwards no client-IP information at all. The gateway
    does compute an `x-client-ip` metadata header (`http_gateway.rs:209-216`) that would carry the
    original caller's IP, but nothing on the FlightSQL side reads it, and this plan doesn't wire it
    up: doing so safely requires FlightSQL to first establish that its immediate gRPC peer really
    *is* the gateway (mTLS service identity, or a network-topology assumption), since a direct gRPC
    caller — e.g. the Python client talking straight to `flight-sql-srv` — could otherwise set
    `x-client-ip` to anything. `analytics-web-srv` has no such header at all. Both are real design
    decisions that shouldn't be bundled into a logging-gap fix. So `/gateway/query` records report
    the gateway's address and every web-app notebook/query-editor query (`client="web"`, by far the
    highest-volume source of audit records) reports `analytics-web-srv`'s. Flagged as a known
    limitation in the doc update (step 13) and as an Open Question below.
- **`AuditClaims` reads unverified claims, same as the `sub`-only extraction it replaces.**
  `auth_callback`/`auth_refresh` don't run the freshly-exchanged `id_token` through
  `auth_provider.validate_request()` before logging — they trust the raw JWT payload from the
  token endpoint's own response (reached over TLS directly), same as today. This plan doesn't
  change that trust boundary; it only extracts one more field from the same already-trusted
  payload.
- **`/auth/*`'s `client_ip` is the same value as everywhere else, and the spoofing hole it used to
  inherit is closed by this plan.** `auth_observability_middleware` calls the same shared
  `get_client_ip`, which after Design §3 returns the ALB-observed address. A caller can still
  *prepend* entries to `X-Forwarded-For` (or send it as a separate header field line before the
  ALB's), but `get_all(...).last()` plus taking the rightmost entry of that last line always
  resolves to the ALB's own appended observation, so those entries are ignored regardless of which
  form the caller uses. Two residual caveats: (1) `X-Real-IP` is only consulted when
  `X-Forwarded-For` is absent entirely — behind the ALB it never is, so that branch is effectively
  unreachable in the deployed topology, but a caller reaching a service *directly* (bypassing the
  ALB) can still set `X-Real-IP` and have it believed; (2) the one-trusted-hop assumption above
  applies here too. Neither is a regression, and both are strictly better than today's leftmost
  read, which any caller could win outright.
- **A single `get_client_ip` rather than one implementation per transport.** The only reason this
  codebase resolves a client IP is logging, so there is no benefit to two functions with different
  trust semantics — that only produces two `client_ip` fields in the same service's logs that
  disagree and can't be compared. The extra machinery a shared function costs on the gRPC side is
  a single `.as_ref()` (Design §3), which is not enough to justify a fork.

## Documentation

- `mkdocs/docs/query-guide/query-audit-log.md` — `## Fields` table (`client_ip` row) and a Notes
  bullet on the proxy-hop caveat (HTTP gateway *and* `analytics-web-srv`'s `/api/query-stream`).
- `mkdocs/docs/gateway/index.md` — "Client IP Security" (`:337-343`): the `X-Forwarded-For`
  bullet and its lead-in, restated for rightmost-entry semantics.
- `CHANGELOG.md` — `## Unreleased` entries under **Analytics:** and **Auth:**, including the
  behavior change to every existing `client_ip` logger.

## Testing Strategy

1. `cargo fmt` and `cargo clippy --workspace -- -D warnings` (per `rust/CLAUDE.md`).
2. `cargo test -p analytics-web-srv` — existing suite plus the new `extract_audit_claims_from_token`
   cases (step 5); confirms no routing regression from wrapping `/auth/*` in
   `auth_observability_middleware`.
3. `cargo test -p micromegas --features server` — covers the new `auth_observability_tests.rs`
   (step 6, query-string redaction), the updated `query_audit_tests.rs`, the new
   `http_utils_tests.rs`, and (unchanged, as a regression check on the `get_client_ip` behavior
   change) `http_gateway_tests.rs`'s four `build_origin_metadata` cases.
4. **Manual verification** (the redaction rule itself is covered by the automated test in step 6;
   the rest — real OIDC flow, real FlightSQL traffic — still needs manual checks; see
   `python3 local_test_env/ai_scripts/start_services.py`):
   - Start services, hit `GET /auth/login` (or trigger any `/auth/*` request) and `tail -f
     /tmp/analytics.log` (or the monolith log), confirming a `request method=GET
     path=/auth/login client_ip=...` / `response status=... path=... client_ip=...` pair now
     appears — it did not before this change.
   - Complete a real OIDC login flow and confirm the `[auth_success] event=login sub=... email=...
     issuer=...` line now includes a real email address, not just `sub`. Trigger a token refresh
     (or wait for one) and confirm `[auth_success] event=token_refresh sub=... email=...` likewise.
   - Run `micromegas-query "SELECT 1" --all` directly against `flight-sql-srv` and confirm the
     `execute_query ...` start-of-query line now includes `client_ip=<your real IP>`; query
     `flightsql_query_audit` (per `query-audit-log.md`'s pattern) and confirm the JSON record has
     `"client_ip"` set to the same value, and that `LogUriService`'s `uri=... client_ip=...` line
     for the same request agrees (they now share one implementation). Locally there is no ALB, so
     this exercises the `SocketAddr` fallback.
   - Rightmost-`X-Forwarded-For` selection against a running service: `curl -H 'X-Forwarded-For:
     1.2.3.4, 198.51.100.9' http://127.0.0.1:3000/...` and confirm the logged `client_ip` is
     `198.51.100.9`, not `1.2.3.4` — i.e. the client-prepended entry is ignored. (Unit-tested in
     step 10; this just confirms the wiring end to end.)
   - Run a query from the web app's notebook/query editor (`/api/query-stream`) and confirm
     `client_ip` in the resulting audit record is `analytics-web-srv`'s own address, not the
     browser's — this is the highest-volume (`client="web"`) case of the proxy caveat. If a
     gateway deployment is available, also run the same query through `/gateway/query` and confirm
     `client_ip` is the *gateway's* address, not the original HTTP caller's — together confirming
     the documented caveat (step 13) matches actual behavior for both proxy hops.

## Open Questions

- **How should a server-side proxy hop pass the original caller's IP to FlightSQL?** Still open,
  but reframed by the single-implementation decision. The gateway already computes `x-client-ip`
  via `get_client_ip` (`http_gateway.rs:213`), so after this change that header carries the
  ALB-observed address of the *original HTTP caller* and is no longer spoofable at the point it's
  computed — the remaining obstacle is on the receiving end: `flight-sql-srv` has no way to know
  that its immediate gRPC peer is a trusted gateway rather than an arbitrary client setting the
  header, so reading `x-client-ip` would reintroduce exactly the spoofing hole this plan removes.
  Given the "one client-IP implementation" rule, the cleaner follow-up is probably not a second
  audit field but making the proxies *append to `X-Forwarded-For`* when they open their downstream
  gRPC connection (gateway: add it to `build_origin_metadata` and drop the bespoke `x-client-ip`;
  `analytics-web-srv`: add it in `stream_query.rs`), so the existing `get_client_ip` picks it up
  with no new code on the server — but that raises the trusted-hop count from one to two and so
  needs the configurable-hop-count work from Trade-offs first. Left as future work either way;
  until then the documented caveat (step 13) stands.
