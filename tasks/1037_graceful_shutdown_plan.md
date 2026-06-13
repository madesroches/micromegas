# Issue #1037 — Graceful shutdown for micromegas services

## Overview

Add SIGTERM-driven graceful shutdown to the deployed services — `telemetry-ingestion-srv`, `flight-sql-srv`, `telemetry-admin crond`, and `analytics-web-srv` — so ECS/Fargate task replacement (deploys, scale-down, AZ rebalance) drains in-flight work instead of killing it. Today every task replacement is a data-loss event for ingestion: the client sink's retry budget is ~1s over 3 attempts (`rust/telemetry-sink/src/lib.rs:420-422`) and its queue caps at 16 blocks (`rust/telemetry-sink/src/lib.rs:115`), so blocks dropped during an abrupt kill are gone permanently. For the other services the damage is milder (aborted queries, half-done materializations, lost final telemetry) but the fix is the same shape. Pairs with #1038 (`/ready` probe).

ECS uses the default `stopTimeout` of 30s, so the drain grace period defaults to **25s** everywhere, leaving headroom for the final telemetry flush.

## Current State

Common to all services:
- The `#[micromegas_main]` macro installs `with_ctrlc_handling()` (`rust/micromegas-proc-macros/src/lib.rs:115`), which registers a **SIGINT-only** handler via the `ctrlc` crate (no `termination` feature — `rust/telemetry-sink/src/lib.rs:182-190`). It flushes telemetry then `std::process::exit(1)`. SIGTERM is not covered anywhere: it hits the default disposition, the process dies instantly, and the service's own telemetry is never flushed.
- The workspace `tokio` dependency (`rust/Cargo.toml:85`) lacks the `signal` feature, so `tokio::signal::unix` is unavailable. (`time` is already enabled transitively via feature unification — `maintenance.rs:209` uses `tokio::time::sleep` today — but should be made explicit.)

Per service:
- **telemetry-ingestion-srv** — `axum::serve(...).await.unwrap()` at `rust/telemetry-ingestion-srv/src/main.rs:99-104`, no shutdown signal. In-flight `insert_block` requests get connection resets on SIGTERM.
- **analytics-web-srv** — same axum pattern at `rust/analytics-web-srv/src/main.rs:457-463`.
- **flight-sql-srv** — tonic `Server::serve_with_incoming` at `rust/public/src/servers/flight_sql_server.rs:206-210`, called from `FlightSqlServer::build_and_serve`. In-flight FlightSQL queries (including long DoGet streams) are aborted on SIGTERM.
- **telemetry-admin crond** — `servers::maintenance::daemon()` spawns four `run_tasks_forever` loops (`rust/public/src/servers/maintenance.rs:176-214, 238-286`) that schedule cron tasks into `JoinSet`s and never return. SIGTERM kills mid-materialization; tasks are re-runnable on the next tick so no permanent loss, but it leaves partial work and drops the admin's own telemetry.

## Design

### Shared primitives: `rust/public/src/servers/shutdown.rs` (new)

```rust
/// Completes when SIGTERM is received. On non-unix targets, never completes
/// (preserves current behavior; production deploys are Linux/ECS).
pub async fn wait_for_sigterm();

/// Fans a shutdown future out to N consumers via a watch channel.
/// `subscribe()` returns a future that completes once `shutdown` has fired —
/// usable as the drain trigger for axum/tonic and as the deadline arm.
pub struct ShutdownFanout { /* watch::Sender/Receiver */ }

/// Axum convenience wrapper: serves `make_service`, draining when `shutdown`
/// completes, returning once drained or once `grace` has elapsed after the signal.
pub async fn serve_axum_with_graceful_shutdown<F>(
    listener: tokio::net::TcpListener,
    make_service: /* IntoMakeServiceWithConnectInfo */,
    shutdown: F,
    grace: Duration,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send + 'static;
```

The shutdown future is always a generic parameter (not hardwired to signals) so tests can drive drain with a `Notify`/oneshot instead of delivering real signals to the test process.

