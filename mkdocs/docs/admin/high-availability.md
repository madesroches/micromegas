# Operating in a High-Availability Environment

Notes for operators running Micromegas against a highly-available Postgres
deployment (e.g. Amazon Aurora with a writer/reader topology). This page currently
covers database failover only; other HA topics may be added here later.

## Database failover

### Strict pools reject read-only connections

Every service's lake connection pool — ingestion, the maintenance daemon, and the
analytics web app's own pools — requires a writable primary. Each connection is
checked with `SHOW transaction_read_only` both when it is opened and again before
it is handed out of the pool:

- A connection that turns out to be read-only is rejected and closed; sqlx retries
  with backoff and a fresh DNS lookup.
- A pooled connection that turned read-only while idle (the old primary, demoted to
  a reader after a failover) is evicted the next time it would be handed out, and a
  new connection is opened in its place.

During the window between a failover completing and DNS catching up to the new
writer, a write **waits in `acquire`** instead of failing against the demoted
instance. It succeeds once DNS resolves to the new writer (Aurora's DNS TTL is a
few seconds), or fails with a `PoolTimedOut` error — a retryable 5xx — if the pool's
`acquire_timeout` (30s for the lake pool, 2s for the dedicated API-key-store pool)
elapses first.

Readiness goes unhealthy for these strict services for as long as only a read-only
connection is reachable, since their readiness probe's `acquire` can't complete
either — see [Readiness probes](service-lifecycle.md#readiness-probes) for the
probe/ALB mechanics this relies on. That correctly drains the task instead of
reporting it ready while every write fails.

### Standalone FlightSQL prefers a writable primary, falls back to read-only

Standalone `flight-sql-srv` (not the monolith, which shares the ingestion role's
strict lake pool) has no writer of its own to fail over to, so it degrades instead
of refusing to start or serve:

- It keeps retrying for a writable connection for **10 seconds** after the first
  read-only rejection since its last writable connect.
- After that window, it accepts a read-only connection and serves from it.
- While serving from a read-only connection, it re-probes for a writable primary
  every **30 seconds** by evicting one pooled connection; if the replacement
  connects to the writer, every connection is strict again from that point on.

Its readiness recovers once the fallback window elapses and a read-only connection
is accepted, rather than staying unhealthy for the outage's whole duration.

### What works and what fails on a read-only connection

| Query / write | On a read-only connection |
|---|---|
| Reads against a `'global'` view instance | Work — global partitions are written only by the maintenance daemon, so a query against a global view never needs the write path |
| A query whose JIT partitions are already materialized | Works — `jit_update` only writes what's missing |
| A query against a process-scoped view instance needing new JIT partitions | Fails with a clear error: the lakehouse is on a read-only connection and the requested range needs partitions that aren't materialized yet. Serving un-materialized data from memory without persisting it is out of scope |
| API-key authentication | Works, minus the `last_used_at` bump — the lookup falls back to a plain `SELECT` when the `UPDATE ... RETURNING` fails on a read-only connection |
| Admin writes (deny-list changes, materialize/retire UDFs) | Fail — these require a writable primary |
| View-set definition changes, lakehouse schema migrations | Fail — a schema migration also requires a writable primary, so FlightSQL falling back to a replica requires the lakehouse schema to already be current |

### Metrics

- `pg_read_only_connection_rejected` — a connection was rejected (or a pooled one
  evicted) for being read-only.
- `pg_read_only_connection_accepted` — a `WritablePolicy::Prefer` pool (standalone
  FlightSQL) accepted a read-only connection after its fallback window elapsed.
- `jit_update_failed_read_only` — a query needing new JIT partitions failed because
  the lakehouse was on a read-only connection.
