# Transactional Audit-Write API Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1606

**Status**: backlog. Not scheduled. Raw-block retention depends on #1695, which waits on #1592.

## Overview

Control-plane audit records (grant, group and self-service-claim mutations, built by
`MutationAudit` in #1604) ride the tracing sink today: they are emitted after the mutation commits,
into a fire-and-forget queue that sheds load. A crash between the commit and the drain leaves a
grant created or revoked with no record of it. This adds a separate audit client that writes one
record per call straight to `/ingestion/insert_block` and returns only after ingestion acknowledges
it. Call sites write an **attempt** record before applying anything and abort with 503 if that
write fails.

The records are ordinary log entries. They go to a dedicated stream tagged `["log", "audit"]`, so
they keep landing in `log_entries` where #1604 put them. The `audit` tag is what #1695's
configurable payload retention will key on to keep their raw blocks past the default horizon.
No new view is built in. A deployment that wants
long-horizon queries defines its own materialized view over `log_entries` with the existing
`CREATE MATERIALIZED VIEW` DDL, using a new per-view `retention` option. The tracing sink is not
changed.

## Current State

### Emission

`rust/analytics-web-srv/src/mutation_audit.rs` holds `MutationAuditRecord` (flat JSON), the
`AuditOutcome` trait (implemented next to `AudienceGrantError`, `GroupsError` and
`IngestionKeyError`), the `MutationAudit` builder and the `ClientIp` extractor. `emit` ends in
`info!(target: "control_plane_audit", "{json}")` (`mutation_audit.rs:202`). There is no I/O and
no failure path.

Every audited handler is a thin wrapper around an `_inner` function. The wrapper builds the audit,
awaits `_inner` (which owns the whole DB transaction, commit included), then calls
`audit.emit(&result)`. The record is therefore always written after the commit or rollback.

| Wrapper | File:line | Transaction in `_inner` |
|---|---|---|
| `create_grant` | `audience_grants.rs:461` | begin `:566`, `FOR UPDATE` on group, commit `:592` |
| `delete_grant` | `audience_grants.rs:653` | none: autocommit `DELETE`s, 403/404 decided after |
| `create_group` | `groups.rs:269` | none: one autocommit `INSERT … ON CONFLICT` |
| `delete_group` | `groups.rs:315` | begin `:347`, commit `:391` |
| `add_member` | `groups.rs:438` | begin `:497`, commit inside the upsert loop `:582` |
| `remove_member` | `groups.rs:629` | begin `:666`, advisory lock, commit `:723` |
| claim (in `mint_key`) | `ingestion_keys.rs:490-498` | `try_claim_and_mint` begin `:615`, commit `:729` |

Gate-level denials emit their own record and return. `GrantGate` does this at
`audience_grants.rs:301`, `:318` and `:324`, and `GroupAdminGate` at `groups.rs:225`.

`mkdocs/docs/admin/control-plane-audit-log.md` documents the record as best-effort and queries it
from `log_entries WHERE target = 'control_plane_audit'`.

### Ingestion's acknowledgement

`insert_block_typed` (`rust/ingestion/src/web_ingestion_service.rs:312-427`) does two writes. It
`put_if_absent`s the re-encoded payload at `blobs/{process}/{stream}/{block}` (`PutMode::Create`),
then runs an autocommit `INSERT INTO blocks … ON CONFLICT (block_id) DO NOTHING` (`:351-375`). The
handler returns 200 only after both are awaited; either failure returns 500. `synchronous_commit`
is never overridden, so the server default (`on`) applies. A 200 therefore means a block with this
id is durable in both the object store and Postgres.

The remaining gap is the one #1647 describes. On `AlreadyExists` the stored bytes are never
compared with the submitted ones (`:382-412`), so for a client that generates random block ids
(`telemetry-sink/src/stream_block.rs:30`, `Uuid::new_v4()`), a 200 doesn't prove that *its*
payload is the one stored. OTLP closes this by deriving `block_id` as `uuid5` of the payload
(`otel-ingestion/src/identity.rs:290`).

There is no foreign key from blocks to streams or processes. A block whose stream row is missing
is accepted but invisible to every view, because views select streams by tag through the `blocks`
view (`analytics/src/lakehouse/partition_source_data.rs:267`).

Auth: ingestion requires a Bearer ingestion API key or an OIDC client-credentials token. Process,
stream and block rows are stamped with the credential's bound audience
(`public/src/servers/write_audience.rs:20-36`).

### Encoding without the dispatch

Everything needed to build and encode a log block works without the global dispatch:

- `make_process_info` (`tracing/src/dispatch.rs:1364`) is a pub, dispatch-free constructor. It
  uses `now()` and `frequency()`.
- `LogStream::new` + `get_block_mut_parts` + `LogStringInteropEvent` + `replace_block` + `close`
  is the pattern in `analytics/tests/log_tests.rs:63-82`.
- `stream_info::make_stream_info` (`telemetry-sink/src/stream_info.rs:34`), `encode_cbor`, and
  the private `encode_block` (`stream_block.rs:25-57`) do the encoding.
- Auth decorators already exist: `ApiKeyRequestDecorator::from_env`,
  `OidcClientCredentialsDecorator::from_env` and `TrivialRequestDecorator`, with selection logic
  in `TelemetryGuardBuilder::with_auth_from_env` (`telemetry-sink/src/lib.rs:397-420`).

The web server reads no ingestion URL of its own. Its telemetry goes to `MICROMEGAS_TELEMETRY_URL`
through `#[micromegas_main]`. The standalone `analytics-web-srv` has no lake connection, and the
monolith's ingestion role is a separate HTTP listener that the monolith's own telemetry already
reaches over HTTP.

### Views and retention

- `log_entries` selects every stream whose tags contain `"log"` (`log_view.rs:122`, `:179`).
  Extra tags don't exclude a stream. Its schema includes `stream_id` and `audience`
  (`log_entries_table.rs`).
- Per-process JIT instances are built from the `blocks` view's global partitions
  (`jit_partitions.rs:901-946`), not from the Postgres `blocks` table. Once those partitions
  retire, a block that is still kept in Postgres and the object store isn't reachable through any
  view.
- Materialized views defined with `CREATE MATERIALIZED VIEW` (#835, `view_definition.rs`,
  `mkdocs/docs/admin/materialized-views.md`) are stored in `lakehouse_view_set_definitions`.
  Their options live in the JSON `view_options` column (`ViewOptions`, `view_definition.rs:40`),
  and durations are parsed by `parse_time_delta` (`:63`). The registry reloads these views on a
  timer and the daemon materializes them.
- `delete_old_data` (`analytics/src/delete.rs:152-170`) applies one global `insert_time` cutoff to
  blocks and blobs, then to empty streams and processes (`NOT EXISTS` children), then to every
  `lakehouse_partitions` row (`write_partition.rs:86-135`). There is no per-stream or per-view
  exemption. `EveryHourTask` calls it (`maintenance.rs:219-227`) and already holds the daemon's
  view list.
- The cutoff is `--retention-days` / `MICROMEGAS_RETENTION_DAYS` (default 90), set in both
  `telemetry-maintenance-srv/src/main.rs:29-31` and `monolith/src/main.rs:168-170`.
- `streams` has no index on `tags` (`ingestion/src/sql_telemetry_db.rs:61-63`).

## Design

### Durability contract: reuse `insert_block`, with content-addressed block ids

There is no new endpoint. `insert_block`'s 200 already means durable in both the object store and
Postgres. The one hole is a duplicate id with different bytes, and that cannot happen if the audit
client derives `block_id` from the content: a uuid5 over the process id, stream id and encoded
payload bytes (the OTLP precedent). With that id, the same id implies the same payload, so
`AlreadyExists` can only be a retry of this exact record. "200" then means "this record is
stored", without depending on #1647. Retries are idempotent for the same reason, so the client can
retry a timed-out request without risking duplicates.

### New module: `rust/telemetry-sink/src/audit_client.rs`

This module lives next to the sink so it can reuse the `RequestDecorator`s, `make_stream_info` and
block encoding. It does not touch `HttpEventSink`, the dispatch, `SharedQueue` or any guard.

```rust
pub const AUDIT_STREAM_TAG: &str = "audit";

/// Write seam for durable audit records. `Ok` only once ingestion acknowledged the record.
#[async_trait]
pub trait AuditWriter: Send + Sync {
    async fn write(&self, target: &'static str, msg: &str) -> anyhow::Result<()>;
}

pub struct HttpAuditWriter { /* reqwest::Client, base url, Arc<dyn RequestDecorator>,
                                ProcessInfo, LogStream metadata, registered: OnceCell<()> */ }

impl HttpAuditWriter {
    /// `MICROMEGAS_TELEMETRY_URL` + the same auth selection as `with_auth_from_env`.
    /// `Ok(None)` when the URL is unset.
    pub fn from_env() -> anyhow::Result<Option<Self>>;
    pub fn new(base_url: String, decorator: Arc<dyn RequestDecorator>) -> anyhow::Result<Self>;
}
```

- **Identity.** Created once per server process. Its own `ProcessInfo` comes from
  `make_process_info(Uuid::new_v4(), parent, props)`, with `parent` set to the server's telemetry
  process id when the dispatch has one (a read only, used for correlation). It has one stream,
  tagged `["log", AUDIT_STREAM_TAG]`. `"log"` puts the records in `log_entries`, and `audit` is
  the key for #1695's retention override and lets a query tell durable records apart
  from same-target lines written through the ordinary sink.
- **Registration before the first record.** `insert_process` then `insert_stream`, both awaited,
  memoized in a `tokio::sync::OnceCell`. A failed registration leaves the cell empty, so the next
  call retries it. This ordering keeps a record from landing in a block whose stream row doesn't
  exist, which would make it invisible to every view.
- **One record per call.** Build a fresh single-event `LogBlock`: one `LogStringInteropEvent`
  (`level: Info`, `target`, `msg`, `time: now()`). Close it, encode it with the content-addressed
  id, and POST it to `/ingestion/insert_block`. `object_offset` is a per-writer `AtomicI64`, as in
  the sink.
- **Retries.** A transient failure (5xx, network, decorator error) is retried up to 3 times with
  short exponential backoff, using the sink's `tokio_retry2` classification (`http_event_sink.rs:34-63`).
  The bytes are encoded once before the loop. A 4xx fails immediately. The request timeout is 5 s,
  so a call is bounded at about 20 s worst case.
- `target` stays `control_plane_audit` and the message is the record JSON, so rows look exactly
  like today's and the documented queries keep working unchanged.

`stream_block.rs` gets a small refactor: split the private `encode_block` into building the
`BlockPayload` and assembling the `Block` from a given id. Add one pub entry point,
`encode_log_block_content_addressed(&LogBlock, &ProcessInfo) -> Result<Vec<u8>>`. The existing
`encode_bin` keeps `new_v4`.

### Record-then-apply in `analytics-web-srv`

`MutationAuditRecord` gains one field, and `outcome` gains one value:

```rust
/// Shared by the attempt record and its outcome record.
pub attempt_id: String,         // uuid v4, minted in MutationAudit::new
pub outcome: &'static str,      // "attempted" | "allowed" | "denied" | "error"
```

`MutationAudit` holds an `Arc<dyn AuditWriter>`, and its methods become async and fallible:

```rust
impl MutationAudit {
    /// Durable "attempted" record. Err -> the caller must not apply anything.
    pub async fn record_attempt(&self) -> Result<(), AuditUnavailable>;
    /// Durable outcome record for `result`, carrying the same attempt_id.
    pub async fn record_outcome<T, E: AuditOutcome>(self, result: &Result<T, E>) -> Result<(), AuditUnavailable>;
    /// Single denial/error record for a gate (no attempt record: nothing was ever going to apply).
    pub async fn record_gate_outcome(self, outcome: &'static str, reason: impl Into<String>) -> Result<(), AuditUnavailable>;
}

/// IntoResponse: 503 "control-plane audit unavailable". Nothing in the body describes the cause.
pub struct AuditUnavailable;
```

Every handler wrapper keeps the #1604 shape. Only the bracketing changes:

```rust
let audit = MutationAudit::from_context(writer, action::CREATE_GRANT, &caller, &client_ip)
    .grant(&body.audience, &body.axis, &body.selector);
audit.record_attempt().await?;                 // 503, nothing applied
let result = create_grant_inner(state, caller, body).await;
let audit = /* fold `created` in, as today */;
finish(audit, result).await
```

`finish` encodes the policy for when the outcome write fails, in one place:

| `_inner` result | outcome write fails | response |
|---|---|---|
| `Ok` (applied) | yes | the real `Ok` response. `warn!` + `control_plane_audit_outcome_write_failed` counter |
| `Err` (denied/error, nothing applied) | yes | 503 |
| any | no | the real response |

The attempt record goes **before** `_inner` starts, not inside its transaction before commit:

- No transaction holds row locks or the advisory lock across an HTTP round-trip, and no pooled
  connection is held during it.
- `delete_grant` and `create_group` need no new transaction.
- Every denial `_inner` decides, inside or outside its transaction, gets an attempt plus a
  `denied` outcome.

The only inconsistency possible is the safe one: an `attempted` record with no outcome, after a
crash or a failed outcome write. Read it as "may or may not have applied; check current state".

**Gates.**

- `GrantGate` and `GroupAdminGate` call `record_gate_outcome(...).await`. If that fails they
  reject with 503 instead of the 403 or 401, so a denial is never silently unrecorded.
- `GrantGate`'s `NotConfigured` exit (missing state extension) stays an `error` record.
- The writer reaches a gate via `parts.extensions` (see wiring below).

**Writer absent.** When `MICROMEGAS_TELEMETRY_URL` is unset, `run_web_server` installs no writer:

- The audited routes and gates answer 503 "control-plane audit not configured". The behavior
  fails closed, per the issue's contract.
- Read routes are unaffected.
- When auth is disabled these routers are not mounted at all (`key_management_disabled_router`),
  so `--disable-auth` dev setups are unaffected.
- `start_services.py` and both docker-compose files already set the URL.

`emit_record`'s `info!` path is removed, so the record is no longer duplicated into `log_entries`.

### Wiring

The writer is built once per process and shared as an `Arc`, using the same `Extension`
mechanism the admin states already use.

1. **Construction.** `run_web_server` (`web_server.rs:705`) runs in both the standalone binary
   and the monolith. It calls `HttpAuditWriter::from_env()` once, next to where it builds the
   four admin states (`:840-860`). `from_env` only reads env vars and builds the
   `reqwest::Client` and the `ProcessInfo`; it does no network I/O. Process and stream
   registration happen on the first `write`. Startup therefore doesn't depend on ingestion being
   reachable, which matters in the monolith, where the ingestion listener starts in the same
   process.
2. **Injection.** The result is wrapped as
   `#[derive(Clone)] pub struct ControlPlaneAudit(pub Option<Arc<dyn AuditWriter>>)` and stored
   in a new `AdminRoutesState::audit` field (`:376`). `build_protected_routes` adds one more
   `.layer(Extension(admin_state.audit))` in the auth-enabled branch (`:459-467`), next to the
   four states. In the `--disable-auth` branch the audited routers aren't merged, so the
   extension isn't layered there.
3. **Use.**
   - The seven handler wrappers take `Extension(audit): Extension<ControlPlaneAudit>` and pass
     the writer into `MutationAudit::new` / `from_context`.
   - `GrantGate` and `GroupAdminGate` already read `parts.extensions`. They read
     `ControlPlaneAudit` the same way, before deciding to reject.
   - A missing extension or a `None` writer both resolve to the 503 "not configured" path, in one
     helper on `ControlPlaneAudit`.

It's a separate extension rather than a field on the three states that need it, because
`GroupAdminGate` has no state of its own and one shared value keeps the three states from each
carrying a copy. The four state structs stay unchanged.

Tests build the router directly, as `mutation_audit_tests.rs` does today, and layer
`Extension(ControlPlaneAudit(Some(Arc::new(FakeWriter::new()))))`. `routing_tests.rs` passes
`ControlPlaneAudit(None)` in its `AdminRoutesState`.

### Where records land

Nothing changes on the query side. Records are log entries in `log_entries`, with
`target = 'control_plane_audit'`, as today. `AUDIT_STREAM_TAG` is defined in
`micromegas-telemetry` (which both `telemetry-sink` and `analytics` already depend on) and
re-exported by `audit_client`.

Access follows the existing audience model. Rows carry the audience of the credential the web
server ingests with, the same audience today's records carry, so readership doesn't change. A line
with the same target written through the ordinary sink, or by another holder of a key for that
audience, still lands in `log_entries`. Joining on the `audit` stream tag filters out the first
case, and the audience is the trust boundary for the second. The docs say so.

### Retention of the raw blocks

Keeping audit blocks past the default horizon is **not built here**. It is #1695: configurable
payload retention keyed by stream tag (`audit` → keep forever), stored in the DB-backed deployment
config from #1592 rather than as another env var. This plan depends on #1695 for that. Until it
lands, audit blocks follow the default horizon like every other stream. The `audit` tag is what
#1695's override will key on.

Kept blocks are the durable source of truth, but on their own they aren't queryable past the
default horizon. `log_entries` partitions retire as usual, and so do the `blocks` partitions that
JIT instances read from (see Current State). Long-horizon queries come from the next section.

### Per-view retention for materialized views

This is a general feature that the audit trail is the first user of. `CREATE MATERIALIZED VIEW`
gains a `retention` option:

| Value | Meaning |
|---|---|
| absent | the deployment default (`MICROMEGAS_RETENTION_DAYS`), as today |
| `'<n> <unit>'` | this view's partitions retire at that horizon, longer or shorter than the default; parsed by `parse_time_delta` |
| `'forever'` | this view's partitions are never retired by age |

- **Storage.** `ViewOptions` gains `retention: Option<String>`. It is stored in the existing
  `view_options` JSON column, so no migration is needed. Validate it at `CREATE` time, next to the
  delta options.
- **Exposure.** A new `View::partition_retention(&self) -> PartitionRetention` trait method with
  a default implementation returning `Default`, where
  `enum PartitionRetention { Default, Horizon(TimeDelta), Forever }`. `SqlBatchView` returns the
  value from its options. Built-in views keep the default.
- **Enforcement.** `EveryHourTask` already holds the views. It builds
  `[(view_set_name, PartitionRetention)]` for the non-default ones and passes the list to
  `retire_expired_partitions`:
  - the default pass excludes those view set names;
  - one extra pass runs per `Horizon` override, with its own cutoff;
  - `Forever` views get no pass.
  - A partition whose view set no longer has a definition, or no longer has an override, falls
    back to the default pass.
- **Caveat** (documented on the option). A view whose horizon is longer than its source's can't
  be regenerated for ranges past the source's horizon. For the audit view the recovery path
  exists: the audit blocks are still in Postgres and the object store, so regenerate `blocks` and
  then `log_entries` for the range first.

The docs for the audit trail ship an example that a deployment can run as is. Its exact SQL is
settled during implementation against `CREATE`'s validation:

```sql
CREATE MATERIALIZED VIEW control_plane_audit WITH (
  extract_query = $$
    SELECT time, insert_time, process_id, stream_id, msg, audience
      FROM log_entries
     WHERE target = 'control_plane_audit'
       AND insert_time >= '{begin}' AND insert_time < '{end}'
       AND stream_id IN (SELECT stream_id FROM blocks
                          WHERE array_has("streams.tags", 'audit')
                            AND insert_time >= '{begin}' AND insert_time < '{end}')
  $$,
  count_src_query = $$
    SELECT sum(nb_objects) AS count FROM blocks
     WHERE array_has("streams.tags", 'audit')
       AND insert_time >= '{begin}' AND insert_time < '{end}'
  $$,
  merge_partitions_query = $$ SELECT * FROM {source} $$,
  update_group = 3000,
  time_column  = 'time',
  retention    = 'forever'
);
```

## Implementation Steps

1. **Tag constant**: `AUDIT_STREAM_TAG` in `rust/telemetry/src/` (new `audit.rs` or `lib.rs`).
2. **Encoding**: refactor `telemetry-sink/src/stream_block.rs` and add
   `encode_log_block_content_addressed`.
3. **Audit client**: `telemetry-sink/src/audit_client.rs` with `AuditWriter`, `HttpAuditWriter`
   and `from_env`. Factor `with_auth_from_env`'s decorator selection into a shared
   `request_decorator_from_env()` that both use. Register the module in `lib.rs`.
4. **Block retention**: none here, see #1695.
5. **Per-view retention**:
   - `view_definition.rs`: add `ViewOptions::retention`, parsing and `CREATE`-time validation.
   - `view.rs`: add `PartitionRetention` and `View::partition_retention`.
   - `sql_batch_view.rs`: return the option.
   - `write_partition.rs`: `retire_expired_partitions` takes the override list.
   - `maintenance.rs`: `EveryHourTask` builds the list from its views.
6. **Web server**:
   - `mutation_audit.rs`: add `attempt_id` and `"attempted"`, the async `record_*` methods,
     `AuditUnavailable`, `ControlPlaneAudit` and `finish`. Remove the `info!` emission.
   - `web_server.rs`: build the writer and layer the extension.
   - `audience_grants.rs`, `groups.rs`, `ingestion_keys.rs`: rebracket the seven wrappers and both
     gates.
7. **Tests** (see Testing Strategy).
8. **Docs + CHANGELOG**.

## Files to Modify

- `rust/telemetry/src/lib.rs` (or new `audit.rs`)
- `rust/telemetry-sink/src/stream_block.rs`
- `rust/telemetry-sink/src/audit_client.rs` (new)
- `rust/telemetry-sink/src/lib.rs`
- `rust/analytics/src/lakehouse/write_partition.rs`
- `rust/analytics/src/lakehouse/view.rs`
- `rust/analytics/src/lakehouse/view_definition.rs`
- `rust/analytics/src/lakehouse/sql_batch_view.rs`
- `rust/public/src/servers/maintenance.rs`
- `rust/analytics-web-srv/src/mutation_audit.rs`
- `rust/analytics-web-srv/src/web_server.rs`
- `rust/analytics-web-srv/src/audience_grants.rs`
- `rust/analytics-web-srv/src/groups.rs`
- `rust/analytics-web-srv/src/ingestion_keys.rs`
- `rust/analytics-web-srv/tests/mutation_audit_tests.rs`
- `rust/analytics-web-srv/tests/routing_tests.rs`
- `rust/telemetry-sink/tests/audit_client_tests.rs` (new)
- `rust/analytics/tests/` (retention and view-option tests, alongside existing ones)
- `mkdocs/docs/admin/control-plane-audit-log.md`
- `mkdocs/docs/admin/materialized-views.md`
- `mkdocs/docs/admin/maintenance.md`
- `CHANGELOG.md`

## Trade-offs

**Content-addressed ids vs. waiting on #1647 vs. a dedicated endpoint.** #1647 fixes
acknowledgement for every client, but it adds a migration, a read-back on duplicates, and a 409
path. A dedicated audit endpoint would duplicate `insert_block`'s storage logic in order to promise
what it already promises. Deriving the id from the content makes the existing 200 exact for this
client today, with no server change. #1647 stays valuable for the native sink and for tamper
evidence, and doesn't conflict with this.

**HTTP to ingestion vs. an in-process `WebIngestionService` call.** In-process would skip a hop in
the monolith, but the standalone `analytics-web-srv` has no lake connection. Two write paths
would also mean two sets of failure modes to test. The monolith already sends its own telemetry
to itself over HTTP.

**Reusing `MICROMEGAS_TELEMETRY_URL` and its credential vs. dedicated audit config.** Reusing them
keeps deployments unchanged and keeps audit records in the lake and audience they land in today.
A deployment that points its own telemetry at a *different* lake would get its audit trail there
too. That is acceptable, and a dedicated URL/credential pair can be added if someone needs it.

**Attempt before `_inner` vs. a single record inside the transaction before commit.** A record
inside the transaction would be atomic with the commit decision only in the "record then crash"
direction, and it would read as `allowed` for a change that never committed. It would also hold
locks and a pooled connection across an HTTP call, and force new transactions into `delete_grant`
and `create_group`. An attempt/outcome pair keyed by `attempt_id` is honest about the in-between
state and leaves the transactions alone, at the cost of two writes per mutation.

**Records in `log_entries` vs. a dedicated hardcoded `audit_entries` view.** A block-based
`audit_entries` view, built by parameterizing `LogView` with a stream tag, would give audit records
their own partitions. But it would add another view hardcoded in `default_view_factory`, change
the SQL surface #1604 documented, and still need a hardcoded partition-retention exemption.
Keeping the records in `log_entries` changes nothing on the query side. Per-view retention on
configured views covers long horizons, and it is useful beyond audit.

## Decisions

- An outcome write that fails after `_inner` applied the change returns the real success response
  (with a `warn!` and a counter) rather than 503. Returning 503 would tell the caller a committed
  change failed.
- Gate-level denials write a single outcome record with no attempt record, because nothing was
  ever going to apply.
- Unconfigured writer (`MICROMEGAS_TELEMETRY_URL` unset) with auth enabled fails closed (503 on
  audited routes).
- No hardcoded audit view (user call): records stay log entries, on a stream tagged
  `["log", "audit"]`. Views should move toward being configured, not hardcoded.
- Raw block retention for the audit stream is deferred to #1695 (user call). It will be configured
  in the DB-backed deployment config (#1592), not through a new flag or env var.
- No migration-seeded audit view either (user call). The docs ship an example
  `CREATE MATERIALIZED VIEW` that a deployment runs if it wants long-horizon queries.

## Migration / Compatibility

- **SQL surface.** Unchanged. Records stay in `log_entries` under the same target and JSON shape.
  The new `retention` view option is additive.
- **Record JSON (additive).** New `attempt_id` key and new `"attempted"` value for `outcome`.
- **Rust API (minor breaking).** `MutationAudit`'s methods become async/fallible.
  `retire_expired_partitions` gains a parameter. `View` gains a
  defaulted method.

## Documentation

- `mkdocs/docs/admin/control-plane-audit-log.md`:
  - replace "Best-effort" with the record-then-apply guarantee and the 503 behavior;
  - document `attempt_id` and `attempted`, and how to read an unmatched attempt;
  - add a query listing attempts with no outcome;
  - explain the `audit` stream tag (filtering out same-target lines written through the ordinary
    sink) and the audience trust boundary;
  - explain that raw-block retention for the audit stream comes from #1695;
  - add the example `control_plane_audit` materialized view with `retention = 'forever'`.
- `mkdocs/docs/admin/materialized-views.md`: the `retention` option in the options table, plus
  the caveat about regenerating ranges past the source's horizon.
- `mkdocs/docs/admin/maintenance.md`: per-view `retention` overrides `MICROMEGAS_RETENTION_DAYS`
  for partitions.
- `CHANGELOG.md`: the feature, fail-closed when unconfigured, the `retention` view option, and the
  **Minor breaking change** clause for the Rust API.

## Testing Strategy

No live-DB tests: nothing here pins a bug witnessed in the wild.

**`telemetry-sink/tests/audit_client_tests.rs`**, against an in-process axum stub of the three
ingestion routes, bound on `127.0.0.1:0`. The stub records requests and returns scripted statuses.

- The first `write` sends `insert_process`, then `insert_stream`, then `insert_block`, in that
  order. A second `write` sends only `insert_block`.
- If `insert_stream` fails, the block is not sent. The next `write` retries registration.
- The decoded `StreamInfo` tags equal `["log", "audit"]`.
- The decoded block holds exactly one log event with the given target and msg, and its
  `block_id == uuid5(...)` over process, stream and payload. The same record re-sent after a 500
  carries the identical body and id.
- 503, 503, 200 returns `Ok` after three requests. 500 ×4 returns `Err`. 403 returns `Err` after
  one request, with no retry.
- The configured API key appears as `Authorization: Bearer …`.

**`stream_block.rs` unit tests**:

- `encode_log_block_content_addressed` is deterministic for the same block.
- It differs when only the msg differs.
- `encode_bin` still yields distinct ids.

**`analytics-web-srv/tests/mutation_audit_tests.rs`**, rewritten around a fake `AuditWriter`
(collecting, optionally failing on the Nth call) in place of the in-memory tracing sink. It keeps
the existing `pool: None` / lazy-pool router setup.

- Admin request with `pool: None`: records are `attempted` then `error`, sharing `attempt_id`,
  and the response is the usual `NotConfigured`. This covers all seven wrappers, the four group
  routes table-driven as today.
- Writer fails on the attempt: the response is 503, `_inner` never runs (observable because
  `pool: None` would otherwise produce `NotConfigured`), and exactly one write was tried.
- Writer fails on the outcome of an `Err` result: 503.
- `finish` with `Ok` + a failing writer returns the `Ok` response. This is a unit test on
  `finish`, since no DB-free route reaches `Ok`.
- Gate denials (both gates, all current cases) produce exactly one `denied` record with no
  attempt. With a failing writer, the gate answers 503 instead of 403/401.
- `ControlPlaneAudit(None)`: audited routes answer 503 "not configured".
- Existing serialization and classification tests stay. Add `attempt_id` presence and the
  `attempted` value.

**Analytics**:

- `retention` option parsing: absent, `'forever'`, `'90 days'`, and rejection of garbage at
  `CREATE` time.
- A definition round-trips through `view_options` JSON, and `SqlBatchView::partition_retention`
  returns the parsed value.
- The `EveryHourTask` override list skips `Default` views.
- The retention SQL builders:
  - the default partition pass excludes override view sets;
  - one pass runs per `Horizon` with its cutoff, and none for `Forever`.
  Assert on the generated statements and bound parameters, extracting the statement building
  from `write_partition.rs` into pure functions if needed.

## Manual Verification

These need real Postgres, object storage, ingestion and the daemon working together, which the
stubs above replace.

1. `python3 local_test_env/ai_scripts/start_services.py --monolith`
2. As an admin in the web app, create a group, add a member, remove it, and delete the group.
3. `micromegas-query "SELECT time, msg FROM log_entries WHERE target = 'control_plane_audit' ORDER BY time" --begin 1h`:
   expect eight rows, attempted/allowed pairs with matching `attempt_id`.
4. Stop only the ingestion listener (or point `MICROMEGAS_TELEMETRY_URL` at a closed port and
   restart), then try to create a group. Expect a 503 and no new row in `groups`.
5. Run the documented `CREATE MATERIALIZED VIEW control_plane_audit …` and wait for one
   materialization pass. `SELECT * FROM control_plane_audit` returns the same eight rows. This
   also validates the doc's example SQL.
6. Restart maintenance with `--retention-days 0`. Expect `control_plane_audit` rows to survive
   while `log_entries` empties.

## Open Questions

- **Fail-closed when unconfigured.** With auth enabled and `MICROMEGAS_TELEMETRY_URL` unset,
  grant/group/claim management goes 503 after upgrade. That is the contract the issue asks for,
  but it is an operational break. Is a CHANGELOG note enough, or do we want an explicit
  `MICROMEGAS_CONTROL_PLANE_AUDIT=disabled` escape hatch? Recommendation: no escape hatch.