Internal shape — the signal must be observable by two consumers (the framework's drain future and the hard-deadline arm):

```
shutdown future ──► fanout ──┬─► framework drain (stop accepting, finish in-flight work)
                             └─► deadline arm: subscribe() + sleep(grace)

tokio::select! {
    res = serve_future => drained cleanly (log + return res),
    _   = deadline arm => grace elapsed (warn! + return Ok),
}
```

Log at three points: signal received ("draining, grace=Ns"), drain completed cleanly, grace elapsed with work still in flight.

### Per-service wiring

**telemetry-ingestion-srv** (`src/main.rs`)
- New CLI flag `--shutdown-grace-period-seconds`, default 25.
- Replace the serve block with `serve_axum_with_graceful_shutdown(listener, app..., wait_for_sigterm(), grace)`. While here, replace the `.unwrap()` on `TcpListener::bind` with `.with_context(...)` per error-handling style.

**analytics-web-srv** (`src/main.rs:456-463`)
- Same flag, same one-line swap of the serve call.

**flight-sql-srv** (`rust/public/src/servers/flight_sql_server.rs` + `src/flight_sql_srv.rs`)
- Builder gains `with_shutdown_grace(Duration)` (default 25s); `build_and_serve` switches `serve_with_incoming` → `serve_with_incoming_shutdown(incoming, fanout.subscribe())`, wrapped in the same select-against-deadline.
- Binary gains the `--shutdown-grace-period-seconds` flag feeding the builder.
- Note: tonic's graceful shutdown waits for in-flight RPCs including long-running DoGet streams — the grace cap matters most here.

**telemetry-admin crond** (`rust/public/src/servers/maintenance.rs` + `src/telemetry_admin.rs`)
- `run_tasks_forever` gains a shutdown receiver: the scheduling loop `select!`s its sleep against the shutdown signal; on shutdown it stops scheduling and drains the `JoinSet` of running tasks (`join_next` until empty).
- `daemon()` gains `shutdown: impl Future + Send + 'static` and `grace: Duration` parameters, fans the signal out to the four runner loops, and applies the deadline arm around `runners.join_all()`.
- The `crond` subcommand gains a `--shutdown-grace-period-seconds` arg (other subcommands are one-shot CLI runs — out of scope).
- Both functions are `pub` in the public crate, so this is a (pre-1.0, acceptable) API change.

In every service, a clean drain means `main` returns normally, the `_telemetry_guard` drops, and the service's own final logs/metrics are flushed — fixing a second loss for free.

### Signal handling boundaries

SIGINT stays with the existing `ctrlc` handler (flush + immediate exit). Registering a second SIGINT handler via tokio's `signal-hook-registry` would conflict with `ctrlc`'s direct `sigaction` registration — last writer wins, behavior becomes registration-order-dependent. ECS, Kubernetes, and systemd all send SIGTERM, so the production path is fully covered; dev ctrl-c keeps its instant-exit behavior.

## Implementation Steps

### Phase 1 — shared infrastructure
1. `rust/Cargo.toml:85` — add `"signal"` and `"time"` to the workspace tokio features.
2. `rust/public/src/servers/shutdown.rs` (new) — `wait_for_sigterm`, `ShutdownFanout`, `serve_axum_with_graceful_shutdown`.
3. `rust/public/src/servers/mod.rs` — register `pub mod shutdown;` with a doc comment matching the module list style.
4. `rust/public/tests/graceful_shutdown_tests.rs` (new) — tests for the axum wrapper (see Testing Strategy).

### Phase 2 — ingestion (the #1037 core)
5. `rust/telemetry-ingestion-srv/src/main.rs` — grace flag, serve swap, bind-`unwrap()` fix.

### Phase 3 — analytics-web-srv
6. `rust/analytics-web-srv/src/main.rs` — grace flag, serve swap.

### Phase 4 — flight-sql-srv
7. `rust/public/src/servers/flight_sql_server.rs` — `with_shutdown_grace` builder option; `serve_with_incoming_shutdown` + deadline select.
8. `rust/flight-sql-srv/src/flight_sql_srv.rs` — grace flag feeding the builder.

### Phase 5 — telemetry-admin crond
9. `rust/public/src/servers/maintenance.rs` — shutdown receiver in `run_tasks_forever`; shutdown + grace params on `daemon()`; JoinSet drain.
10. `rust/telemetry-admin-cli/src/telemetry_admin.rs` — `--shutdown-grace-period-seconds` on the `crond` subcommand, wire `wait_for_sigterm()` into `daemon()`.

### Finish
11. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 ../build/rust_ci.py`.

Each phase is a self-contained commit; the branch is one PR.

## Files to Modify

- `rust/Cargo.toml` — tokio features
- `rust/public/src/servers/shutdown.rs` — **new**
- `rust/public/src/servers/mod.rs` — module registration
- `rust/public/src/servers/flight_sql_server.rs` — tonic drain + builder option
- `rust/public/src/servers/maintenance.rs` — cron loop drain
- `rust/public/tests/graceful_shutdown_tests.rs` — **new**
- `rust/telemetry-ingestion-srv/src/main.rs`
- `rust/analytics-web-srv/src/main.rs`
- `rust/flight-sql-srv/src/flight_sql_srv.rs`
- `rust/telemetry-admin-cli/src/telemetry_admin.rs`

## Trade-offs

- **Shared `shutdown.rs` vs. per-service inline code** — four services × the same fanout/deadline logic argues for one module. The generic `shutdown: impl Future` parameter is what makes drain testable without real signals.
- **SIGTERM-only vs. SIGTERM+SIGINT** — the issue suggests SIGINT for local dev, but the `ctrlc` crate already owns SIGINT process-wide and exits immediately. Taking SIGINT over would require an opt-out on `TelemetryGuardBuilder`/`micromegas_main`, touching every binary that uses the macro. Not worth the blast radius; deferred.
- **Explicit grace timeout vs. relying on ECS SIGKILL** — letting ECS SIGKILL at `stopTimeout` would also bound the drain, but the process would die mid-flush with no log trail. An in-process deadline logs the timeout and still exits through the normal path (telemetry flush included). This matters most for flight-sql-srv (long streams) and crond (long materializations).
- **`daemon()`/`run_tasks_forever` signature change vs. new parallel APIs** — these are `pub`, but the crate is pre-1.0 and the only in-repo caller is telemetry-admin. A signature change keeps one code path; duplicating the functions for compatibility isn't justified.
- **CLI flag vs. env var for the grace period** — flag, matching each binary's existing clap pattern. Env vars are reserved for connection/auth config.
- **Longer client-side retry window instead** — complementary, not a substitute: it can't be retrofitted into already-shipped instrumented binaries, and the sink's single dispatch thread plus 16-block queue cap its benefit. Server-side drain fixes the most frequent loss event (deploys) for every client version at once. Possible follow-up issue on the sink.

## Documentation

No ops/deployment page exists under `mkdocs/docs/` to update. Rustdoc on the new module, the changed `pub` signatures in `maintenance.rs`/`flight_sql_server.rs`, and clap `--help` text cover it. CHANGELOG/release notes should mention the `daemon()`/`run_tasks_forever` signature changes for external users of the public crate.

## Testing Strategy

Integration tests in `rust/public/tests/graceful_shutdown_tests.rs`, driving shutdown via a `Notify`/oneshot future instead of real signals:

1. **Axum drain completes** — handler sleeps ~300ms; dispatch a request, trigger shutdown while it's in flight; assert the client gets 200 and the serve call returns `Ok` well before the grace period.
2. **Axum grace cap enforced** — handler sleeps longer than a short grace (e.g. 200ms); assert the serve call returns shortly after grace elapses.
3. **New connections refused after signal** — after triggering shutdown, a fresh connection attempt fails. (Axum-version-dependent; keep the assertion loose.)
4. **Cron loop drain** — `run_tasks_forever` with a stub task that records start/finish and sleeps ~300ms; trigger shutdown mid-task; assert the loop returns only after the task finished and schedules nothing new.

The tonic path reuses `ShutdownFanout` + the same select shape; a dedicated tonic integration test would need a full FlightSQL stack, so it's covered by manual verification instead.

Manual verification: `python3 local_test_env/ai_scripts/start_services.py`, run a client send loop plus a long `micromegas-query`, `kill -TERM` each service PID, confirm drain log lines in `/tmp/ingestion.log`, `/tmp/analytics.log`, `/tmp/admin.log` and zero client-side errors.

## Open Questions

None — grace default of 25s fits the confirmed ECS default `stopTimeout` (30s), and scope covers all four deployed services per discussion.
