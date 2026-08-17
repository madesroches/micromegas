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
the five mutating functions. All of it fed by one new, size-bounded, invalidation-free cache
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

Also relevant: `streams.process_id` and `blocks.process_id` both exist and are indexed
(`sql_telemetry_db.rs:48-88`), so `stream_id → owning process` and `block_id → owning process` are
one indexed point query each — no join through the lakehouse needed.

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerAudience {
    Unknown,
    Unstamped,
    Audience(Arc<str>),
}

/// Which table resolves the id to its owning process.
#[derive(Copy, Clone, Debug)]
pub enum IdKind {
    Process,          // processes.process_id
    Stream,           // streams.stream_id     -> processes
    Block,            // blocks.block_id       -> processes
    ProcessOrStream,  // list_partitions' view_instance_id: either, resolved in one round trip
}

pub struct AudienceIndex {
    pool: sqlx::Pool<sqlx::Postgres>,
    cache: moka::future::Cache<Uuid, OwnerAudience>,
}

impl AudienceIndex {
    pub fn new(pool: sqlx::Pool<sqlx::Postgres>, max_entries: u64) -> Self;
    pub async fn resolve(&self, id: Uuid, kind: IdKind) -> anyhow::Result<OwnerAudience>;
    pub async fn resolve_many(
        &self,
        ids: &[Uuid],
        kind: IdKind,
    ) -> anyhow::Result<HashMap<Uuid, OwnerAudience>>;
}
```

**One cache for all three kinds, keyed by the raw `Uuid`, is correct by construction.** The cached
value is the *owning process's* audience, and a given UUID is a process id, a stream id or a block
id — never two of those — so no key can be populated with two different answers. A cross-kind hit
(e.g. a caller passing a stream id where `process_spans` wants a process id, after
`list_partitions` cached it) returns that stream's owning process's audience, which is exactly the
authorization question being asked; the downstream query then returns nothing because the id isn't a
process. No confidentiality consequence, and one cache instead of the parent plan's three.

**Invalidation-free, bounded by entry count.** `process_id → properties` is written once at process
insert and never updated (no `UPDATE processes` exists anywhere in the tree); `streams.process_id`
and `blocks.process_id` are fixed at creation. So the mapping is immutable and the cache needs no
TTL — only an LRU bound (`max_capacity(entries)`, default `100_000`, one `Uuid` + short string
≈ 100 B ⇒ ~10 MB). Mirrors `MetadataCache`'s `imetric!` entry-count reporting.

**`Unknown` is never cached.** A miss means "no such row *yet*" — the process may be mid-ingestion —
and caching it would both pin a wrong answer and let a caller pollute the cache with random UUIDs.
Cost of not caching: one indexed point query per denied lookup.

The SQL, for `IdKind::Process` (the other kinds swap the driving table and join through
`processes`; `ProcessOrStream` is a `UNION ALL` of the first two, one round trip):

```sql
SELECT p.process_id AS id, a.value AS audience
FROM processes p
LEFT JOIN LATERAL (
    SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1
) a ON TRUE
WHERE p.process_id = ANY($1)
```

`LEFT JOIN LATERAL` (not an inner `unnest` in the `FROM` list) is what keeps *unstamped* rows in the
result: an inner unnest silently drops them, collapsing `Unstamped` into `Unknown` and turning the
`MICROMEGAS_UNSTAMPED_AUDIENCE` escape hatch off for exactly the rows it exists for. `LIMIT 1`
handles a duplicated property key; `= ANY($1::uuid[])` is what makes `resolve_many` one query.
Neither `processes(process_id)` nor `blocks(block_id)` is a *unique* index
(`sql_telemetry_db.rs:41,86`), so read at most one row per id and do not assume uniqueness.

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

- **It would break the functions for fresh data, in every deployment.** Prong A resolves audiences
  from the daemon-materialized `processes`/`streams` views. `process_spans`' inner query is
  `get_process_thread_list` over `blocks` (Prong A §4 semi-join) and then
  `view_instance('thread_spans', …)` (Prong A §6 two-hop `EXISTS`); `perfetto_trace_chunks`' is
  `get_process_exe` over `processes` plus the same span queries. A process ingested since the
  daemon's last pass contributes no row to the aggregate, so **every one of those inner queries
  returns nothing** — a caller's own just-finished process would produce an empty flame graph or an
  empty trace. That is precisely the class of surprise Stage 2's CHANGELOG already flags as its
  known limitation; extending it into the interactive tracing UX is a bad trade.
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
schema construction untouched. Do it there.

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

### 11. Two prongs, two copies of the audience — accepted, with the direction stated

Prong A reads the daemon-materialized parquet snapshot; Prong B reads Postgres, the origin. They can
disagree only while a process is stamped in Postgres and not yet (or not consistently) materialized,
and the disagreement is always in the same direction: **Prong B is fresher and at least as accurate**.
Consequences:

- A caller's own just-ingested process: `process_spans`/`perfetto_trace_chunks`/`parse_block`/
  `get_payload` work (Prong B allows, correctly); the equivalent plain-SQL query over
  `log_entries`/`thread_spans` returns nothing until the daemon catches up (Prong A's known
  limitation, unchanged by this stage).
- A foreign process: denied by both.

There is no configuration in which Prong B is more permissive than the ground truth in Postgres, so
this asymmetry is a usability gradient, not a hole. Making Prong B read the parquet snapshot instead
(for symmetry) was rejected: it would import Prong A's daemon dependency into the interactive tracing
path and cost a full aggregate scan per guard check instead of one indexed point query.

## Implementation Steps

### Phase 1 — the index and the guard (no call sites yet)

1. New `rust/analytics/src/lakehouse/audience_guard.rs`: `AUDIENCE_PROPERTY`, `OwnerAudience`,
   `IdKind`, `AudienceIndex` (`resolve`, `resolve_many`, the four SQL shapes), `is_readable`,
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
   `flight_sql_server.rs`, `monolith`, and the test files.
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
   `generate_streaming_perfetto_trace` (:254-266) for its inner context.
10. `parse_block_table_function.rs`: `authorize(block_id, IdKind::Block)` in `scan` (:296) before
    `fetch_block_metadata`, whose inner context (:83-92) becomes the witness's.
11. `get_payload_function.rs`: distinct arg-1 `process_ids` → `resolve_many` →
    all-or-nothing denial in `invoke_async_with_args` (:75-118).
12. Replace the three `TODO(#1371)` comments with the §6 invariant, and the two in `metadata.rs`
    (:182, :286) with §7's rationale.

### Phase 4 — `list_partitions`

13. `list_partitions_table_function.rs`: guard field on the function and the provider; in `scan`
    (:104), keep today's path for `ReadScope::All`, otherwise drop the `LIMIT` pushdown, filter the
    `sqlx` rows per §8, then truncate and build the batch.

### Phase 5 — tests, docs, CHANGELOG

14. Tests per Testing Strategy.
15. Docs per Documentation.
16. `CHANGELOG.md` entry under the unreleased section, including the **Minor breaking change**
    clause for the `IsolationConfig` rename, the `ListPartitionsTableFunction::new` /
    `GetPayload::new` / three UDTF constructor signature changes, and the operator note that
    `list_partitions` now hides rows (and `'global'` rows in particular) from a
    `ReadScope::Audiences` session unless `MICROMEGAS_UNSTAMPED_AUDIENCE` is set.

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
- **One cache keyed by bare `Uuid` across three id kinds (§1).** Simpler than the parent plan's
  three caches and correct because the answer is kind-independent; the cost is that a wrong-kind id
  is accepted by the guard (and then returns no data downstream) instead of being rejected outright.
- **Not caching `Unknown`.** Avoids pinning a stale denial and cache pollution from random ids; costs
  one point query per denied lookup. A denial path being the slower path is the right way round.
- **No `LIMIT` pushdown for filtered `list_partitions` (§8).** Correct results over one optimization,
  on a table whose unlimited path is already the common case.
- **Renaming `OwnershipRewriteConfig` (§9).** Churn across ~6 construction sites and a published
  API, bought against a config object whose name would otherwise lie about a third of its contents.
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

## Performance

- Warm path: one `moka` hash lookup plus an `Arc<[String]>` scan of the caller's audiences —
  effectively free next to the parquet reads that follow.
- Cold path: one indexed Postgres point query per new id, at most once per id for the process's
  lifetime. `list_partitions` and `get_payload` batch their misses into a single `= ANY($1)` query.
- `list_partitions` under a filtered scope loses `LIMIT` pushdown and gains one extra query; the
  row-filter itself is a `Vec` retain over `sqlx` rows.
- Cache bound: `100_000` entries ≈ 10 MB, one per service process.

## Documentation

- `mkdocs/docs/admin/flight-sql.md` env-var table: add a `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` row
  alongside `MICROMEGAS_UNSTAMPED_AUDIENCE`/`MICROMEGAS_PUBLIC_VIEW_SETS`, with the API-key-only-
  deployment rationale from Current State.
- `mkdocs/docs/admin/monolith.md` env-var table: add the `MICROMEGAS_ANALYTICS_`-prefixed
  `MICROMEGAS_ANALYTICS_USER_MAINTENANCE_FUNCTIONS` row (falls back to unprefixed
  `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS`, following the table's existing prefixed-knob rows).
- `mkdocs/docs/admin/authentication.md` §"Audience Filtering Activation" (:152-175): extend from
  "every query plan gets a predicate" to also cover Prong B — the four guarded functions and their
  uniform denial, `list_partitions` row filtering including the `'global'`-row rule and its
  dependence on `MICROMEGAS_UNSTAMPED_AUDIENCE`, and the freshness difference between the prongs
  (§11). Add `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` to the knob list with the API-key-only-
  deployment rationale.
- `mkdocs/docs/admin/functions-reference.md`: `list_partitions()` (:42) gains a note that rows are
  audience-filtered for a scoped caller and that `'global'` rows follow the knob; the five mutating
  functions gain the knob alongside the existing admin requirement.
- `mkdocs/docs/query-guide/functions-reference.md`: `perfetto_trace_chunks` (:85),
  `process_spans` (:138), `parse_block` (:196) and `get_payload` gain one line each — the id
  argument must name data in an audience the caller can read, otherwise the call fails with a
  not-found-shaped error.
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
  runtime one exercised here: `ProcessSpansExecutionPlan::new`/`ProcessSpansTableProvider` are
  private to their module and `TableFunctionImpl::call_with_args` returns only
  `Arc<dyn TableProvider>`, so an integration test in `rust/analytics/tests/` has no way to reach an
  un-authorized plan except through `scan` — exactly the path this invariant exists to not depend
  on. The DB-backed denial tests (below) cover the caller-observable behavior instead.
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
  same rows/bytes as a `ReadScope::All` session (no over-blocking) — including for a process whose
  `processes` view partitions have deliberately **not** been materialized, which is the regression
  §6 exists to prevent.
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
