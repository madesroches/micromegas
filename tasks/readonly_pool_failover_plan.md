# Reject Read-Only Postgres Connections After Failover Plan

Issue: #1625

## Overview

After an Aurora PostgreSQL failover, ingestion kept writing to the demoted instance (restarted
as a reader) for ~40s after RDS reported the failover complete. Every insert failed with
SQLSTATE `25006` (`cannot execute INSERT in a read-only transaction`), and the API-key store
returned `503` because its lookup runs `UPDATE ... SET last_used_at`. The pools accept and keep
connections to a read-only instance because nothing checks for it. This plan makes every
write-capable pool refuse read-only connections, both when a connection is opened and before a
pooled connection is handed out. During the stale-DNS window writes wait in `acquire` (failing
only on acquire timeout) instead of failing against the demoted writer until the pool recycles
its connections.

## Current State

- `rust/ingestion/src/data_lake_connection.rs:122` (`connect_to_data_lake`) and
  `rust/ingestion/src/remote_data_lake.rs:56` (`connect_to_remote_data_lake`) build the lake pool
  with bare `PgPoolOptions::new()`. Every service's lake pool comes from these two:
  telemetry-ingestion-srv and the monolith through `connect_to_remote_data_lake`; flight-sql and
  the maintenance daemon through `LakehouseContext` (`rust/analytics/src/lakehouse/lakehouse_context.rs:72`)
  → `connect_to_data_lake`.
- `rust/auth/src/db_api_key.rs:142` (`dedicated_key_store_pool`) builds the key-store, group, and
  audience-grant pools from the lake pool's **connect options only**
  (`lake_pool.connect_options()`), so any pool-level hooks on the lake pool are lost. Callers:
  `telemetry-ingestion-srv/src/main.rs:57`, `monolith/src/main.rs:211,234,235,279`,
  `public/src/servers/flight_sql_server.rs:328,341,342,364`.
- `rust/analytics-web-srv/src/web_server.rs:707` (app DB, writes screens, data sources, and so on) and
  `:746` (the analytics-keys/groups pool, which mints keys) build their own pools.
  `rust/monolith/src/main.rs:426` (`seed_local_data_source`) opens a short-lived pool that inserts.
- The sqlx 0.8.6 defaults that cause the bug are `test_before_acquire = true` (a `ping`, which
  succeeds on a reader), `max_lifetime = 30 min`, and `idle_timeout = 10 min`. Connections that land on
  the demoted instance stay in the pool until those timeouts, or something else, recycle them.
- Readiness probes (`web_ingestion_service.rs:206`) run `SELECT 1`, which also passes on a
  reader, so the tasks stayed "ready" while every write failed.

### sqlx capabilities (checked against 0.8.6 and the latest 0.9.0)

- There's no `target_session_attrs` support in either version. `PgConnectOptions` doesn't parse it, and
  the server's `ParameterStatus` map (which on PG14+ reports `default_transaction_read_only` /
  `in_hot_standby`) is `pub(crate)`, so it can't be read without a query.
- Both versions have `PoolOptions::after_connect`, `before_acquire`, and `after_release`
  (`sqlx-core/src/pool/options.rs:380,435,492`). Their semantics
  (`sqlx-core/src/pool/inner.rs`):
  - When `after_connect` returns `Err`, sqlx calls `close_hard()` on the connection, sleeps an
    exponential backoff, and **retries the connect** until the acquire deadline. It doesn't
    propagate the error. Each retry calls `connect_options.connect()` again, which runs a new DNS
    lookup.
  - When `before_acquire` returns `Ok(false)`, the idle connection is closed and acquire opens a
    new connection in its place (which goes through `after_connect`). `Err` does the same with
    `close_hard()`.
- `Pool::options()` returns `&PoolOptions`, and `PoolOptions: Clone` clones the hook `Arc`s. A
  pool derived from `lake_pool.options().clone()` therefore inherits the hooks without the
  deriving crate knowing about them.

## Design

### Read-write pool options

Add one constructor in `rust/ingestion/src/data_lake_connection.rs`, the crate that owns the
lake connection and that every write-pool site already depends on (`micromegas::ingestion` for
the binaries):

```rust
/// Pool options for a pool that must reach a writable primary.
pub fn read_write_pool_options() -> PgPoolOptions
```

It returns `PgPoolOptions::new()` with:

- **`after_connect`**: runs `SHOW transaction_read_only`. If the result is `on`, it emits
  `warn!` plus `imetric!("pg_read_only_connection_rejected", "count", 1)` and returns an `Err`
  (`sqlx::Error::Io(io::Error::other("connected to a read-only postgres instance"))`). sqlx then
  closes the connection and retries with backoff and a fresh DNS resolution, which is exactly the
  "reconnect and re-resolve" the issue asks for.
