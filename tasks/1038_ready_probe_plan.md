# Issue #1038 — Deep `/ready` probe for all services

## Overview

Add a `/ready` endpoint to every service so ALBs can stop routing to tasks whose dependencies are unavailable. The existing liveness checks stay unconditional. During Aurora failover or transient object-store errors, readiness failures pull the task out of the ALB target group rather than serving 5xx to clients.

Four targets: `telemetry-ingestion-srv` (HTTP), `flight-sql-srv` (gRPC), `analytics-web-srv` (HTTP), and the `micromegas-monolith` which inherits from all three.

## Current State

### telemetry-ingestion-srv / serve_ingestion

`rust/public/src/servers/ingestion.rs:118-119`:
```rust
let health_router =
    Router::new().route("/health", get(|| async { axum::http::StatusCode::OK }));
```

`WebIngestionService` (`rust/ingestion/src/web_ingestion_service.rs:61`) owns a private `DataLakeConnection` with `pub db_pool: PgPool` and `pub blob_storage: Arc<BlobStorage>`.

### flight-sql-srv / FlightSqlServer

`rust/public/src/servers/grpc_health_service.rs:40-44`: the tower middleware intercepts any path ending in `/health` and returns `Status::ok` unconditionally — no probe. The gRPC server (`rust/public/src/servers/flight_sql_server.rs`) has access to `LakehouseContext` (containing `DataLakeConnection`) only inside `build_and_serve()`.

### analytics-web-srv / run_web_server

`rust/analytics-web-srv/src/web_server.rs:119-121`:
```rust
fn build_public_routes(base_path: &str) -> Router {
    Router::new().route(&format!("{base_path}/api/health"), get(health_check))
}
```
`health_check` returns a JSON blob with `flightsql_connected: false` hardcoded — unconditional. The `app_db_pool: PgPool` is local to `run_web_server()`.

### monolith

`rust/monolith/src/main.rs` calls `serve_ingestion`, `FlightSqlServer::builder()`, and `run_web_server` — it inherits whatever each role exposes.

## Design

### Two-endpoint model (all services)

| Endpoint | Purpose | Returns |
|----------|---------|---------|
| liveness | Is the process alive? | 200 always — ECS task restart decision |
| readiness | Can the process serve traffic? | 200 / 503 — ALB routing decision |

### 1. telemetry-ingestion-srv

**New endpoint**: `GET /ready` on the existing HTTP port (8081).

Added to the `health_router` (outside auth middleware, same as `/health`).

**Probe**: `tokio::join!` (DB `SELECT 1` + blob `list(None).try_next()`) under a 2 s `tokio::time::timeout`. Returns 503 on any failure or timeout.

**Caching**: Cache last-success `Instant` for 1 s inside `WebIngestionService`. `std::sync::Mutex<Option<std::time::Instant>>` — the critical section is nanosecond-range, no async work inside.

**BlobStorage probe helper**: Add `BlobStorage::probe()` to `rust/telemetry/src/blob_storage.rs` to keep `object_store` types out of `micromegas-ingestion`'s dependency surface:
```rust
pub async fn probe(&self) -> anyhow::Result<()> {
    use futures::TryStreamExt as _;
    self.blob_store.list(None).try_next().await?;
    Ok(())
}
```

**Cache + check_ready on WebIngestionService**:
```rust
pub struct WebIngestionService {
    lake: DataLakeConnection,
    ready_ok_until: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
}

// check_ready():
//   1. Lock cache — if last_ok + 1s > now, return true immediately.
//   2. tokio::time::timeout(2s, tokio::join!(SELECT 1, blob.probe()))
//   3. Success: update cache, return true.
//   4. Failure / timeout: clear cache, return false.
```

`ready_ok_until` is wrapped in `Arc` so that `#[derive(Clone)]` on `WebIngestionService` continues to compile (`std::sync::Mutex` alone does not implement `Clone`; `Arc<Mutex<_>>` does).

`tokio` with the `time` feature must be added as a direct dependency of `micromegas-ingestion`.

### 2. flight-sql-srv

