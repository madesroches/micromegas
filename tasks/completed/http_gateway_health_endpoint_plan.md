# HTTP Gateway Health Check Endpoint Plan

## Overview

Add a `GET /gateway/health` endpoint to the HTTP gateway so it can be used as a
target for load balancer health checks (e.g. AWS ALB) without the current
workaround of hitting `POST /gateway/query` and accepting `405 Method Not
Allowed` as a healthy response.

GitHub issue: [#994](https://github.com/madesroches/micromegas/issues/994).

## Current State

- Routes are registered in
  `rust/public/src/servers/http_gateway.rs:380-382` via
  `register_routes()`, which currently only wires up `POST /gateway/query`.
- The gateway binary
  (`rust/http-gateway/src/http_gateway_srv.rs:26`) wraps that router with
  `Extension(header_config)` and serves it.
- `MICROMEGAS_FLIGHTSQL_URL` is read *per query request* (line 279–282).
  The gateway does not hold a persistent FlightSQL channel, so there is no
  always-on "is the backend reachable?" state to report.
- Unit tests for the gateway live in
  `rust/public/tests/http_gateway_tests.rs` — they exercise helpers
  (`HeaderForwardingConfig`, `build_origin_metadata`) rather than spinning
  up a live HTTP server.

### Precedent in the repo

- `rust/telemetry-ingestion-srv/src/main.rs:56-57` — minimal liveness
  endpoint that returns `StatusCode::OK`.
- `rust/analytics-web-srv/src/main.rs:388-395` — returns a JSON payload
  with `status`, `timestamp`, and a (currently hardcoded `false`)
  `flightsql_connected` flag.
- Documentation reference: `mkdocs/docs/admin/authentication.md:381`
  states the `/health` endpoint remains public even when auth is enabled —
  consistent with treating health as unauthenticated.

## Design

### Endpoint

- Method/path: `GET /gateway/health`
- Auth: none (matches the other services; makes it usable by ALB probes
  that can't send bearer tokens).
- Response: `200 OK` with a small JSON body for observability, keeping
  shape consistent with the analytics web app:

  ```json
  {
    "status": "healthy",
    "timestamp": "2026-04-20T12:34:56Z"
  }
  ```

### Liveness vs. readiness

Scope this change to **liveness only**. Rationale:

- The gateway is stateless between requests and re-connects to FlightSQL
  per query. There is no cached "backend is up" signal to surface.
- Adding a real backend probe (e.g. opening a gRPC channel on every
  health request) would make `/gateway/health` expensive, noisy in logs,
  and itself a potential source of cascading failure under ALB probe
  load.
- ALB's primary need is "is this gateway process accepting HTTP traffic?"
  — a liveness check answers that.

The issue says "Ideally it could also verify connectivity to the FlightSQL
backend." That is deferred (see **Open Questions**). The JSON body leaves
room to add a `flightsql_connected` field later without breaking clients.

### Handler

Add a new handler in `rust/public/src/servers/http_gateway.rs`:

```rust
#[derive(Debug, serde::Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub timestamp: DateTime<Utc>,
}

pub async fn handle_health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy",
        timestamp: Utc::now(),
    })
}
```

Update `register_routes`:

```rust
pub fn register_routes(router: Router) -> Router {
    router
        .route("/gateway/query", post(handle_query))
        .route("/gateway/health", get(handle_health))
}
```

`axum::routing::get` needs to be added to the existing `use axum::{...}`
block alongside `post`.

## Implementation Steps

1. **Add health handler** in `rust/public/src/servers/http_gateway.rs`:
   - Add `HealthResponse` struct (derive `Debug`, `Serialize`).
   - Add `handle_health` async function returning `Json<HealthResponse>`.
   - Extend the `use axum::routing::{post}` import with `get`.
2. **Register the route** in the same file by updating
   `register_routes()` to chain `.route("/gateway/health", get(handle_health))`.
3. **Add a unit test** in `rust/public/tests/http_gateway_tests.rs`:
   - Call `handle_health()` directly and assert `status == "healthy"`.
   - (Spinning up an axum server is not necessary and would diverge from
     the existing test style in that file.)
4. **Update documentation** in `mkdocs/docs/gateway/index.md`:
   - Add a short "Health Check" subsection near the Quick Start (or
     under Architecture / Endpoints) showing
     `curl http://localhost:3000/gateway/health` and the example
     response.
   - Mention that the endpoint is unauthenticated and intended for load
     balancer probes.
5. **Run checks** from `rust/`:
   - `cargo fmt`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test -p micromegas`

## Files to Modify

- `rust/public/src/servers/http_gateway.rs` — new handler, route
  registration, import tweak.
- `rust/public/tests/http_gateway_tests.rs` — unit test for
  `handle_health`.
- `mkdocs/docs/gateway/index.md` — documentation of the new endpoint.

No changes are needed in `rust/http-gateway/src/http_gateway_srv.rs`
because it already delegates route registration to
`servers::http_gateway::register_routes`.

## Trade-offs

- **JSON body vs. bare `200 OK`.** Telemetry-ingestion-srv uses a bare
  200. Analytics-web-srv uses JSON. We pick JSON to mirror the
  analytics-web-srv shape and leave room for future fields
  (`flightsql_connected`, `version`, etc.) without breaking consumers.
  ALBs treat any `2xx` as healthy by default, so the body doesn't hurt.
- **Liveness only (no FlightSQL probe).** Keeps the endpoint O(1) and
  avoids amplifying probe traffic onto the backend. If true readiness is
  needed later, a separate `/gateway/ready` path (or a query parameter)
  can probe the backend without changing the liveness contract.
- **No auth on the route.** Consistent with the other services'
  health endpoints. The gateway itself doesn't authenticate today (auth
  is transparently forwarded to FlightSQL), so nothing changes.

## Documentation

- Update `mkdocs/docs/gateway/index.md` to document `GET /gateway/health`
  (example curl + response, unauthenticated, intended for load balancer
  probes). No other docs reference gateway endpoints directly.

## Testing Strategy

- **Unit test**: call `handle_health()` and assert the response body
  fields (status string, timestamp parses as RFC3339).
- **Manual smoke test**: start the gateway locally and run
  `curl -i http://localhost:3000/gateway/health` — expect `200 OK` and
  a JSON body. Also verify `curl -X POST http://localhost:3000/gateway/health`
  returns `405 Method Not Allowed` (route is GET-only), and that the
  existing `POST /gateway/query` path still works.

## Open Questions

- **Should the endpoint probe the FlightSQL backend?** The issue mentions
  this as an "ideally" nice-to-have. Current recommendation: no, for the
  reasons in the Design section. Confirm with the user before
  implementing if they want a readiness probe instead of (or in addition
  to) liveness.
- **Path shape: `/gateway/health` vs. `/health`.** Issue text asks for
  `/gateway/health`, matching the existing `/gateway/query` prefix. Going
  with that. (Other services use root `/health`, but the gateway's
  single-prefix convention is worth preserving.)