- **`before_acquire`**: runs the same check and returns `Ok(false)` if the connection is read-only
  (evict it), or propagates the error if the query fails (connection broken). This covers
  connections that became read-only while pooled.
- **`test_before_acquire(false)`**: the `before_acquire` query already makes one round trip and
  fails on a dead connection, so it replaces the `ping`. The hot path pays one round trip per
  acquire, the same as today.

The check matches libpq's `target_session_attrs=read-write` on pre-14 servers:
`transaction_read_only` is `on` on hot standbys and Aurora replicas, and also when
`default_transaction_read_only` is set. A small pure helper `pub fn is_read_only(setting: &str) -> bool`
holds the decision, so it can be unit-tested from
`rust/ingestion/tests/read_write_pool_tests.rs` and both hooks share it.

### FlightSQL: prefer a writable primary, fall back to read-only

FlightSQL keeps trying for a writable connection but accepts a read-only one when that is the
best available. Every other service stays strict.

**Selection.** A new enum in `data_lake_connection.rs` picks the policy:

```rust
pub enum WritablePolicy { Require, Prefer }
pub fn pool_options(policy: WritablePolicy) -> PgPoolOptions
pub fn read_write_pool_options() -> PgPoolOptions        // pool_options(Require)
pub fn prefer_read_write_pool_options() -> PgPoolOptions // pool_options(Prefer)
```

`connect_to_data_lake` and `LakehouseContext::from_env` both take a `WritablePolicy` as a new
first parameter, so the compiler lists every caller. The FlightSQL builder's non-injected path
passes `Prefer`. The maintenance daemon, `web_ingestion_service.rs:277`, and the tests pass
`Require`. `connect_to_remote_data_lake` stays strict and takes no policy. That keeps the
monolith, which injects its shared lake pool into the flight-sql role, on `Require`.

**Mechanism.** `Prefer` installs the same three settings as `Require`, but both hooks consult a
`ReadOnlyFallback` state that the closures capture by `Arc`. That state is shared by every
connection of the pool and by every pool cloned from its options. It holds `streak_start:
Option<Instant>` (the first read-only rejection since the last writable connect) and
`last_probe: Instant`, behind a `std::sync::Mutex` that is never held across an await. The
decisions are pure methods that take `now`, so they can be unit-tested:

- `on_connect(read_only, now) -> bool` (accept?): a writable connection clears `streak_start`
  and is accepted. A read-only connection starts the streak if none is running and is rejected
  until `now - streak_start >= FALLBACK_AFTER` (10s). Each rejection is an `after_connect` `Err`,
  so sqlx retries with a fresh DNS lookup. After the window it is accepted with
  `warn!` + `imetric!("pg_read_only_connection_accepted", "count", 1)`. Because the streak is
  pool-wide, only the first connect of an outage waits. A replica-only deployment pays the 10s
  once, at startup, and never again.
- `on_acquire(read_only, now) -> bool` (keep?): writable connections are kept. A read-only
  connection is evicted (`Ok(false)`) immediately when `streak_start` is `None`, because a
  writable connect has succeeded since the fallback. Otherwise it is evicted at most once per
  `PROBE_INTERVAL` (30s) pool-wide, as a re-probe, and kept in between. The replacement connect
  goes through `on_connect`. The streak is past the window, so it lands at once on either the
  writer (which clears the streak) or the replica (accepted without waiting).

The hot path stays at the one `SHOW transaction_read_only` round trip. The worst case is one
extra connect per 30s while on a replica. The window (10s) is shorter than the lake pool's 30s
`acquire_timeout`, so the first acquire of an outage succeeds rather than timing out. The derived
key-store pools (2s timeout) can return `PoolTimedOut` during those first 10s, the same as
strict mode. After that they share the elapsed streak. Rejections still emit
`pg_read_only_connection_rejected`, and `on_connect` logs `info!` when a writable connect ends a
streak.

This beats a strict pool with a short `acquire_timeout` plus a read-only fallback pool. Every
lake query takes `&PgPool` (about 160 `.db_pool` uses across 34 source files), so a two-pool
wrapper would have to thread an explicit acquire through all of them. To avoid paying the short
timeout on every acquire against a replica, it would also need the same cached state and
rate-limited re-probe. The hook version is that short timeout, applied once per outage inside a
single pool.