The ALB health check protocol doesn't need to match the service protocol — a plain HTTP endpoint is sufficient. Keeping all services on HTTP avoids adding `tonic-health` and keeps the operational model identical across the fleet.

**Approach**: a lightweight sidecar HTTP listener on a configurable port. `FlightSqlServerBuilder` gets:
```rust
pub fn with_health_addr(mut self, addr: SocketAddr) -> Self
```
If set, `build_and_serve()` spawns a minimal Axum router (`/health` unconditional, `/ready` DB + blob probe) on that address alongside the gRPC server, both under the same `ShutdownFanout`.

The `DataLakeConnection` is available via `lakehouse.lake()`. A `ReadinessProbe` struct (see below) holds the lake reference and 1 s cache.

`GrpcHealthService` stays unchanged — it continues to serve the unconditional gRPC liveness response for any gRPC tooling that uses it.

`--health-listen-addr` is added to `rust/flight-sql-srv/src/flight_sql_srv.rs`. The monolith omits it for its FlightSQL role — the shared lake is already covered by the ingestion role's `/ready` at 8081.

### 3. analytics-web-srv

**New endpoint**: `GET {base_path}/api/ready` on the existing HTTP port (3000).

**Probe**: `SELECT 1` on `app_db_pool` with a 2 s timeout. Blob storage for maps is optional (already returns 503 when unconfigured) and is out of scope for readiness here — the app DB is the critical dependency.

**Implementation**: `build_public_routes()` accepts `Arc<ReadinessState>` and layers it as an Extension. `ReadinessState` already holds the `PgPool`, so no separate `Extension(pool)` is needed on this router:
```rust
fn build_public_routes(base_path: &str, readiness_state: Arc<ReadinessState>) -> Router {
    Router::new()
        .route(&format!("{base_path}/api/health"), get(health_check))
        .route(&format!("{base_path}/api/ready"), get(ready_check))
        .layer(Extension(readiness_state))
}
```

**Caching**: same 1 s cache pattern. Hold the cache in a small `Arc<ReadinessState>` Extension rather than in a service struct (there is no service struct in `web_server.rs` — the pool is passed directly). `ReadinessState` owns the `PgPool` and `Mutex<Option<Instant>>`.

**Ready handler**:
```rust
async fn ready_check(
    Extension(state): Extension<Arc<ReadinessState>>,
) -> StatusCode { ... }
```

### 4. monolith

No dedicated changes. The monolith inherits:
- `/ready` at port 8081 from the ingestion role
- `/api/ready` at port 3000 from the web role
- No FlightSQL health port (not set by default; the ingestion `/ready` covers the shared lake)

## Shared readiness module

`rust/public/src/servers/readiness.rs` — new file that houses the reusable:
- `ReadinessProbe` struct (lake + cache)
- `check_ready()` async method (timeout + join)

Both the FlightSQL sidecar and the ingestion service can use it. `WebIngestionService.check_ready()` can delegate to it, or it can be inlined if the coupling is too tight.

## Implementation Steps

1. **`rust/telemetry/src/blob_storage.rs`**: add `BlobStorage::probe()`.

2. **`rust/public/src/servers/readiness.rs`** (new): `ReadinessProbe` struct with `check_ready()`.

3. **`rust/ingestion/Cargo.toml`**: add `tokio = { workspace = true, features = ["time"] }`.

4. **`rust/ingestion/src/web_ingestion_service.rs`**:
   - Add `ready_ok_until: Arc<Mutex<Option<Instant>>>` field; update `new()`.
   - Add `pub async fn check_ready(&self) -> bool` with the 10-line logic inlined (do **not** delegate to `ReadinessProbe` — that lives in the public crate and `micromegas-ingestion` must not depend on it).

5. **`rust/public/src/servers/ingestion.rs`**:
   - Add `ready_handler`.
   - Add `.route("/ready", get(ready_handler))` to `health_router`.
   - Layer `Extension(service.clone())` onto `health_router` **before** merging with `protected_app`. `serve_ingestion` applies `.layer(Extension(service))` only to `protected_app`; Axum's `merge()` does not propagate Extensions between sub-routers, so any handler on `health_router` that extracts `Extension<Arc<WebIngestionService>>` will panic at runtime without this explicit layer.

