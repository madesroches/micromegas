# UDTF/UDF Audience Guards — Query Enforcement Prong B Plan (#1371 — AbAC Stage 3)

## Overview

Stage 2 (#1370) landed `OwnershipRewrite`, the analyzer rule that injects an audience predicate
into every `MaterializedView`-backed scan. It structurally cannot reach the span/metadata
table-valued functions: they bake their target id into a provider at plan time, return schemas with
no `process_id` column to filter on, and — worse — three of them build their *own* inner session
context under `CallerContext::internal()` (`ReadScope::All`), which makes `OwnershipRewrite` a
no-op inside them. Today a restricted, authenticated caller can read any process's spans, any
block's decoded objects, any block's raw payload, and the full partition inventory of every
audience, completely unfiltered. Stage 3 closes that: an **arg-addressed guard** on
`process_spans`, `perfetto_trace_chunks`, `parse_block`, and the `get_payload` UDF, **row filtering**
on `list_partitions`, and the two remaining registration arms (maintenance context, opt-in knob) for
the five mutating functions. All of it fed by one new, size- and TTL-bounded cache
resolving *any telemetry id → its owning process's audience* from Postgres.

Like Stages 1, 2 and 4, this is inert by default: every guard is a no-op under `ReadScope::All`, and
a deployment with no isolation config still resolves `ReadScope::All` for every request.

## Current State

### The four read surfaces Prong A cannot reach, verified

| Surface | Registration | Target id | Why Prong A misses it |
|---|---|---|---|
| `process_spans(process_id, types)` | `query.rs:142` | process_id, arg 1 (`process_spans_table_function.rs:105-118`) | output schema has `stream_id`/spans columns, no `process_id`; its provider's plan runs an inner session under `ReadScope::All` (`process_spans_table_function.rs:254-264`) |
| `perfetto_trace_chunks(process_id, types, begin, end)` | `query.rs:125` | process_id, arg 1 (`perfetto_trace_table_function.rs:65-75`) | output is `(chunk_id, chunk_data)` protobuf blobs; inner session under `ReadScope::All` (`perfetto_trace_execution_plan.rs:254-266`) |
| `parse_block(block_id)` | `query.rs:133` | block_id, arg 1 (`parse_block_table_function.rs:253-268`) | output is `(object_index, type_name, value)`; inner session under `ReadScope::All` (`parse_block_table_function.rs:83-92`) |
| `get_payload(process_id, stream_id, block_id)` | `query.rs:151` | process_id, arg 1 (`get_payload_function.rs:75-118`) | not a table scan at all — an `AsyncScalarUDF` that reads `blobs/{process_id}/{stream_id}/{block_id}` straight out of object storage, bypassing the lakehouse entirely |

`list_partitions()` (`query.rs:117`, `list_partitions_table_function.rs`) is a fifth surface of a
different kind: it `SELECT`s `lakehouse_partitions` verbatim from Postgres
(`list_partitions_table_function.rs:104-170`) and exposes a `view_instance_id` column that is "a
process_id, a stream_id or 'global'" (`view.rs:56`) — leaking the existence, size and timing of
every other audience's data.

`list_view_sets()` stays unfiltered (decided in the parent plan): schema/definitions only, no
per-principal data.

### What is already done, and what is left of the mutating-function gate

#1382 (`tasks/completed/admin_gate_mutating_lakehouse_functions_plan.md`) already gates all five
mutating functions on `caller.is_admin` (`query.rs:154-179`) — the parent plan's `is_admin` arm
(#1377). Two arms remain:

- **maintenance contexts** — already covered incidentally: `CallerContext::maintenance()` sets
  `is_admin: true`, `internal()` sets `false` (deliberately: internal callers that must not get the
  mutating functions). No `ReadScope::All`-based arm needs adding; the current shape is the intended
  one, and this plan records that rather than adding a redundant condition.
- **`MICROMEGAS_USER_MAINTENANCE_FUNCTIONS`** — not implemented, deferred to this stage by #1382 on
  purpose. It is still wanted, and not as a loosening for its own sake: API keys can never be admin
  (`api_key.rs:124`), so an **API-key-only deployment has no admin principal at all** and lost
  access to `retire_partitions`/`materialize_partitions`/… outright in #1382. This knob is that
  deployment's way back.

### Where an audience actually lives

`micromegas.audience` is a plain property on the process. Its **origin** is Postgres
`processes.properties` (`micromegas_property[]`, i.e. `(key TEXT, value TEXT)` composite rows —
`sql_telemetry_db.rs:15-45`), written at process insert. Prong A does **not** read that origin: it
reads a *snapshot* of it, `property_get(properties,'micromegas.audience')` off
`__processes__partitions` (`ownership_rewrite.rs:140-167`), because `BlocksView::data_sql` copies
`processes.properties` into the `blocks` parquet partitions at materialization time and the
`processes` `SqlBatchView` derives from those (see `ownership_rewrite_db_test.rs`'s module comment).
That is why Stage 2's own CHANGELOG entry has to declare a running maintenance daemon as an
undeclared precondition: a process the daemon has not materialized yet contributes no row to the
aggregate and is invisible to everyone, including its owner.

Prong B has no reason to inherit that. It resolves audiences from **Postgres**, by primary-key point
query — authoritative, fresh, and independent of materialization. §11 covers the consequences of the
two prongs reading different copies.

Also relevant: `streams.process_id` is indexed (`sql_telemetry_db.rs:48-88`), so
`stream_id → owning process` is one indexed point query. `blocks.process_id` itself is not indexed,
but the block lookup doesn't need it: `block_id → owning process` is one indexed point query via the
unique `blocks_block_id_unique` index (`sql_migration.rs:263`), joined to `processes` by its unique
`process_id` index — no join through the lakehouse needed either way.

### Existing pieces to reuse

- `ReadScope` / `CallerContext` / `OwnershipRewriteConfig` (`read_scope.rs`), already threaded into
  `register_lakehouse_functions(…, caller: &CallerContext)` (`query.rs:100-107`). **The scope
  already reaches Prong B's registration site** — Stage 1 did that plumbing. Nothing in
  `rust/public` needs to change for the guards.
- `moka::future::Cache` + the size-bounded, metric-emitting cache shape of `MetadataCache`
  (`metadata_cache.rs`), and its home on `LakehouseContext` (`lakehouse_context.rs:22-85`) — the
  per-service object every UDTF already holds an `Arc` to.
- `Uuid` canonicalization precedent: `OwnershipRewrite::canonical_view_instance_id`
  (`ownership_rewrite.rs:277-290`).
- Offline (no live DB) session-context fixture: `lakehouse_admin_gate_test.rs:20-32`
  (`connect_lazy` + `InMemory` object store). DB-backed audience-seeding fixture:
  `ownership_rewrite_db_test.rs`.

## Design

### 1. `AudienceIndex` — one cache, one question

New module `rust/analytics/src/lakehouse/audience_guard.rs`. The index answers exactly one
question, for three id kinds:

> Which audience is stamped on the process that owns this id?

```rust
/// The process property carrying the data-isolation audience. Single definition, shared with
/// `OwnershipRewrite` (which currently inlines the literal at `ownership_rewrite.rs:148`).
pub const AUDIENCE_PROPERTY: &str = "micromegas.audience";

/// What a resolution attempt found. `Unknown` is *not* `Unstamped`: an id with no row at all
/// is always denied, while an unstamped process is subject to the `unstamped_audience` knob.
/// "No row" covers both "not yet ingested" and "retention already deleted it" (§11) — both
/// deny, on the same fail-closed reasoning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerAudience {
    Unknown,
    Unstamped,
    Audience(Arc<str>),
}

/// Which table resolves the id to its owning process.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum IdKind {
    Process,          // processes.process_id
    Block,            // blocks.block_id       -> processes
    ProcessOrStream,  // list_partitions' view_instance_id: either, resolved in one round trip
}

pub struct AudienceIndex {
    pool: sqlx::Pool<sqlx::Postgres>,
    cache: moka::future::Cache<(IdKind, Uuid), OwnerAudience>,
}

impl AudienceIndex {
    pub fn new(pool: sqlx::Pool<sqlx::Postgres>, max_entries: u64, ttl: std::time::Duration) -> Self;
    pub async fn resolve(&self, id: Uuid, kind: IdKind) -> anyhow::Result<OwnerAudience>;
    pub async fn resolve_many(
        &self,
        ids: &[Uuid],
        kind: IdKind,
    ) -> anyhow::Result<HashMap<Uuid, OwnerAudience>>;
}
```

**One cache for every kind, keyed by `(IdKind, Uuid)`, not by the bare `Uuid`.** A UUID is
client-supplied at ingestion for all three tables (`ProcessInfo.process_id`, and stream/block
registration — `web_ingestion_service.rs`) and no constraint spans `processes`, `streams` and
`blocks`, so nothing stops the *same* UUID from being a process id in one audience and a stream id
or block id in another. Keying on the bare `Uuid` would let a cache entry populated for one kind
answer a lookup of a different kind for a colliding id — not "returns nothing downstream" but a
genuine cross-audience read, because §6 runs the inner query under `Authorized::internal_caller()`
(`ReadScope::All`): a wrong-kind hit that resolves to a *readable* audience would authorize the
guard, and the inner, unscoped session would then return the colliding id's real data (e.g.
`get_process_thread_list`/`get_process_exe`, `process_streams.rs:9-21`,
`perfetto_trace_execution_plan.rs:302-317`) to a caller who was never granted that audience. Keying
on `(IdKind, Uuid)` removes the collision instead of relying on downstream emptiness: `Process` and
`Block` each own a disjoint slice of the keyspace, and `ProcessOrStream` (`list_partitions`' usage) is
deliberately its own key too rather than reusing `Process` entries or caching the `streams` table
under its own kind, so its `UNION ALL` result is what gets cached. If both arms of that `UNION ALL`
return a row for the same id (only possible for a `process_id`/`stream_id` collision), take the
`processes` row: each arm is tagged with a source-discriminator column (`'process'`/`'stream'`) and
the process-wins precedence is applied deterministically in Rust while collapsing the rows into the
cached `OwnerAudience`, never inferred from `UNION ALL` result order — PostgreSQL does not guarantee
that order (e.g. Parallel Append may interleave the branches), so a caller who names a colliding id
gets the process's audience, not the stream's, on every run. One cache, two disjoint keyspaces plus one
derived one, instead of the parent plan's three caches.

**Bounded by entry count and by a TTL — no other invalidation.** `process_id → properties` is
written once at process insert and never updated in place (no `UPDATE processes` exists anywhere in
the tree); `streams.process_id` and `blocks.process_id` are fixed at creation. But the row is not
immutable across the process's *lifetime*: `delete_old_data` (`delete.rs:151-170`) deletes `blocks`
→ `streams` → `processes` rows once retention expires them, and ids are client-supplied — for OTLP, a
deterministic UUIDv5 derived from resource attributes (`otel-ingestion/src/identity.rs:230,236`,
`NS_OTEL_PROCESS_V1`) — so a routine retention-then-re-export cycle recreates the *same* `process_id`
under fresh `properties`, and nothing ever `UPDATE`s the stale cache entry to match. An entry-count
bound alone does not help here: `max_capacity` only evicts under size pressure, which may never occur
at 100k entries, so a stale entry can otherwise sit forever. So the cache also carries
`time_to_live(entries_ttl)` (default `5m`) — `AudienceIndex`'s only freshness mechanism, cheap because
a miss is one indexed point query — bounding how long a re-derived process's audience can serve a
stale answer; `max_capacity(entries)` (default `100_000`, one `Uuid` + short string ≈ 100 B ⇒ ~10 MB)
remains the size bound. Mirrors `MetadataCache`'s `imetric!` entry-count reporting.

**`Unknown` is never cached.** A miss means "no such row *yet*" — the process may be mid-ingestion —
and caching it would both pin a wrong answer and let a caller pollute the cache with random UUIDs.
Cost of not caching: one indexed point query per denied lookup.

The SQL, for `IdKind::Process` (`Block` swaps the driving table and joins through `processes`;
`ProcessOrStream` is a `UNION ALL` of this process-id shape with the analogous stream-id shape, one
round trip, each arm tagged with a literal `source` column):

```sql
SELECT p.process_id AS id, a.value AS audience, 'process' AS source
FROM processes p
LEFT JOIN LATERAL (
    SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1
) a ON TRUE
WHERE p.process_id = ANY($1)
```

`LEFT JOIN LATERAL` (not an inner `unnest` in the `FROM` list) is what keeps *unstamped* rows in the
result: an inner unnest silently drops them, collapsing `Unstamped` into `Unknown` and turning the
`MICROMEGAS_UNSTAMPED_AUDIENCE` escape hatch off for exactly the rows it exists for. `LIMIT 1`
handles a duplicated property key; `= ANY($1::uuid[])` is what makes `resolve_many` one query. Each
id resolves to at most one row per arm — `process_id`, `stream_id` and `block_id` are all unique
since migration v3 (`sql_migration.rs:250-268`) — but `ProcessOrStream`'s two arms can each return a
row for the same id (the collision case above), so PostgreSQL may return them in either order and
even interleaved (`UNION ALL` carries no ordering guarantee, and Parallel Append can run both arms
concurrently). `resolve_many` never relies on that order: it groups the returned rows by `id` in
Rust and, for any id with a row from both arms, keeps the `source = 'process'` row and discards the
`'stream'` one — the same process-wins precedence stated above, applied deterministically regardless
of how Postgres ordered the result set.

### 2. `AudienceGuard` — pure decision, async resolution, fail-closed

```rust
pub struct AudienceGuard {
    read_scope: ReadScope,
    unstamped_audience: Option<String>,
    public_view_sets: Vec<String>,   // list_partitions' 'global'-row rule only (§8)
    index: Arc<AudienceIndex>,
}

/// Pure, offline-testable: the whole authorization rule, with no I/O in it.
pub fn is_readable(
    scope: &ReadScope,
    unstamped_audience: Option<&str>,
    owner: &OwnerAudience,
) -> bool {
    match scope {
        ReadScope::All => true,
        ReadScope::Audiences(auds) => match owner {
            OwnerAudience::Unknown => false,
            OwnerAudience::Unstamped => unstamped_audience
                .is_some_and(|u| auds.iter().any(|a| a == u)),
            OwnerAudience::Audience(a) => auds.iter().any(|x| x.as_str() == &**a),
        },
    }
}
```

Splitting the rule out as a free function keeps every branch of it unit-testable with no database
(§Testing), and makes the `ReadScope::All` short-circuit explicit in one place.

The guard's fallible surface:

```rust
impl AudienceGuard {
    /// `ReadScope::All` ⇒ `Ok` with no I/O at all.
    pub async fn authorize(&self, id: Uuid, kind: IdKind, fname: &str)
        -> datafusion::error::Result<Authorized>;

    pub async fn readable_ids(&self, ids: &[Uuid], kind: IdKind)
        -> datafusion::error::Result<HashSet<Uuid>>;

    pub fn global_rows_visible(&self, view_set_name: &str) -> bool;   // §8
}

/// Witness that some id's owning process was resolved and found readable under the caller's
/// scope. Constructible only by `AudienceGuard::authorize`.
pub struct Authorized { id: Uuid }

impl Authorized {
    pub fn id(&self) -> Uuid;
    /// The `CallerContext` an inner, post-authorization session may run under (§6).
    pub fn internal_caller(&self) -> CallerContext { CallerContext::internal() }
}
```

A resolution *error* (Postgres unreachable) is a denial, not a pass: `authorize` maps it to
`DataFusionError::External` and the query fails. There is no branch in which a failed lookup yields
a readable verdict.

### 3. Where the check runs: async `scan`, not `call_with_args`

The parent plan assumed the check had to be deferred into the execution plan because
`call_with_args` is synchronous. It does not have to go that far: **`TableProvider::scan` is
`async`** for all three UDTFs (`process_spans_table_function.rs:384`,
`perfetto_trace_execution_plan.rs:570`, `parse_block_table_function.rs:296`), and it runs before any
data is touched. That is the check's home — an error there surfaces as a planning/execution failure
with no partial stream, and no `ExecutionPlan::execute` (sync, stream-returning) contortion.

`call_with_args` still gains one thing: **parsing the id argument into a `Uuid`** and keeping the
canonical hyphenated rendering (`Uuid::hyphenated`), so a malformed id is a plan-time error. This is
a free side benefit: `process_id` is currently interpolated verbatim into inner SQL
(`process_streams.rs:9-20` `get_process_thread_list`, `perfetto_trace_execution_plan.rs:305-317`
`get_process_exe`), so today a crafted argument is a SQL-injection vector into the *inner* DataFusion
query. Parsing it as a `Uuid` at plan time closes that, and matches what
`parse_block_table_function.rs:299` already does for its block id.

**The witness must survive `scan` → `execute`.** `process_spans` and `perfetto_trace_chunks` do their
work inside `ExecutionPlan::execute`, so the plan returned by `scan` carries the witness:

```rust
struct ProcessSpansExecutionPlan { /* ... */ authorized: Option<Authorized> }
```

`call_with_args` builds it with `None`; `scan` awaits `guard.authorize(...)` and returns a clone
holding `Some(...)`; `execute` returns `DataFusionError::Internal("unauthorized plan")` on `None`.
An execution plan that never went through `scan` cannot run — fail-closed by construction rather
than by comment.

### 4. `get_payload` — the process_id argument is the whole check

`get_payload(process_id, stream_id, block_id)` builds `blobs/{process_id}/{stream_id}/{block_id}`
and reads it (`get_payload_function.rs:99-118`). Checking **arg 1** is therefore sufficient and
complete: a caller who names a readable process cannot reach another process's blob, because the
foreign block simply is not under that prefix. The parent plan's `block_id → process_id` chain for
this function is unnecessary — the process id is right there in the call.

In `invoke_async_with_args`, collect the **distinct** `process_ids` values, parse each with
`Uuid::parse_str`, and deny the whole call under `ReadScope::Audiences` if any value fails to parse
(the one input that could otherwise take `read_blob` outside the `blobs/{process_id}` prefix the
completeness argument relies on). Then `resolve_many` the parsed ids in one query, and fail the whole
call if any is unreadable (never per-row `NULL`s: a partially-filtered binary column would be
indistinguishable from a missing payload).

### 5. `parse_block` — `IdKind::Block`

`parse_block(block_id)` gets the only real id-chase: `blocks.block_id → blocks.process_id →
processes.properties`, one indexed point query, cached like the rest. Guard in `scan`
(`parse_block_table_function.rs:296`) **before** `fetch_block_metadata`, then run the metadata fetch
under the witness's internal caller (§6).

### 6. The three inner contexts: guard-then-authorized-internal (a deliberate deviation from the parent plan's §5)

The parent plan's §5 says the three user-reachable recursive context sites "must **inherit the
caller's `ReadScope`**, never `All`, or they become bypasses". Inheriting is the wrong call here, and
this plan deviates deliberately:

- **Inheriting would add one more daemon-freshness hop, on top of one every deployment already
  has.** `get_process_thread_list` (`process_spans`' inner query) and `fetch_block_metadata`
  (`parse_block`'s) both read the `blocks` global view, whose `jit_update` is already a no-op —
  it is populated only by the maintenance daemon's pass (`blocks_view.rs:158-168`) — so a process
  ingested since the daemon's last pass already produces an empty flame graph or trace today; that
  is the known limitation Stage 2's CHANGELOG already flags, unchanged by this stage either way.
  Both run under `CallerContext::internal()` (`ReadScope::All`) today, where `OwnershipRewrite`'s
  analyzer pass is a no-op on that `blocks` scan, so the read needs only `blocks` materialized.
  Inheriting the caller's scope instead would activate `OwnershipRewrite`'s `process_id` semi-join
  against `__processes__partitions` on that same scan (`blocks` is one of the rewritten tables,
  §4) — a second, independent daemon-populated dependency that is not there today: a process whose
  `blocks` partitions are materialized but whose `__processes__partitions` are not yet would newly
  fail `process_spans`/`parse_block`, on top of the gap above. `perfetto_trace_chunks`'
  `get_process_exe` does not have this problem to create, because it already has it
  unconditionally: it reads the `processes` named table (its own `SqlBatchView`, no-op
  `jit_update`, `sql_batch_view.rs:306-312`) under `CallerContext::internal()` today too, so in
  that same window it is already empty regardless of scope — inheriting would not make it "newly"
  empty, and the guard reading Postgres does not change that outcome either, since the failure is
  a materialization gap in the inner session's own read, not an authorization decision.
- **The guard is the stronger check anyway.** It reads Postgres, so it is both fresher and
  independent of the maintenance daemon.
- **The inner SQL is server-constructed and confined to the guarded process.** Every inner statement
  is a format string our own code writes, keyed on the id the guard just authorized
  (`process_streams.rs:9-20`, `perfetto_trace_execution_plan.rs:305-317`,
  `parse_block_table_function.rs:96-101`), and audience is a per-process property — so if the process
  is readable, everything those statements can reach is readable. There is no caller-supplied SQL
  inside these contexts.

So: **authorize first, then run the inner session under `Authorized::internal_caller()`**, and give
each of the three sites a doc comment stating the invariant it relies on (server-constructed SQL,
confined to `Authorized::id()`). The witness type is what keeps that honest — none of the three sites
can obtain a context without a resolved, readable verdict in hand, so a future edit that adds a
fourth such UDTF cannot silently reproduce today's bypass by calling
`CallerContext::internal()`. (Not airtight: `internal()` stays public for genuinely non-user-reachable
callers. It converts a silent omission into a visible, reviewable choice.)

Each of the three `TODO(#1371)` comments is replaced by that rationale.

### 7. `metadata.rs`'s two contexts stay `ReadScope::All`, with the reason recorded

`find_stream_from_view` (`metadata.rs:182-191`) and `find_process_with_latest_timing`
(`metadata.rs:286-295`) carry `TODO(#1371)` markers saying their `ReadScope::All` is a latent
bypass. Analysis says otherwise, and this plan resolves the TODOs by recording that instead of
threading a scope through the `View` trait:

- Both are called only from `jit_update` implementations (`thread_spans_view.rs:358,367`,
  `net_spans_view.rs:330`, `async_events_view.rs:130`) — the JIT materialization step
  `MaterializedView::scan` runs (`materialized_view.rs:70`) before the caller's own scan.
- Neither returns rows to the caller. They return stream/process *metadata* used to build a
  partition of the very view instance the caller named, and the caller's read of that partition is
  filtered by Prong A (`async_events`/`thread_spans` §5/§6 `EXISTS`).
- Threading a scope there means putting a `CallerContext` on the `View` trait's `jit_update` and on
  `MaterializedView` — ~10 impls — to gate reads that produce no caller-visible output.

**Residual, accepted:** a caller can still *trigger* JIT materialization of a view instance they
cannot read (compute and storage cost, and any error text that materialization surfaces). That is an
availability/cost issue, not a confidentiality one, and it is a property of `MaterializedView::scan`
rather than of these two functions — the right place to fix it is a scan-time guard on
`view_instance` in a later stage, not a scope parameter here. Recorded in Trade-offs and worth a
follow-up issue.

### 8. `list_partitions` row filtering

Row kinds, keyed on the `view_instance_id` value itself (no `ViewFactory` lookup needed, so retired
or SQL-defined view sets are handled uniformly):

| `view_instance_id` | Rule |
|---|---|
| parses as a `Uuid` | `IdKind::ProcessOrStream` resolution; keep iff `is_readable(...)` |
| `'global'` (the literal, `view_factory.rs:6`) | keep iff `ReadScope::All`, **or** the row's `view_set_name` is in `public_view_sets`, **or** `unstamped_audience` is in the caller's scope |
| anything else | drop (fail-closed; nothing produces such a value today) |

The `'global'` rule is the parent plan's §4 rule verbatim: a global aggregate has no single audience,
so it is treated like unstamped data — an open deployment with `MICROMEGAS_UNSTAMPED_AUDIENCE=public`
keeps its global rows visible (today's behavior), a privacy deployment leaves the knob unset and
they stay hidden.

Implementation, in `ListPartitionsTableProvider::scan`
(`list_partitions_table_function.rs:104-170`):

1. `ReadScope::All` ⇒ unchanged path, including the `LIMIT` pushdown.
2. Otherwise: **do not push `LIMIT` to Postgres.** Filtering after a pushed-down limit would return
   fewer rows than a client asked for while more matching rows exist — silently wrong results.
   Fetch, filter, then truncate to `limit` in Rust.
3. Collect the distinct `view_instance_id`s that parse as `Uuid`, `resolve_many` them in one round
   trip (`IdKind::ProcessOrStream`), then build the output `RecordBatch` from the kept rows.

Filtering the `RecordBatch` after `rows_to_record_batch` would mean a `take` kernel over 15 columns;
filtering the `sqlx` row vector *before* `rows_to_record_batch` is simpler and cheaper, and keeps the
schema construction untouched. Do it there — **except when the filter empties the row vector**:
`rows_to_record_batch` maps an empty slice to `make_empty_record_batch()`
(`sql_arrow_bridge.rs:371-374`), which builds a **zero-field** struct (`arrow_utils.rs:14-18`), not
one matching `ListPartitionsTableProvider::schema()`'s 15 columns. That mismatch exists today only
on a genuinely empty `lakehouse_partitions` table; under this filter it becomes the steady state for
any `ReadScope::Audiences` caller with no readable partitions. Guard it: if the filtered row vector
is empty, build the empty batch directly from `ListPartitionsTableProvider::schema()`
(`RecordBatch::new_empty`) instead of calling `rows_to_record_batch`.

### 9. The mutating-function registration knob, and where its config lives

Gate becomes `caller.is_admin || config.user_maintenance_functions` (`query.rs:154`). No
`ReadScope::All` arm — see Current State: `maintenance()` already implies `is_admin`, and
`internal()`'s exclusion is deliberate.

`MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` (`{prefix}_…` with an unprefixed fallback, like every other
knob) is a per-service deployment value, so it belongs on the object that already carries
`unstamped_audience`/`public_view_sets` — but that object is named `OwnershipRewriteConfig`, and a
UDTF-registration knob has nothing to do with the rewrite. **Rename it to `IsolationConfig`**, with
`CallerContext.isolation_config`, `FlightSqlServiceImpl::new`'s parameter, and
`FlightSqlServerBuilder::with_ownership_config()` → `with_isolation_config()` following. Rust API
churn is explicitly cheaper than a misleading name in this codebase (`CLAUDE.md` §Interface
stability), and the compiler enumerates all ~6 construction sites (three servers, three test files).
**Decided: rename, not an additive field** — `CLAUDE.md` §Interface stability states the Rust API
surface may change freely, that "a clean design beats a compatible one", and that the preferred
shape is the one that makes the compiler enumerate every affected call site; the rename touches only
Rust construction sites, none of them SQL-layer surface. Parse the knob with the existing
`resolved_var` helper (`read_scope.rs:151-162`); accept exactly `true`/`false` (case-insensitive) and
`Err` on anything else, matching the fail-fast posture of the other two knobs.

**Carried caveat from the parent plan (§4): the knob is deployment-wide, not per-audience.** None of
the five mutating functions carries an audience filter — `retire_partitions` takes an arbitrary
`(view_set_name, view_instance_id)` pair (`query.rs:155-158`) — so once
`MICROMEGAS_USER_MAINTENANCE_FUNCTIONS=true` is set, *any* authenticated caller can retire or
materialize partitions belonging to *any* audience, not just their own. This is fine for the
API-key-only deployment the knob is meant for (no admin principal exists at all, so the alternative
is no access to these functions for anyone); it stops being fine the moment that deployment also has
personal or per-team audiences, where it hands every user destructive access to every other user's
data. Tighten to per-audience checks if such a hybrid deployment becomes real; out of scope here.

### 10. Denial is indistinguishable from absence

Every guard denial and every "no such id" produces the **same** error text, e.g.:

```
process_spans: process '…' not found or not accessible
```

A distinct "permission denied" would be an existence oracle: a caller could enumerate which process
ids exist in other audiences. `DataFusionError::Plan` classifies through
`classify_datafusion_error`/`client_error` (`flight_sql_service_impl.rs:196-229`) to the same status
code a malformed id already gets, which is the point. The *server* log records the real reason
(`debug!` with the id, the resolved `OwnerAudience`, and the caller's scope) so an operator can tell
the two apart; the client cannot.

`list_partitions` denials are silent by construction — rows are simply absent, exactly like Prong A's
predicate.

### 11. Two prongs, two copies of the audience — accepted, with the direction stated per lifecycle stage

Prong A reads the daemon-materialized parquet snapshot; Prong B reads Postgres, the origin. **For
live (not-yet-retained) data, Prong B is fresher and at least as accurate — never more permissive
than the ground truth in Postgres:**

- A caller's own just-ingested process: `process_spans`/`perfetto_trace_chunks`/`parse_block`/
  `get_payload` work (Prong B allows, correctly); the equivalent plain-SQL query over
  `log_entries`/`thread_spans` returns nothing until the daemon catches up (Prong A's known
  limitation, unchanged by this stage).
- A foreign process: denied by both.

**That direction flips once retention has run.** `delete_old_data`
(`EveryHourTask::run` → `delete_old_data`, `delete.rs:151-170`) deletes `blocks` rows with
`insert_time <= expiration`, then empty `streams`, then empty `processes` — Postgres forgets the
process entirely. `retire_expired_partitions` (`write_partition.rs:86-135`) in the same pass only
deletes lakehouse partitions with `end_insert_time < expiration`: a partition whose insert range
extends past the boundary (a merged/compacted partition in particular) survives with its snapshot of
`micromegas.audience` intact. In that window, `OwnerAudience::Unknown` means "no such row *any
more*", not just "not yet" — Prong B denies `process_spans`/`perfetto_trace_chunks`/`parse_block`/
`get_payload`/`list_partitions` for a process whose Prong-A-filterable partition data an owner can
still query directly, permanently, with no path back once the Postgres row is gone.

The cache has a parallel, narrower residual on the same delete-then-recreate cycle: a
`(IdKind::Process, Uuid)` entry populated *before* deletion keeps answering with the pre-deletion
`OwnerAudience` — now stale — until the cache's `time_to_live` (§1) elapses, not just until the next
Postgres lookup. That is exactly why the cache is TTL-bounded rather than invalidation-free (§1): with
only an entry-count bound, an entry that never gets evicted (a real possibility at 100k capacity)
would serve a stale audience forever instead of for one bounded window.

**Decision: accept the denial, matching the plan's fail-closed posture everywhere else** (Security:
`Unknown` ⇒ deny is stated as unconditional for a fresh, uncached resolution — the cache's TTL bounds
how long a stale cached verdict, allow or deny, can outlive the Postgres row it was read from).
Falling back to the parquet snapshot when Postgres has
no row would mean trusting an *un-refreshable* audience — the whole reason Prong B reads Postgres
instead of the snapshot (§1) is that the snapshot can go stale in the safe direction (missing rows,
never wrong ones) but a snapshot with no live source of truth behind it can never be corrected if the
stamp was ever wrong, which is exactly the property Prong B exists to avoid. The residual is narrow
(only merged/compacted partitions that outlive their process's Postgres row) and self-resolves on
that partition's own next retention pass. Rejected alternative, for the same reason as before: making
Prong B read the parquet snapshot instead of Postgres, which would reintroduce the daemon dependency
and the aggregate-scan cost this design was chosen to avoid.

## Implementation Steps

### Phase 1 — the index and the guard (no call sites yet)

1. New `rust/analytics/src/lakehouse/audience_guard.rs`: `AUDIENCE_PROPERTY`, `OwnerAudience`,
   `IdKind`, `AudienceIndex` (`resolve`, `resolve_many`, the three SQL shapes), `is_readable`,
   `AudienceGuard`, `Authorized`. Register in `lakehouse/mod.rs`.
2. Point `OwnershipRewrite::audience_col` (`ownership_rewrite.rs:148`) at `AUDIENCE_PROPERTY` so the
   property name has exactly one definition.
3. Add `audience_index: Arc<AudienceIndex>` to `LakehouseContext` (`lakehouse_context.rs:22-85`)
   with an `audience_index()` accessor, constructed in `LakehouseContext::new` next to
   `metadata_cache`. Size from a module constant `DEFAULT_AUDIENCE_CACHE_ENTRIES = 100_000`; no env
   knob — same shape as `analytics-web-srv/src/data_source_cache.rs`'s hardcoded
   `.max_capacity(1000)`, a fixed-shape entry cache with no operational knob, unlike
   `MICROMEGAS_METADATA_CACHE_MB` whose per-entry weight genuinely varies.
4. Offline unit tests for `is_readable` and for `AudienceIndex`'s cache behavior that needs no DB.

### Phase 2 — `IsolationConfig` and the registration knob

5. Rename `OwnershipRewriteConfig` → `IsolationConfig`, `CallerContext.ownership_config` →
   `isolation_config`, add `user_maintenance_functions: bool` parsed from
   `{prefix}_USER_MAINTENANCE_FUNCTIONS`/`MICROMEGAS_USER_MAINTENANCE_FUNCTIONS`
   (`read_scope.rs:84-193`). Follow the compiler through `flight_sql_service_impl.rs:500-565`,
   `flight_sql_server.rs`, `monolith`, and the test files. Also fix the doc comment in
   `rust/auth/tests/policy_tests.rs:529` (`"OwnershipRewriteConfig::from_env"` → `"IsolationConfig::
   from_env"`) — a comment in a crate with no compile-time dependency on the type, so the compiler
   won't flag it.
6. Extend the gate at `query.rs:154` to `caller.is_admin || caller.isolation_config
   .user_maintenance_functions`; extend `lakehouse_admin_gate_test.rs` with the knob's two states.

### Phase 3 — arg-addressed guards

7. `register_lakehouse_functions` (`query.rs:100-180`): build one
   `Arc<AudienceGuard>` from `caller` + `lakehouse.audience_index()` and pass it into
   `PerfettoTraceTableFunction::new`, `ParseBlockTableFunction::new`,
   `ProcessSpansTableFunction::new`, `GetPayload::new`, `ListPartitionsTableFunction::new`.
8. `process_spans_table_function.rs`: parse arg 1 as a `Uuid` in `call_with_args` (:105-118), carry
   the canonical string plus the `Uuid` into the plan, add `authorized: Option<Authorized>`, do the
   `authorize` in `scan` (:384) and require the witness in `execute` (:254-264), where the inner
   context becomes `authorized.internal_caller()`.
9. `perfetto_trace_table_function.rs` / `perfetto_trace_execution_plan.rs`: same shape —
   `Uuid` parse at :65-75, `authorize` in `scan` (:570), witness threaded into
   `generate_streaming_perfetto_trace` (:254-266) for its inner context. Also narrow
   `PerfettoTraceExecutionPlan::new` and `PerfettoTraceTableProvider::new` from `pub` to
   `pub(crate)` (only ever called from `perfetto_trace_table_function.rs`, same crate), matching
   `process_spans`' existing module-private shape so the "no un-authorized plan reachable outside
   `scan`" invariant (Testing Strategy) actually holds for this function too.
10. `parse_block_table_function.rs`: `authorize(block_id, IdKind::Block)` in `scan` (:296) before
    `fetch_block_metadata`, whose inner context (:83-92) becomes the witness's.
11. `get_payload_function.rs`: distinct arg-1 `process_ids` → `resolve_many` →
    all-or-nothing denial in `invoke_async_with_args` (:75-118).
12. Replace the three `TODO(#1371)` comments with the §6 invariant, and the two in `metadata.rs`
    (:182, :286) with §7's rationale.

### Phase 4 — `list_partitions`

13. `list_partitions_table_function.rs`: guard field on the function and the provider; in `scan`
    (:104), keep today's path for `ReadScope::All`, otherwise drop the `LIMIT` pushdown, filter the
    `sqlx` rows per §8, then truncate and build the batch — using `RecordBatch::new_empty(schema())`
    rather than `rows_to_record_batch` when the filtered vector is empty (§8).

### Phase 5 — tests, docs, CHANGELOG

14. Tests per Testing Strategy.
15. Docs per Documentation.
16. `CHANGELOG.md`: `OwnershipRewriteConfig`/`ownership_config`/`with_ownership_config` have not
    shipped in a release (they are introduced by Stage 2's own entry under `## Unreleased`), so
    rename every mention of them to `IsolationConfig`/`isolation_config`/`with_isolation_config`
    *inside that existing Unreleased entry* rather than announcing a break against them. Add a new
    entry under the same Unreleased section for this stage's actual release-facing changes,
    including the **Minor breaking change** clause for the `ListPartitionsTableFunction::new` /
    `GetPayload::new` / three UDTF constructor signature changes (these *are* breaks against the
    released v0.29.0 shape), and the operator note that `list_partitions` now hides rows (and
    `'global'` rows in particular) from a `ReadScope::Audiences` session unless
    `MICROMEGAS_UNSTAMPED_AUDIENCE` is set.

## Files to Modify

**New**
- `rust/analytics/src/lakehouse/audience_guard.rs`
- `rust/analytics/tests/audience_guard_tests.rs` (offline)
- `rust/analytics/tests/prong_b_guard_db_test.rs` (DB-backed)

**Analytics**
- `lakehouse/mod.rs` (module registration + the Prong B note at `mod.rs:96`)
- `lakehouse/read_scope.rs` (`IsolationConfig` rename, the knob)
- `lakehouse/lakehouse_context.rs` (`audience_index`)
- `lakehouse/query.rs` (guard construction, function wiring, registration gate)
- `lakehouse/ownership_rewrite.rs` (`AUDIENCE_PROPERTY`; the Stage-3 note at :31)
- `lakehouse/process_spans_table_function.rs`, `lakehouse/perfetto_trace_table_function.rs`,
  `lakehouse/perfetto_trace_execution_plan.rs`, `lakehouse/parse_block_table_function.rs`,
  `lakehouse/get_payload_function.rs`, `lakehouse/list_partitions_table_function.rs`
- `metadata.rs` (the two TODO resolutions)
- existing tests constructing `OwnershipRewriteConfig`/`CallerContext`:
  `lakehouse_admin_gate_test.rs`, `ownership_rewrite_config_tests.rs`,
  `ownership_rewrite_db_test.rs`, `ownership_rewrite_public_view_set_tests.rs`

**Public / servers**
- `rust/public/src/servers/flight_sql_service_impl.rs`, `flight_sql_server.rs` (rename only)
- `rust/monolith/` and any other `with_ownership_config` caller (rename only)
- `rust/public/tests/read_policy_threading_tests.rs`
- `rust/auth/tests/policy_tests.rs` (comment-only: stale `OwnershipRewriteConfig::from_env` reference)

**Docs**
- `mkdocs/docs/admin/flight-sql.md`, `mkdocs/docs/admin/monolith.md` (env-var tables — the new knob,
  prefixed + unprefixed fallback)
- `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/admin/functions-reference.md`,
  `mkdocs/docs/query-guide/functions-reference.md`, `CHANGELOG.md`,
  `tasks/data_isolation/audience_based_access_control_plan.md` (record the §5 and cache-shape
  deviations)

## Trade-offs

- **Guard-then-authorized-internal instead of scope inheritance (§6).** Chosen for correctness on
  fresh data and a crisper security argument; costs the defense-in-depth of a second, independent
  filter inside those three functions. Mitigated by the witness type, which makes the authorization
  a precondition of building the inner context rather than a convention.
- **Postgres as Prong B's audience source (§11).** Fresher and one indexed point query, at the cost
  of the two prongs reading different copies of the same property. Rejected alternative: resolving
  through the materialized `processes` view for symmetry.
- **One cache keyed by `(IdKind, Uuid)` instead of the parent plan's three caches (§1).** Same
  disjoint-per-kind isolation as three separate caches, in one `moka` instance and one eviction
  policy; the cost is a slightly larger key than a bare `Uuid`.
- **Not caching `Unknown`.** Avoids pinning a stale denial and cache pollution from random ids; costs
  one point query per denied lookup. A denial path being the slower path is the right way round.
- **No `LIMIT` pushdown for filtered `list_partitions` (§8).** Correct results over one optimization,
  on a table whose unlimited path is already the common case.
- **Renaming `OwnershipRewriteConfig` (§9).** Churn across ~6 construction sites, bought against a
  config object whose name would otherwise lie about a third of its contents. Not a break against
  any released API: the type was introduced under the still-`## Unreleased` Stage-2 entry, so the
  rename lands as an in-place edit of that entry, not a new breaking-change clause.
- **JIT materialization remains triggerable for unreadable instances (§7).** Accepted as a
  cost/availability residual, not a confidentiality one; a `view_instance` scan-time guard is the
  fix and is deliberately out of scope here.

## Security

- **Fail-closed everywhere:** `Unknown` ⇒ deny; resolution error ⇒ deny (the query fails, never a
  pass); empty `ReadScope::Audiences` ⇒ deny (no audience matches); missing witness at `execute` ⇒
  internal error. `ReadScope::All` is the only permissive branch and only internal/maintenance
  callers (and an auth-unset deployment) can hold it.
- **No new existence oracle (§10)**, and no new leak surface in the audit log beyond what
  `flightsql_query_audit` already records.
- **Closes an injection vector incidentally (§3):** `process_id` stops reaching inner SQL as an
  unvalidated string.
- **Still not a boundary against a malicious *writer*.** `micromegas.audience` remains
  client-asserted until Stage 5 (#1373) stamps it server-side from the authenticated
  `bound_audience`. Prong B inherits that limitation verbatim from Prong A
  (`ownership_rewrite.rs:59-75`) and must be documented the same way.
- **Unchanged bypasses, by design:** no admin read bypass (`is_admin` never feeds `ReadScope`);
  `list_view_sets` unfiltered; public view sets relax only `list_partitions`' `'global'` rows, never
  the arg-addressed guards (which are process-scoped, so the public exemption cannot apply).
- **`MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` is a deployment-wide grant, not a per-audience one
  (§9).** None of the five mutating functions filters by audience, so enabling the knob lets any
  authenticated caller retire or materialize partitions belonging to any audience — acceptable for
  its intended API-key-only deployment (no admin principal exists), a real cross-audience
  destructive-access hole in a hybrid deployment that also has personal or per-team audiences.

## Performance

- Warm path: one `moka` hash lookup plus an `Arc<[String]>` scan of the caller's audiences —
  effectively free next to the parquet reads that follow.
- Cold path: one indexed Postgres point query per new id, at most once per id per TTL window
  (default `5m`). `list_partitions` and `get_payload` batch their misses into a single `= ANY($1)`
  query.
- `list_partitions` under a filtered scope loses `LIMIT` pushdown and gains one extra query; the
  row-filter itself is a `Vec` retain over `sqlx` rows.
- Cache bound: `100_000` entries ≈ 10 MB, one per service process.

## Documentation

- `mkdocs/docs/admin/flight-sql.md` env-var table: add a `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` row
  alongside `MICROMEGAS_UNSTAMPED_AUDIENCE`/`MICROMEGAS_PUBLIC_VIEW_SETS`, with the API-key-only-
  deployment rationale from Current State **and the §9 deployment-wide-not-per-audience caveat**.
- `mkdocs/docs/admin/monolith.md` env-var table: add the `MICROMEGAS_ANALYTICS_`-prefixed
  `MICROMEGAS_ANALYTICS_USER_MAINTENANCE_FUNCTIONS` row (falls back to unprefixed
  `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS`, following the table's existing prefixed-knob rows), with
  the same caveat.
- `mkdocs/docs/admin/authentication.md` §"Audience Filtering Activation" (:152-175): extend from
  "every query plan gets a predicate" to also cover Prong B — the four guarded functions and their
  uniform denial, `list_partitions` row filtering including the `'global'`-row rule and its
  dependence on `MICROMEGAS_UNSTAMPED_AUDIENCE`, and the freshness difference between the prongs
  (§11). Add `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` to the knob list with the API-key-only-
  deployment rationale **and the caveat that it grants cross-audience destructive access in a
  hybrid deployment (§9)**.
- `mkdocs/docs/admin/functions-reference.md`: `list_partitions()` (:42) gains a note that rows are
  audience-filtered for a scoped caller and that `'global'` rows follow the knob; the five mutating
  functions gain the knob alongside the existing admin requirement.
- `mkdocs/docs/query-guide/functions-reference.md`: `perfetto_trace_chunks` (:85),
  `process_spans` (:138), `parse_block` (:196) and `get_payload` gain one line each — the id
  argument must name data in an audience the caller can read, otherwise the call fails with a
  not-found-shaped error. The 🔒 legend line (:5) and the five 🔒-marked entries it describes
  (`retire_partitions` :49, `materialize_partitions` :55, `regenerate_partitions` :61,
  `retire_partition_by_metadata` :73, `retire_partition_by_file` :79) currently state admin-only
  access unconditionally; qualify both the legend and the five entries with "...unless
  `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` is enabled", matching the admin-gate caveat already
  planned for `admin/functions-reference.md` above.
- `tasks/data_isolation/audience_based_access_control_plan.md`: record the two deviations (§6's
  guard-then-internal instead of scope inheritance; one cache instead of three, and no
  `block_id → process_id` chain for `get_payload`) in the same "Implemented — corrections to the
  sketch above" style Stage 2 used.
- `CHANGELOG.md` per step 16.

## Testing Strategy

**Offline (`audience_guard_tests.rs`, no DB)**
- `is_readable` truth table: `All` passes everything including `Unknown`; `Audiences` denies
  `Unknown` always; `Unstamped` passes iff `unstamped_audience` is set *and* in scope; `Audience`
  matches byte-exactly (`Team-Alpha` ≠ `team-alpha`); empty audience set denies everything.
- `AudienceGuard::authorize` under `ReadScope::All` performs no I/O (a guard built over a
  `connect_lazy` pool to an unroutable address must still return `Ok` — the same trick
  `lakehouse_admin_gate_test.rs:20` uses).
- The `None` ⇒ `Internal` branch in `execute` (§3) is a compile-time-enforced invariant, not a
  runtime one exercised here, for `process_spans`: `ProcessSpansExecutionPlan::new`/
  `ProcessSpansTableProvider` are private to their module and `TableFunctionImpl::call_with_args`
  returns only `Arc<dyn TableProvider>`, so an integration test in `rust/analytics/tests/` has no way
  to reach an un-authorized plan except through `scan` — exactly the path this invariant exists to
  not depend on. `perfetto_trace_chunks` needs the same shape to make the same claim: today
  `PerfettoTraceExecutionPlan::new` and `PerfettoTraceTableProvider::new` are `pub`
  (`perfetto_trace_execution_plan.rs:60,555`), so an external test crate *could* build an
  un-authorized plan directly. Phase 3 (step 9) narrows both to `pub(crate)` alongside the witness
  field, closing that gap the same way `process_spans` already has it closed. The DB-backed denial
  tests (below) cover the caller-observable behavior for both functions either way.
- `IsolationConfig::from_env`: knob `true`/`false`/absent/garbage (`Err`), prefixed and unprefixed.
- Registration gate: the five mutating functions are absent for a non-admin with the knob unset,
  present with the knob `true`, present for an admin regardless (extends
  `lakehouse_admin_gate_test.rs`).

**DB-backed (`prong_b_guard_db_test.rs`, requires `MICROMEGAS_SQL_CONNECTION_STRING` /
`MICROMEGAS_OBJECT_STORE_URI`, mirroring `ownership_rewrite_db_test.rs`)**

Seed two processes stamped with different audiences plus one unstamped, through the real ingestion
pipeline (reuse `ownership_rewrite_db_test.rs`'s `ProcessInfo.properties` stamping approach), with
thread and async spans and at least one block per process. Then, for a
`ReadScope::Audiences(["team-a"])` session:
- `process_spans`, `perfetto_trace_chunks`, `parse_block`, `get_payload` on **own** ids return the
  same rows/bytes as a `ReadScope::All` session (no over-blocking), with every view — including
  `processes` — materialized. Then pin the §6 regression window for the two functions that
  genuinely avoid a `processes` dependency: with the process's `blocks` partitions materialized but
  its `processes` partitions deliberately **not**, `parse_block`/`get_payload` still succeed (they
  never touch `processes`). `process_spans` and `perfetto_trace_chunks` do **not** belong in that
  assertion: `process_spans`' `view_instance('thread_spans'/'async_events', …)` call triggers a
  `jit_update` that itself reads `processes` (`find_process_with_latest_timing`,
  `thread_spans_view.rs:358-374`, `async_events_view.rs:130`, bailing `"Process not found"` on an
  empty result, `metadata.rs:319-321`), and `perfetto_trace_chunks`' `get_process_exe` hop reads the
  `processes` named table directly — so both already fail in that window under `ReadScope::All`
  today, guard or no guard; that daemon-materialization gap is pre-existing and out of scope here.
- The same four on the **other** audience's ids fail, with an error indistinguishable from the
  error for a random, nonexistent id (assert the message shape, not just the failure).
- The same four on the **unstamped** process fail with `MICROMEGAS_UNSTAMPED_AUDIENCE` unset and
  succeed with it set to an audience in scope.
- `get_payload` with a mixed batch (one own, one foreign process id) fails the whole call.
- `list_partitions`: only own instance rows visible; a `thread_spans` row of the caller's own stream
  *is* visible (the stream→process hop works); `'global'` rows hidden with the knob unset, visible
  with it set to an in-scope audience, visible when the row's view set is in `public_view_sets`, and
  always visible under `ReadScope::All`; `LIMIT n` over a filtered set returns `min(n, matching)`
  rows and never fewer than that because of pushdown.
- `ReadScope::All` sees everything, byte-for-byte as today, for all five surfaces.

**Whole-suite**
- `cargo test`, `cargo clippy --workspace -- -D warnings`, `cargo fmt`, then
  `python3 build/rust_ci.py`.
- Manual smoke on the local test env: monolith with `--disable-auth` (⇒ `ReadScope::All`) must show
  zero behavior change for flame graphs, perfetto export, and the admin partitions page.

## Open Questions

1. **Follow-up issue for the `view_instance` JIT residual (§7)?** A caller can trigger
   materialization of an instance they cannot read. Recommend filing it rather than widening this
   stage.