**Degradation on a read-only connection.** A new helper,
`is_read_only_violation(&anyhow::Error) -> bool` in `data_lake_connection.rs`, finds a
`sqlx::Error` with SQLSTATE `25006` in the error chain. Neither change below is gated on the
policy: under `Require` a `25006` only arises from a connection that turned read-only mid-query.

- **JIT partitions.** Today a failed insert (after the Parquet upload, cleaned up by
  `delete_if_orphan`) propagates out of `jit_update` and fails the scan
  (`materialized_view.rs:107`) with the raw sqlx error. That failure stays: returning the
  partitions already written would silently serve a partial result for a range that isn't fully
  materialized, which is a wrong answer, not a degraded one. On an `is_read_only_violation`
  error, `MaterializedView::scan` instead emits `warn!` +
  `imetric!("jit_update_failed_read_only", "count", 1)` and fails the scan with a clear error:
  the lakehouse is on a read-only connection, and the requested range needs partitions that
  aren't materialized yet. Every view's `jit_update` no-ops for the `'global'` instance --
  global partitions come only from the maintenance daemon -- so a query against a global view
  never reaches the write path and always succeeds on a replica; only a process-scoped instance
  needing new JIT partitions can hit this error. A query whose partitions are already
  materialized also still succeeds, since `jit_update` only writes what's missing. Serving
  un-materialized data from memory without persisting it stays out of scope.
