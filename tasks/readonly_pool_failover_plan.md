# Reject Read-Only Postgres Connections After Failover Plan

Issue: #1625

## Overview

After an Aurora PostgreSQL failover, ingestion kept writing to the demoted instance (restarted
as a reader) for ~40s after RDS reported the failover complete. Every insert failed with
SQLSTATE `25006` (`cannot execute INSERT in a read-only transaction`), and the API-key store
returned `503` because its lookup runs `UPDATE ... SET last_used_at`. The pools accept and keep
connections to a read-only instance because nothing checks for it. This plan makes every
write-capable pool refuse read-only connections, both when a connection is opened and before a
pooled connection is handed out. Once a write reaches the database it then fails only while the
cluster endpoint's DNS still points at the old writer, not until the pool recycles its
connections.

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
  - When `before_acquire` returns `Ok(false)`, the idle connection is closed and acquire moves on
    to the next idle connection or opens a new one. `Err` does the same with `close_hard()`.
- `Pool::options()` returns `&PoolOptions`, and `PoolOptions: Clone` clones the hook `Arc`s. A
  pool derived from `lake_pool.options().clone()` therefore inherits the hooks without the
  deriving crate knowing about them.
- **Upgrading to sqlx 0.9.0 doesn't help.** It adds no failover or session-attribute feature, and
  it brings a large breaking change: `query*()` takes `impl SqlSafeStr`, which touches every
  `format!`-built query, including `db_api_key.rs`. Keep 0.8.6 for this fix and track the upgrade
  separately.

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
`default_transaction_read_only` is set. A small pure helper `fn is_read_only(setting: &str) -> bool`
holds the decision, so it can be unit-tested and both hooks share it.

### Applying it

| Site | Change |
|---|---|
| `connect_to_data_lake`, `connect_to_remote_data_lake` | `read_write_pool_options().connect(db_uri)` |
| `dedicated_key_store_pool` | `lake_pool.options().clone().max_connections(4).acquire_timeout(2s).connect_lazy_with(options)`, so it inherits the hooks from the lake pool with no dependency on `ingestion` |
| `analytics-web-srv` app DB pool, analytics-keys pool | `read_write_pool_options()` (+ existing `max_connections`/`acquire_timeout` for the keys pool) |
| monolith `seed_local_data_source` | `read_write_pool_options()` |

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

1. Add `read_write_pool_options()` and `is_read_only()` to
   `rust/ingestion/src/data_lake_connection.rs`, and use the new function in `connect_to_data_lake` and
   `rust/ingestion/src/remote_data_lake.rs::connect_to_remote_data_lake`.
2. Change `rust/auth/src/db_api_key.rs::dedicated_key_store_pool` to derive from
   `lake_pool.options().clone()`, and update its doc comment to say it inherits the lake pool's
   options.
3. Switch `rust/analytics-web-srv/src/web_server.rs:707,746` and
   `rust/monolith/src/main.rs:426` to `read_write_pool_options()`.
4. Tests (see Testing Strategy).
5. Docs and CHANGELOG.

## Files to Modify

- `rust/ingestion/src/data_lake_connection.rs`
- `rust/ingestion/src/remote_data_lake.rs`
- `rust/auth/src/db_api_key.rs`
- `rust/analytics-web-srv/src/web_server.rs`
- `rust/monolith/src/main.rs`
- `rust/ingestion/tests/read_write_pool_tests.rs` (new)
- `rust/auth/tests/db_api_key_tests.rs`
- `mkdocs/docs/admin/service-lifecycle.md`
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
- **Shared helper crate vs `ingestion`.** `auth` can't depend on `ingestion`, but it doesn't have to:
  `Pool::options().clone()` carries the hooks, so a single definition in `ingestion` covers every
  derived pool without a new crate.
- **Upgrade sqlx.** It doesn't help (see Current State).

## Documentation

- `mkdocs/docs/admin/service-lifecycle.md`: add an operational note under "Operational notes"
  saying pools reject connections to a read-only instance, so during a failover writes wait (up to the
  acquire timeout) instead of failing against the demoted writer, and readiness goes unhealthy
  while only a reader is reachable. Mention the `pg_read_only_connection_rejected` metric.
- `CHANGELOG.md` (Unreleased): a bug-fix entry referencing #1625. The only Rust API change is
  additive (`read_write_pool_options`); `dedicated_key_store_pool`'s signature is unchanged.
  Also note that every service's Postgres pools now require a writable primary: a service pointed
  at a read replica (for example flight-sql, which writes JIT partitions through its lake pool via
  `LakehouseContext::from_env` → `connect_to_data_lake` → `migrate_lakehouse`) now fails at
  startup with `PoolTimedOut` (the eager `PoolOptions::connect` does an initial acquire) instead of
  failing at its first write.

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