6. **`rust/public/src/servers/flight_sql_server.rs`**:
   - Add `health_listen_addr: Option<SocketAddr>` to `FlightSqlServerBuilder`.
   - Add `pub fn with_health_addr(mut self, addr: SocketAddr) -> Self`.
   - In `build_and_serve()`: if `health_listen_addr` is set, spawn a sidecar Axum task with `/health` and `/ready` using `ReadinessProbe(lakehouse.lake().clone())`.

7. **`rust/flight-sql-srv/src/flight_sql_srv.rs`**: add `--health-listen-addr` CLI flag, pass to `FlightSqlServerBuilder::with_health_addr`.

8. **`rust/analytics-web-srv/src/web_server.rs`**:
   - Introduce `ReadinessState` (holding `PgPool` + `Mutex<Option<Instant>>`).
   - Add `ready_check` handler (extracts `Extension<Arc<ReadinessState>>` only; pool is accessed via `state.pool`).
   - Update `build_public_routes(base_path, Arc<ReadinessState>)` signature; layer the state with `.layer(Extension(readiness_state.clone()))` inside `build_public_routes`.
   - In `run_web_server()`, after the pool is created, construct `let readiness_state = Arc::new(ReadinessState::new(app_db_pool.clone()));` and pass it to `build_public_routes`. No additional `.layer()` call is needed on the merged app router — the Extension is scoped to the public routes sub-router where the handler lives.

9. **Tests**: add `rust/ingestion/tests/readiness.rs` (integration, requires env vars). Add unit test for caching logic (can be done without a real DB by using a fake `Instant`).

## Files to Modify

- `rust/telemetry/src/blob_storage.rs`
- `rust/ingestion/Cargo.toml`
- `rust/ingestion/src/web_ingestion_service.rs`
- `rust/public/src/servers/mod.rs` (expose `readiness`)
- `rust/public/src/servers/readiness.rs` (new)
- `rust/public/src/servers/ingestion.rs`
- `rust/public/src/servers/flight_sql_server.rs`
- `rust/flight-sql-srv/src/flight_sql_srv.rs`
- `rust/analytics-web-srv/src/web_server.rs`
- `rust/ingestion/tests/readiness.rs` (new)

## Trade-offs

**FlightSQL: sidecar HTTP vs. `tonic-health`**
The ALB health check protocol doesn't need to match the service protocol. A sidecar HTTP endpoint keeps all services on the same operational pattern (plain HTTP health checks), requires no new dependency, and is simpler to reason about. `tonic-health` would be the right call if gRPC-native health tooling (service meshes, `grpc-health-probe`) was a requirement, but it isn't here.

**Shared `ReadinessProbe` module vs. per-service structs**
The probe logic is 10 lines; a shared module is only worth it if both callers use it. `micromegas-ingestion` does **not** depend on `micromegas` (the public crate) — the dependency is the other way around — so `WebIngestionService.check_ready()` must inline the logic rather than delegate to `ReadinessProbe`. `ReadinessProbe` in `rust/public/src/servers/readiness.rs` is therefore used only by the FlightSQL sidecar. The small duplication is preferable to reversing the existing layering.

**analytics-web-srv: blob storage not probed**
Maps blob storage is optional — the service already returns 503 for maps routes when unconfigured, so it's never a hard dependency. Only the app DB (always required) is probed.

## Testing Strategy

- `cargo test` in `rust/` after each step.
- Manual smoke: `python3 local_test_env/ai_scripts/start_services.py` then:
  - `curl http://127.0.0.1:9000/ready` → 200
  - `curl http://127.0.0.1:3000/api/ready` → 200
- Negative: stop Postgres (`pg_ctl stop`) and verify both return 503 within 2 s.
- FlightSQL sidecar: `flight-sql-srv --health-listen-addr 127.0.0.1:8082`, then `curl http://127.0.0.1:8082/ready`.

## Open Questions

None. Direction is clear.