- **API-key auth.** Today the `UPDATE ... SET last_used_at ... RETURNING` failure becomes
  `LookupError::Db`, which returns 503 (`db_api_key.rs:303-323`). On `25006`, the loader instead
  runs `SELECT <same columns> FROM <table> WHERE key_hash = $1 AND revoked_at IS NULL` and
  proceeds, skipping the `last_used_at` bump. A `pub fn is_read_only_error(&sqlx::Error)` in
  `db_api_key.rs` checks the code (`auth` doesn't depend on `ingestion`).

Admin writes (deny-list, materialize, and retire UDFs) and a lakehouse schema migration still
fail on a replica. FlightSQL on a replica therefore requires a lakehouse schema that is already
current.

### Applying it

| Site | Change |
|---|---|
| `connect_to_data_lake(policy, ..)` | `pool_options(policy).connect(db_uri)` |
| `connect_to_remote_data_lake` | `read_write_pool_options().connect(db_uri)` |
| `LakehouseContext::from_env(policy)` | forwards `policy` to `connect_to_data_lake` |
| FlightSQL builder (non-injected lakehouse) | `LakehouseContext::from_env(WritablePolicy::Prefer)` |
| maintenance daemon, `web_ingestion_service`, tests | `WritablePolicy::Require` |
| `dedicated_key_store_pool` | `lake_pool.options().clone().max_connections(4).acquire_timeout(2s).connect_lazy_with(options)` (inherits the lake pool's policy and shared fallback state) |
| `analytics-web-srv` app DB pool, analytics-keys pool | `read_write_pool_options()` (+ existing `max_connections`/`acquire_timeout` for the keys pool) |
| monolith `seed_local_data_source` | `read_write_pool_options()` |
| `MaterializedView::scan` | on `is_read_only_violation`, log, count, and fail with a clear error |
| `DbApiKeyAuthProvider` lookup | on `25006`, fall back to a `SELECT` without the `last_used_at` bump |

### Resulting behavior during a failover

```
failover drops conns ──▶ reconnect (DNS → old writer, now reader)
                          after_connect: read-only → close, backoff, re-resolve, retry
                          ...until DNS → new writer (Aurora TTL 5s) → connect OK
```

A write that arrives during the stale-DNS window waits in `acquire` instead of failing
immediately. It succeeds if DNS flips before `acquire_timeout` (30s default for the lake pool,
2s for the key-store pool), and otherwise fails with `PoolTimedOut`, which is still a 5xx that
senders retry. Readiness probes now also fail while only read-only connections are reachable,
because their `acquire` can't complete. That correctly drains the task instead of reporting it
ready while every write fails.

The lake pool keeps the sqlx default 30s `acquire_timeout`: the stale-DNS window is bounded by
the Aurora DNS TTL (~5s), which the default comfortably covers, and readiness already bounds its
own probe with a 2s `tokio::time::timeout` (`rust/ingestion/src/web_ingestion_service.rs:217`).

## Implementation Steps

1. Add `WritablePolicy`, `pool_options()`, `read_write_pool_options()`,
   `prefer_read_write_pool_options()`, `ReadOnlyFallback`, `is_read_only()`, and
   `is_read_only_violation()` to `rust/ingestion/src/data_lake_connection.rs`. Add the `policy`
   parameter to `connect_to_data_lake`, and use `read_write_pool_options()` in
   `rust/ingestion/src/remote_data_lake.rs::connect_to_remote_data_lake`.
2. Add the `policy` parameter to `LakehouseContext::from_env`
   (`rust/analytics/src/lakehouse/lakehouse_context.rs`). Pass `Prefer` from
   `rust/public/src/servers/flight_sql_server.rs:242`. Pass `Require` from
   `rust/telemetry-maintenance-srv/src/main.rs:40`, `rust/ingestion/src/web_ingestion_service.rs:277`,
   and every test caller the compiler flags.
3. In `rust/analytics/src/lakehouse/materialized_view.rs::scan`, map an
   `is_read_only_violation` error from `jit_update` to a clear error, as designed. In
   `rust/auth/src/db_api_key.rs`, add the `SELECT` fallback on `25006`.
4. Change `rust/auth/src/db_api_key.rs::dedicated_key_store_pool` to derive from
   `lake_pool.options().clone()`, and update its doc comment to say it inherits the lake pool's
   options. Reword the two call-site comments that describe this as using only the lake pool's
   "connect options" — `rust/monolith/src/main.rs:200-203` and
   `rust/public/src/servers/flight_sql_server.rs:271-273` — to say "pool options" (hooks
   included), since after this change the full pool options are cloned, not just the connect
   options.
5. Switch `rust/analytics-web-srv/src/web_server.rs:707,746` and
   `rust/monolith/src/main.rs:426` to `read_write_pool_options()`.
6. Tests (see Testing Strategy).
7. Docs and CHANGELOG.

## Files to Modify

- `rust/ingestion/src/data_lake_connection.rs`
- `rust/ingestion/src/remote_data_lake.rs`
- `rust/ingestion/src/web_ingestion_service.rs`
- `rust/analytics/src/lakehouse/lakehouse_context.rs`
- `rust/analytics/src/lakehouse/materialized_view.rs`
- `rust/telemetry-maintenance-srv/src/main.rs`
- test callers of `connect_to_data_lake` / `LakehouseContext::from_env` under `rust/*/tests/`
- `rust/auth/src/db_api_key.rs`
- `rust/analytics-web-srv/src/web_server.rs`
- `rust/monolith/src/main.rs`
- `rust/public/src/servers/flight_sql_server.rs`
- `rust/ingestion/tests/read_write_pool_tests.rs` (new)
- `rust/auth/tests/db_api_key_tests.rs`
- `mkdocs/docs/admin/service-lifecycle.md`
- `mkdocs/docs/admin/flight-sql.md`
- `CHANGELOG.md`

## Trade-offs

- **Evict on SQLSTATE `25006` instead of checking up front.** Rejected. Queries run against
  `&PgPool`, so the failing connection is never visible to the caller and can't be marked for
  closing, and sqlx has no "flush idle connections" call (`Pool::close` is permanent). Handling
  it would mean wrapping every write in an explicit `acquire()`. It would also still fail one
  write per bad connection. The `before_acquire` check prevents those failures instead.
- **Check only in `after_connect`.** That would cover the Aurora case, where demotion restarts the
  instance and drops every connection. But it leaves the pool exposed when an instance turns
  read-only without dropping connections (`default_transaction_read_only` reloaded,
  another managed-PG flavor). The `before_acquire` check costs nothing extra because it replaces
  the ping, so do both.
- **Lower `max_lifetime`/`idle_timeout`.** This only bounds the damage and churns connections all the time.
- **Upgrade sqlx.** Rejected: 0.9.0 adds no failover or session-attribute feature, and it brings a
  large breaking change (`query*()` takes `impl SqlSafeStr`, touching every `format!`-built query,
  including `db_api_key.rs`). Keep 0.8.6 for this fix and track the upgrade separately.

## Decisions

- FlightSQL prefers a writable connection but falls back to read-only; other services stay strict.
- On a read-only fallback, a query that needs un-materialized JIT partitions fails with a clear
  error rather than returning partial data.

## Documentation

- `mkdocs/docs/admin/service-lifecycle.md`: add an operational note under "Operational notes"
  saying pools reject connections to a read-only instance, so during a failover writes wait (up to the
  acquire timeout) instead of failing against the demoted writer, and readiness goes unhealthy
  while only a reader is reachable. Mention the `pg_read_only_connection_rejected` metric.
- `mkdocs/docs/admin/flight-sql.md`: reword the "Scaling" section's "Queries are read-only
  against object storage and PostgreSQL" sentence. FlightSQL prefers a writable primary, because
  it writes JIT partitions and API-key `last_used_at` through its pools. When only a read-only
  instance is reachable for 10s, it falls back to it and re-probes for a writable one every 30s.
  On a read replica, queries against global views (materialized by the maintenance daemon) keep
  working, since flight-sql never writes for those; a query that needs new JIT partitions --
  process-scoped data not yet materialized -- fails with a clear error instead of returning
  partial data. API-key auth works without updating `last_used_at`, and admin writes and
  lakehouse schema migrations fail. Mention the `pg_read_only_connection_accepted` and
  `jit_update_failed_read_only` metrics.
- `CHANGELOG.md` (Unreleased): a bug-fix entry referencing #1625. Additive Rust API:
  `read_write_pool_options`, `prefer_read_write_pool_options`, `pool_options`, `WritablePolicy`,
  `ReadOnlyFallback`, `is_read_only`, `is_read_only_violation`. **Minor breaking change**:
  `connect_to_data_lake` and `LakehouseContext::from_env` take a new leading `WritablePolicy`
  argument. `dedicated_key_store_pool`'s signature is unchanged.
  Also note that Postgres pools now require a writable primary, except flight-sql's. A service
  other than flight-sql that is pointed at a read replica now fails at startup with `PoolTimedOut`
  (the eager `PoolOptions::connect` does an initial acquire) instead of at its first write.
  Standalone flight-sql prefers a writable primary and falls back to a replica, as documented in
  `flight-sql.md`.

## Testing Strategy

No-DB unit tests:

- `is_read_only`: `"on"` → true; `"off"` → false. It should reject unknown values safely, so
  anything that isn't `"on"` counts as writable. This matches libpq, and an unexpected value
  shouldn't take the service down.
- `read_write_pool_options().get_test_before_acquire() == false`.
- `dedicated_key_store_pool` inherits the lake pool's options. `auth` doesn't depend on
  `ingestion`, so the test can't call `read_write_pool_options()` directly; instead build the lake
  pool with `PgPoolOptions::new().test_before_acquire(false).acquire_timeout(Duration::from_millis(50)).connect_lazy(...)`
  (the existing `unreachable_pool` pattern plus the one non-default option), derive from it, and
  assert that `get_test_before_acquire()` is `false` and that `max_connections`/`acquire_timeout`
  still hold 4 / 2s. sqlx's `PoolOptions::clone` copies `test_before_acquire` along with the hook
  Arcs, so the inherited flag stands in for "options were cloned" (the existing
  `dedicated_key_store_pool_is_small_and_lazy` test is the neighbor to extend).
- `ReadOnlyFallback` (driven with explicit `Instant`s, no sleeps): `on_connect` rejects
  read-only before `FALLBACK_AFTER` and accepts it at or after the window, and a writable connect
  clears the streak so the next read-only connect is rejected again. `on_acquire` keeps writable
  connections. For read-only ones, it evicts once per `PROBE_INTERVAL` (a second call inside the
  interval keeps) and evicts immediately once the streak is cleared.
- `prefer_read_write_pool_options().get_test_before_acquire() == false`.
- `is_read_only_violation`: true for a `sqlx::Error::Database` carrying SQLSTATE `25006` wrapped
  in `anyhow` context layers, and false for another code or a non-database error. The test
  implements `sqlx::error::DatabaseError` on a small fake type, since `PgDatabaseError` has no
  public constructor. `auth`'s `is_read_only_error` gets the same cases in
  `rust/auth/tests/db_api_key_tests.rs` (made `pub` for the external test crate).

Live-DB regression tests (`#[ignore]`, `MICROMEGAS_SQL_CONNECTION_STRING`), justified because
this is a bug seen in the wild. A fake can't reproduce it: the behavior depends on the
server's real `transaction_read_only` and on sqlx's real connect-retry and acquire logic.

- **New read-only connection is rejected** (pins the ~40s read-only phase): build the pool with
  `PgConnectOptions::options([("default_transaction_read_only", "on")])` and a short
  `acquire_timeout` (for example 500ms). `acquire()` must fail with `PoolTimedOut`, never hand out
  the connection.

## Manual Verification

On a staging Aurora cluster, run `aws rds failover-db-cluster` while a load generator sends
blocks through ingestion. The expected result is that the `read-only transaction` errors are
gone from the ingestion log, and any `pg_read_only_connection_rejected` warnings stop within
seconds of `Completed failover`. This isn't automated because it needs a real Aurora cluster and
its DNS behavior. The incident was triggered by a maintenance-driven failover, and a manual
failover didn't reproduce the read-only phase, so a clean run here is necessary but not
conclusive.
