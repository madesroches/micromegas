# AbAC: audience-based access control for telemetry

> **Supersedes** [`per_user_data_isolation_plan.SUPERSEDED.md`](per_user_data_isolation_plan.SUPERSEDED.md). That document analyzed the
> per-user case and remains the reference for the confidentiality/integrity analysis and the
> current-state audit; this plan generalizes its mechanism. Where they disagree, this document wins.

> **Revised 2026-07-30** for deployment staging (issue #1334 follow-up): the `self|audience` mode
> enum and its `self` default are replaced by a single grant-configured policy engine (open
> deployments are an `everyone`-group configuration, not a query-side bypass); implementation steps
> are re-ordered into deployment stages so existing team-wide deployments migrate with zero behavior
> change; and the code audit is refreshed (see Appendix B for drift found since 2026-07-21).

> **Re-staged 2026-07-30**: the DB-backed key store is split out of Stage 4 into a new **Stage 0**
> that depends on nothing and ships standalone operational value (revocation/rotation without
> redeploy, no cleartext keys in env); Stage 4 keeps only the `audience` column, which genuinely
> needs the policy seam. The env keyring's per-request full scan cannot scale to per-user keys, so
> the store is a precondition for Stage 6, not a convenience.

> **Keys track revised 2026-07-30** (issue #1383 discussion): write and read credentials get
> **separate tables** (`ingestion_api_keys` / `analytics_api_keys`) — one key is never valid on both
> surfaces, because the risk is asymmetric and only ingestion keys carry an `audience`. Key
> management is an **OIDC-authenticated HTTP API** on the ingestion service, pulled forward from
> Stage 6 into Stage 0 (create/revoke/list, no audience); Stage 6 now only adds audience resolution
> to a route that already exists. Admin-gated lakehouse UDFs were considered for key management and
> rejected — see Stage 0.

> **Renamed 2026-07-30** from "policy-based data isolation". The design was described as *RBAC*
> throughout, but it has no roles — see [Naming](#naming) below. The model is **AbAC,
> audience-based access control**; `Rbac*` identifiers become `Audience*`.

## Overview

Isolate telemetry so that data produced under one identity is only **readable** by principals
authorized to read it. The mechanism is a small, general **AbAC seam**. There is **no mode enum and
no default policy**: one policy engine, configured by grants, spans the whole deployment spectrum —
from an **open (team-wide) deployment**, where an implicit `everyone` group preserves today's
everyone-reads-everything behavior with no data migration, to a **privacy deployment**, where each
user reads only their own data and sharing happens through the IdP `groups` claim. Moving along
that spectrum is configuration, not code — no API change, no rewrite of the enforcement rule.

The design rests on one structural decision: model everything as a **generic principal stamped on
data** (`audience`) plus a **set-valued read check** (`audience IN (readable principals)`), never an
equality (`owner = caller`). In a privacy deployment with no group grants the readable set is a
singleton, so the executed query plan is identical to the simple per-user design — but the code is
already the general form and never has to be reworked.

### The three-relation model

Authorization decomposes into three independent relations:

1. **`write(subject → group)`** — checked **once, at mint time** (key issuance). Grants the right to
   mint an ingestion key that stamps `audience = group`.
2. **`read(subject → group)`** — checked **on every query**. Grants visibility of data whose
   `audience = group`.
3. **`audience`** — the label physically stamped on data by the key; the link between the two.

**Per-user isolation is the restriction of this model to singleton self-groups**: every principal is
its own group, with `write(u → u)` and `read(u → u)` the only grants, and `audience` always the
minter's own email. **Open (team-wide) deployments are the other end of the same model**: an
implicit group `everyone` that every authenticated principal belongs to, with shared data stamped
`group:everyone` — including the audience assigned to existing ingestion API keys when they are
imported into the key store (Stage 0 imports them; Stage 4 assigns the audience). Every point on the
spectrum runs the *same* code; only the configuration differs.

### Load-bearing property preserved

Confidentiality rides entirely on **OIDC identity + `ReadPolicy`**, evaluated per-query. The write
key governs **integrity only**:

- A stolen write key lets an attacker *write* data labeled with some `audience` (pollutes that
  audience's view — an integrity problem). It grants **zero** read power: reading requires the
  caller's own OIDC identity to satisfy `read(caller → audience)`.
- There is no write→read escalation: a holder of `write(caller → G)` who lacks `read(caller → G)`
  cannot read G — not even the rows they themselves wrote (audience is `G`; reading `G` needs
  `read`).

Because the write grant is frozen into the key at mint and ingestion never re-checks it, **write
keys can be eternal** (the current use case). No per-write policy lookup; no `minted_by` bookkeeping
in v1 (see Deferred / Trade-offs for when that changes).

### Naming

This model is **AbAC — audience-based access control**. Earlier revisions called it RBAC, which was
wrong: there is no role anywhere in the design. RBAC's defining feature is the indirection
`subject → role → permission`, where a role is a named, reusable bundle of permissions. Nothing
here bundles permissions. A subject carries a set of audiences; data carries one audience; access
is set membership.

Structurally the closest industry analogue is AWS-style **ABAC** (attribute-based access control):
a tag on the resource matched against a tag on the principal, exactly like
`aws:PrincipalTag/team == aws:ResourceTag/team`. This design is that pattern degenerated to a
single attribute — hence the distinct casing **AbAC**, and hence *audience*-based rather than
*attribute*-based: naming the one attribute is more honest than implying a general attribute
engine with conditions and a deny effect, none of which exist here.

Vocabulary used throughout:

| Term | Meaning |
|---|---|
| **audience** | the label stamped on data at ingestion — `user:<email>` or `group:<id>`. Distinct from `AuthContext.audience` (the OIDC token audience); see "Naming collision to avoid" below. |
| **grant** | one of the two relations — `write(subject → group)` at mint, `read(subject → group)` at query. In v1 both derive from IdP group membership. |
| **readable set** | the audiences a caller may read; the `ReadScope` resolved per request. |
| **role** | reserved for the *capability* axis (`is_admin`, issues #1376/#1377), which is orthogonal to audience scope. Not used for data isolation. |

Two names deliberately **not** used: *RBAC* (no roles — and reserving the word keeps the admin
capability axis distinct), and the bare acronym *ABAC* (taken by attribute-based access control;
write "audience-based access control" or `AbAC`, never `ABAC`).

Roles would become meaningful here if read and write ever separate: today group membership grants
both, so there is nothing for a role to bundle. If the deferred grants table or a second write-role
claim lands (see Deferred / Trade-offs), `viewer`/`minter` per group becomes a real role and the
term earns its place.

## Current State

(Condensed from the superseded plan; verified against current code.)

### Query path — authentication real, authorization absent
- `make_session_context()` takes **no caller identity**
  (`rust/analytics/src/lakehouse/query.rs:186-228`) and executes SQL verbatim (`ctx.sql(sql)`,
  `flight_sql_service_impl.rs:389`).
- The only injected analyzer rule is `TableScanRewrite`
  (`rust/analytics/src/lakehouse/table_scan_rewrite.rs`), which adds **time-range predicates only**
  and, critically, **only rewrites `MaterializedView` table providers** — it early-returns
  `Transformed::no` for table functions (`table_scan_rewrite.rs:37-43`). Any ownership rule must
  handle the table-function case explicitly.
- Per-request caller identity **is** available: `validate_and_resolve_user_attribution_grpc` yields
  `attr.user_email` before the session context is built (`flight_sql_service_impl.rs:317`,
  used for audit at `349-366`). It is never passed to `make_session_context`.
- `SessionConfigurator` (`rust/analytics/src/lakehouse/session_configurator.rs`) is a per-**service**
  object (`self.session_configurator.clone()`), not per-request — it cannot by itself carry
  per-caller identity.

### Ingestion path — key gate, identity discarded
- API keys come from the static `MICROMEGAS_API_KEYS` env var
  (`rust/auth/src/api_key.rs`), parsed into an in-memory `HashMap<Key, name>`; constant-time compare;
  no runtime add/revoke. On match: `AuthContext { subject: name, email: None, issuer: "api_key",
  audience: None, expires_at: None, auth_type: ApiKey, is_admin: false, allow_delegation: true }`
  (`api_key.rs:116-127`).
- Providers compose via `MultiAuthProvider` in `default_provider::provider_with_prefix`
  (`rust/auth/src/default_provider.rs:51-119`).
- No ingestion handler reads `AuthContext`; identity gates the request and is dropped.

### Data model — no owner dimension
- `processes` table (`rust/ingestion/src/sql_telemetry_db.rs`): no owner/tenant column.
- `processes_view` is a SQL view exposing `process_id … properties`
  (`rust/analytics/src/lakehouse/processes_view.rs`). Properties are row-level queryable via
  `property_get` but cannot prune partitions.

### Naming collision to avoid
`AuthContext` **already has** an `audience` field (`rust/auth/src/types.rs:26`) — it holds the *OIDC
token audience* (API audience / client id). The data-isolation principal is a **different concept**.
To avoid confusion, this plan uses:
- `audience` — the principal **stamped on data** (process property / column). New concept.
- `bound_audience: Option<String>` — a **new** `AuthContext` field: the principal a credential is
  bound to write as. Do **not** overload the existing `audience` field.

## Design

Four seams. Two are trait objects (the policy seam); two are the mechanical stamp + enforce.

### 1. `MintPolicy` — who may stamp which audience (mint-time, ingestion side)

```rust
/// Resolves the audience a mint request is permitted to bind to a key.
#[async_trait]
pub trait MintPolicy: Send + Sync + std::fmt::Debug {
    /// `caller` is the authenticated OIDC context of the mint request.
    /// `requested` is the optional audience the caller asked for.
    /// Returns the audience to bind, or Err if not permitted.
    async fn resolve_audience(
        &self,
        caller: &AuthContext,
        requested: Option<&str>,
    ) -> anyhow::Result<String>;
}
```

The one shipped impl (`AudienceMintPolicy`) permits `requested` iff it is in the caller's **mintable
set**: `{user:<caller email>} ∪ {group:G : G ∈ caller's IdP groups claim} ∪ {group:G : G ∈
MICROMEGAS_IMPLICIT_GROUPS}`. With `requested = None`, the audience defaults to
`user:<caller email>`. In a privacy deployment with no implicit groups and no groups claim this
degenerates to "you may only mint keys for yourself" — the per-user case, with no separate
`SelfMintPolicy` implementation.

### 2. `ReadPolicy` — which audiences a caller may read (query-time, flight-sql side)

```rust
/// Resolves the set of audiences a caller is permitted to read.
#[async_trait]
pub trait ReadPolicy: Send + Sync + std::fmt::Debug {
    async fn readable_principals(&self, caller: &AuthContext) -> anyhow::Result<ReadScope>;
}

/// Result of a ReadPolicy. Explicit `All` variant models the daemon bypass
/// without a magic sentinel string.
pub enum ReadScope {
    /// Unfiltered. Produced ONLY for the internal maintenance daemon — never for a user session.
    All,
    /// Filter to `audience IN (principals)`. May be a singleton (per-user default).
    Principals(Vec<String>),
}
```

The one shipped impl (`AudienceReadPolicy`) returns the caller's **readable set**:

```
ReadScope::Principals(
    {user:<caller email>}
  ∪ {group:G : G ∈ caller's IdP groups claim}
  ∪ {group:G : G ∈ MICROMEGAS_IMPLICIT_GROUPS}
)
```

`ReadScope::All` is **never** produced by this policy — it exists only for the internal maintenance
daemon's contexts (§5). In a privacy deployment (no implicit groups, no groups claim) the readable
set is the singleton `{user:<caller email>}` — per-user isolation with no separate `SelfReadPolicy`
implementation. In an open deployment (`MICROMEGAS_IMPLICIT_GROUPS=everyone`) every caller's set
includes `group:everyone`, which is what imported keys stamp and what unstamped data coalesces to
(§4), so everyone keeps reading everything.

### 3. Ingestion stamps `audience`

- Mint endpoint runs `MintPolicy::resolve_audience` **once** and records the resolved audience on the
  key (env keyring: not applicable — see key-store note; DB keyring: an `audience` column).
- Key auth sets `AuthContext.bound_audience = Some(key.audience)`.
- Ingestion handlers (native `rust/public/src/servers/ingestion.rs`, OTLP
  `rust/public/src/servers/otlp.rs`) read `AuthContext.bound_audience` (currently discarded) and
  write it onto the process. **No policy lookup at write time** — the audience is already vetted and
  frozen into the key.
- Client-supplied `process.owner` / `host.*` stay **display metadata only**, never the audience.
  (Note: OTel already lands these as `otel.resource.process.owner` / `otel.resource.host.name`
  properties — `otel-ingestion/src/block.rs:467-475`. Those remain display-only; the trusted
  `micromegas.audience` is written server-side from `bound_audience`.)

**Audience value shape (resolved, Q4).** Property values are arbitrary `TEXT`; `property_get` returns
dict-encoded Utf8 usable directly in `IN` predicates (case-insensitive key match). No user/group
discriminator exists in the codebase today, so **namespace the value**: `user:<email>` for personal
audiences, `group:<id>` for groups. This prevents a group id from ever colliding with a user email in
the one `audience` field and makes intent explicit to consumers. The key name `micromegas.audience`
follows the existing dotted-namespace precedent (`otel.resource.*`).

Storage of the stamped audience (v1 vs later) mirrors the superseded plan's open decision:
- **v1: reserved property** `micromegas.audience` on the process — zero schema migration, flows
  through existing property plumbing. In-tree usage of `property_get` in WHERE predicates is equality
  only (`rust/public/src/client/query_processes.rs:73`); the `IN (...)` form relies on DataFusion's
  dictionary-type coercion (`property_get` returns `Dictionary(Int32, Utf8)`,
  `rust/datafusion-extensions/src/properties/property_get.rs:48,87-92`).
- **later: first-class `audience` column** on `processes` + propagate through views — enables
  partition pruning and a physical boundary.

### 4. Query enforcement — two prongs (resolved by research; see Appendix A)

Enforcement **cannot** be a single analyzer rule. UDTF table functions surface as
`LogicalPlan::TableScan`, but the span/metadata functions (`process_spans`, `perfetto_trace_chunks`,
`list_partitions`, `parse_block`) **do not carry their owner id in the output schema**, bake the
`process_id`/`stream_id` opaquely into the provider at plan time, and some ignore pushed-down filters
(`process_spans_table_function.rs:384`). A predicate-injecting rule has no column to filter on for
them. So enforcement is two-pronged, both fed the same per-request `ReadScope`:

**Prong A — `OwnershipRewrite` analyzer rule** (for `MaterializedView`-backed scans). A new mandatory
`AnalyzerRule` beside `TableScanRewrite`, non-bypassable (operates on the logical plan below the SQL
text). Constructed with the resolved `ReadScope`.
- `ReadScope::All` → no-op (bypass; see §5).
- `ReadScope::Principals(ps)`:
  - **`processes` view** (carries `audience` as a property in v1): `audience IN (ps)` via
    `property_get(properties, 'micromegas.audience') IN (ps)`. When
    `MICROMEGAS_UNSTAMPED_AUDIENCE` is configured, the effective audience is
    `coalesce(property_get(properties,'micromegas.audience'), '<unstamped>')` — data ingested
    before stamping existed is attributed to the configured audience at query time (the
    zero-migration path for open deployments: set it to `group:everyone` and legacy data stays
    visible with no backfill and no retention wait). Unset (privacy deployments), `NULL` audiences
    fail the `IN` and unstamped data is hidden — fail-closed.
  - **`process_id`-keyed views** (`streams`, `blocks`, `log_entries`, `measures`, span views):
    semi-join, **not** a materialized id list —
    `process_id IN (SELECT process_id FROM processes WHERE <same audience predicate as above>)`.
    No ceiling on owned processes (streaming-friendly; matches the project's no-hard-limits stance).
  - **`view_instance('<set>', <id>)`** already surfaces as a `TableScan<MaterializedView>` and is
    caught by this rule exactly like a named view — the same predicate applies. (This is why the
    existing `TableScanRewrite` can already rewrite `view_instance`.)
  - **Public view sets (opt-in):** if the scanned view set is on the public allowlist, inject **no**
    predicate — see §5b. Default allowlist is empty, so this branch is inert unless configured.

**Prong B — construction-time guard inside each UDTF `call_with_args`** (for the span/metadata
functions Prong A can't reach). The owner id literal is available there via `exp_to_string` before
the provider is built (`process_spans_table_function.rs:110`, `perfetto_trace_table_function.rs:71`).
Thread `ReadScope` into `register_lakehouse_functions` (`query.rs:95-163`) and into each function
struct, then:
- **Arg-addressed functions** (`process_spans`, `perfetto_trace_chunks`, `parse_block`): the guard
  captures `(named_process_id, ReadScope)`. Since `call_with_args` is **synchronous** and the
  process→audience mapping needs metadata, perform the actual check at **scan time** (async) inside
  the execution plan: resolve the process's `audience` and fail closed if `∉ ReadScope`. Fails at
  plan time only if the check can be satisfied from already-resolved data. The **`get_payload` UDF**
  (`query.rs:165` — added since the original audit) is the same shape in scalar form: it reads a raw
  block payload by id, so it gets the identical async guard via the `block_id → process_id →
  audience` cache chain (§4 "Prong B performance").
- **Listing functions:** `list_partitions` has no owner arg but exposes a generic `view_instance_id`
  Utf8 column whose contents depend on the view set — per `view.rs:56`, "`view_instance_id` can be a
  process_id, a stream_id or 'global'" — leaking the existence/size/timing of other principals' data
  if left unfiltered. It **must be row-filtered**, per row kind:
  - **`process_id`-keyed rows** (`log_entries`, `measures`, `async_events`, `net_spans`, ... instance
    partitions): resolve `view_instance_id` as a `process_id` through the `process_id → audience`
    cache (§4 "Prong B performance"); keep the row iff its audience `∈ ReadScope`.
  - **`stream_id`-keyed rows** (`thread_spans` — the one view set with no `process_id`-scoped
    alternative, per `view_factory.rs`): resolve via a `stream_id → process_id` lookup (added to the
    cache design below), then the same `process_id → audience` cache; same keep-iff-readable rule.
  - **`'global'` rows** (the unscoped aggregate partitions — `processes`, `streams`, `blocks`, and the
    global `log_entries`/`measures` instances): carry no single audience to check. Per the fail-closed
    posture (§5), these rows are **hidden** from a `ReadScope::Principals` session — visible only
    under `ReadScope::All` (maintenance daemon), **or** when the row's view set is on the public
    allowlist (§5b), **or** when the configured `MICROMEGAS_UNSTAMPED_AUDIENCE` is in the caller's
    `ReadScope` (a global aggregate has no single audience, so it is treated like unstamped data —
    in an open deployment every caller reads `group:everyone`, so global rows stay visible, matching
    today's behavior; in a privacy deployment the knob is unset and they stay hidden).
    Otherwise `list_partitions` never shows a row it cannot resolve to a readable audience.

  `list_view_sets` **stays unfiltered (decided):** it returns view-set schema/definitions only, which
  contain no PII or per-principal data.
- **Mutating functions (decided, revised 2026-07-30): maintenance-only unless the deployment opts
  user sessions in.** The mutating set is now **five** entries: `retire_partitions`
  (`query.rs:120`) destructively deletes `lakehouse_partitions` rows for a
  `(view_set_name, view_instance_id)` pair (`write_partition.rs:116`), and `view_instance_id` is a
  `process_id` for process-scoped view sets — the same opaque, unchecked argument as `process_spans`,
  but destructive rather than read-only: naming another principal's id destroys their partitions (an
  integrity/availability hole, not a confidentiality one). `materialize_partitions`
  (`query.rs:132`) takes no per-process id — it materializes a *global* view
  (`view_factory.get_global_view`) over an insert-time range, so it can't target another principal's
  data, but it is an unbounded write/compute operation with no legitimate use from a read session.
  `regenerate_partitions` (`query.rs:139` — added since the original audit) and the scalar UDFs
  `retire_partition_by_file` / `retire_partition_by_metadata` (`query.rs:168,170` — the original
  audit covered UDTFs only) are likewise mutating/destructive. None is a read, so none gets an
  audience filter; instead `register_lakehouse_functions` registers the set only when the session
  is an internal maintenance context (`ReadScope::All`), **or** the caller's authenticated
  `AuthContext.is_admin` is set, **or** the operator sets
  `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS=true` — the knob open deployments use to let
  non-admins keep calling them. Otherwise a user calling any of them gets "function not found".
  The admin arm is tracked independently as issue #1377 (it closes a hole that exists today,
  before any isolation work: every authenticated caller can invoke these functions) and may land
  ahead of this stage; `is_admin` must be threaded from the authenticated `AuthContext`, never
  from client-claimed attribution. (Recorded caveat: the knob is deployment-wide — in a hybrid
  deployment mixing an everyone-group with personal audiences, enabling it lets users retire
  partitions of personal audiences too; tighten to per-audience checks if hybrid becomes real.)

In a privacy deployment (no implicit groups, no groups claim), `ps` is a singleton, so Prong A
reduces to `… IN ('user:alice@…')` — the exact per-user filter, same DataFusion plan — and Prong B
checks membership in a one-element set.

**Prong B performance.** The scan-time check is fast because **`process_id → audience` is immutable**
(stamped once at ingestion, never mutated). Add an in-memory `process_id → audience` cache — a
`moka::future::Cache` mirroring `metadata_cache.rs` (moka is already a workspace dep), backed on miss
by `find_process` (`rust/analytics/src/metadata.rs:241`, a primary-key point query). Because the mapping is immutable the
cache **needs no invalidation** — bound it by size (LRU) only. Warm hit = O(1) in-memory lookup;
cold miss = one indexed PG query, at most once per process ever. An entry is ~60 B, so caching far
more than the "thousands of users" population costs a few MB. The membership test itself is an O(1)
hash lookup against `ReadScope`, which is resolved once per query from the JWT `groups` claim plus
the configured implicit groups (no server-side lookup, independent of user count). `parse_block`
and `get_payload` add one more immutable `block_id → process_id` resolution, cached the same way. `list_partitions`' `thread_spans` rows need
one further immutable resolution, **`stream_id → process_id`** — a stream's owning process is fixed
at stream creation and never mutated — backed on miss by a primary-key point query against `streams`
(mirroring `find_process`); cache it the same size-bounded, invalidation-free way, then chain into the
existing `process_id → audience` cache to reach the audience for the membership test.

### 5. Bypass paths

- **Maintenance daemon** materializing global views must run with `ReadScope::All` (internal
  materialization path, never a user session). This is the **only** producer of `ReadScope::All`.
  **There is no single chokepoint** (drift vs. the original audit): the daemon never calls
  `make_session_context` itself — internal session contexts are built per-view at ~10 sites, each
  hardcoding `NoOpSessionConfigurator` (`view.rs:109`, `merge.rs:101`, `sql_batch_view.rs:87,154`,
  `export_log_view.rs:118,171`, `batch_partition_merger.rs:133`, `metadata.rs:182,287`). All of
  these are internal-only and get `ReadScope::All` at construction. Three further context-building
  sites are **reachable from user queries** (`parse_block_table_function.rs:81`,
  `process_spans_table_function.rs:254`, `perfetto_trace_execution_plan.rs:232` — UDTFs that
  recursively build a context to run their inner query): these must **inherit the caller's
  `ReadScope`**, never `All`, or they become bypasses.
- **No human-admin query-path bypass (decided).** `is_admin` does **not** map to `ReadScope::All`; an
  admin's FlightSQL session is filtered like any other. Rationale: an operator with lakehouse/object-
  store access can read the raw parquet directly, so a query-path bypass adds attack surface and audit
  burden for no confidentiality gain. Admins needing cross-principal reads use direct storage access,
  not the query path. (`is_admin` never feeds `ReadScope`; it does get threaded to the
  session for the mutating-function registration gate — §4 Prong B, issue #1377 — an
  integrity/availability control, not a read bypass.)

### 5b. Public (audience-agnostic) views — optional, opt-in

Some aggregate views carry no per-principal PII (e.g. a metrics rollup or a fleet-wide health
summary derived across all audiences). It is useful to expose such views to **every** authenticated
caller regardless of their `ReadScope`, without granting `ReadScope::All`. This is a deliberate,
per-view-set confidentiality relaxation — **off by default, fail-closed**: a view set is private
unless an operator explicitly lists it.

Mechanism (reuses the existing per-view-set branch point, no new enforcement seam):
- A configured allowlist of **public view-set names** is resolved once per request alongside
  `ReadScope` and threaded to both prongs.
- **Prong A** already branches per view set — `OwnershipRewrite` can read the view set via
  `MaterializedView::get_view_set_name()` (`materialized_view.rs:77`). For a view set on the public
  allowlist it injects **no** predicate (neither the `processes` audience filter nor the
  `process_id` semi-join); for every other set it filters exactly as before.
- **Prong B** — `list_partitions` shows the view set's `'global'` aggregate rows (otherwise hidden
  from a `ReadScope::Principals` session, §4) when that set is public. The arg-addressed UDTFs
  (`process_spans`, `perfetto_trace_chunks`, `parse_block`) are inherently **process-scoped**, not
  aggregate, so the public exemption never applies to them — they always audience-check.
- `ReadScope::All` (maintenance daemon) is unaffected; it already sees everything.

Constraints (operator responsibility — the allowlist is a confidentiality decision):
- **Only genuinely aggregated / non-PII view sets** may be listed. The unscoped **global
  `log_entries` / `measures`** instances carry raw per-principal bodies across all audiences —
  listing those would expose every principal's raw telemetry and **must not** be done. The
  allowlist is meant for derived rollups, not raw global views.
- **Public means "any authenticated caller," not unauthenticated.** The query path always
  authenticates via OIDC; truly anonymous access is out of scope.
- **Fail-closed:** the default allowlist is empty, so with no configuration the plan is
  byte-for-byte the design above (every view set private).

Config: `MICROMEGAS_PUBLIC_VIEW_SETS` (comma-separated view-set names, default empty), resolved by
the same factory as the isolation grant knobs (see Config surface). This can be deferred past v1
with no rework — an empty allowlist is the current behavior, and the branch point
(`get_view_set_name`) is already required by Prong A.

### 6. Threading identity into the session context

`make_session_context` currently takes no identity. Add the resolved `ReadScope` as a parameter
(no `is_admin` needed — there is no admin query bypass):

```
make_session_context(lakehouse, part_provider, query_range, view_factory, configurator, read_scope)
```

- `flight_sql_service_impl` already resolves `attr.user_email` per request
  (`flight_sql_service_impl.rs:318`); call `ReadPolicy::readable_principals` there and pass the
  result into both `make_session_context` call sites (`:372`, `:842`).
- **Two identity holes must be closed as part of this threading work** (found in the 2026-07-30
  audit; either would be a full enforcement bypass):
  - The **prepared-statement path** (`flight_sql_service_impl.rs:842`,
    `do_action_create_prepared_statement`) builds its session context with **no identity
    resolution at all**. It must resolve the caller and a `ReadScope` exactly like the
    `do_get` path.
  - `validate_and_resolve_user_attribution_grpc` (`rust/auth/src/user_attribution.rs:108`) **falls
    back to client-claimed identity** when the `x-auth-subject` header is absent (`:125-133`).
    That fallback is acceptable for audit attribution but must **never** feed `ReadScope`:
    the scope is resolved from the authenticated `AuthContext` only. Note the tower `AuthService`
    currently stringifies identity into gRPC metadata (`x-auth-subject`, `x-auth-email`,
    `x-allow-delegation`); the `groups` claim must cross that boundary too (new header or,
    better, the `AuthContext` request extension directly — it is in-process middleware).
- **The scope must reach Prong B too.** `make_session_context` calls `register_functions` →
  `register_lakehouse_functions` (`query.rs:95-163`), which is where UDTFs are registered. Thread the
  `ReadScope` down that path so each `TableFunctionImpl` is constructed with it. `call_with_args` is
  **synchronous**, so pass the already-resolved `ReadScope` value (not a policy object needing async
  I/O) — the arg-addressed functions defer the actual audience check to async scan time.
- The `ReadPolicy` object itself is a per-**service** dependency (like `session_configurator`),
  stored on `FlightSqlServiceImpl`; the **resolved scope** is per-request.
- Do **not** try to smuggle identity through `SessionConfigurator` — it is shared across requests.

### Config surface

**No mode enum, no default policy** (revised 2026-07-30 — replaces the former
`MICROMEGAS_ISOLATION_POLICY=self|audience` knob and its `self` default). One AbAC engine, configured
by grants; every knob is fail-closed when empty/unset:

| Knob | Meaning | Open deployment | Privacy deployment |
|---|---|---|---|
| `MICROMEGAS_IMPLICIT_GROUPS` | comma-separated groups every authenticated principal belongs to (added to both readable and mintable sets) | `everyone` | unset |
| `MICROMEGAS_UNSTAMPED_AUDIENCE` | audience attributed at query time to data with no `micromegas.audience` property, and the visibility rule for `'global'` partition rows (§4) | `group:everyone` | unset (unstamped data hidden) |
| `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` | register the five mutating UDTFs/UDFs (§4) for **non-admin** user sessions (admin sessions always get them — issue #1377) | `true` | unset/false |
| `MICROMEGAS_PUBLIC_VIEW_SETS` | §5b public view-set allowlist | — | optional |

`MICROMEGAS_UNSTAMPED_AUDIENCE` is the migration-pain killer: an open deployment can turn
enforcement on **before any stamping exists** — legacy `NULL`-audience data coalesces to
`group:everyone`, which every caller implicitly reads. No backfill, no retention wait, no
mode flip, nothing ever disappears.

**Activation story:** while the stages ship, absence of all isolation config = enforcement
inactive (exactly today's behavior, plus a startup warning once the machinery exists). At the GA
release the configuration becomes **required** — startup error if the operator has not chosen a
posture (deliberately no default; every operator makes a conscious choice). Release notes document
the two profiles above.

Wiring lives next to `default_provider::provider_with_prefix` (a `mint_policy()` / `read_policy()`
factory reading the env vars; `from_env` precedent: `static_tables_configurator.rs:44-54`). The
trait seam permits asymmetric policies later (e.g. group reads, self-only mint) with no code
change.

## Implementation Steps — staged rollout (revised 2026-07-30)

Re-ordered from the original Phases 1–5 around one constraint: **every stage ships with zero
behavior change** until an operator sets config, so existing team-wide deployments upgrade
untouched. Two tracks run in parallel — **enforcement** (Stages 1–3) and **keys/stamping**
(Stages 0, 4–6) — converging at activation (Stage 7). Each stage maps to one GitHub issue.

**Stage 0 depends on nothing** and is the only stage that delivers value on its own; everything
else is inert until an operator configures it. Stages 2, 3 and 4 depend on Stage 1; Stages 5 and 6
depend on Stage 0. Stage 0 now carries the key-management routes as well as the store, so Stage 6 is
reduced to audience resolution on an existing route plus the setup script.

### Stage 0 — DB-backed API key store + key-management API (no audience yet; independent of the AbAC seam)

Split out of the original Stage 4 (revised 2026-07-30). Moving the *key store* ahead of the policy
seam, and leaving only the *audience column* behind it, for four reasons:

- **Standalone value.** Stages 1–3 ship zero behavior change by design. This one fixes a live
  operational problem: keys are plaintext JSON in `MICROMEGAS_API_KEYS`, so grant/revoke means an
  env edit plus a redeploy of every service that validates keys — three construction sites
  (`telemetry-ingestion-srv/src/main.rs:51`, `flight_sql_server.rs:226`,
  `monolith/src/main.rs:193,207`). No revocation, no rotation, no per-key audit, no last-used.
- **The env keyring cannot survive Stage 6.** `ApiKeyAuthProvider::validate_request` deliberately
  scans **every** key with `ct_eq` on **every request** with no early exit
  (`api_key.rs:100-129`) — correct for a handful of team keys, but the mint route lands here and
  Stage 6 turns it into one key per user/machine, taking N from ~5 to thousands and charging
  ingestion N constant-time compares per request. A hash-indexed lookup is O(1). The store is
  therefore a *precondition* for minting at all, not a convenience.
- **It is the long pole of the keys track** (Stages 5 and 6 both hang off it) and the migration
  vehicle for open deployments — the earlier it lands, the longer it soaks before anything depends
  on it.
- **It does not shorten the path to enforcement.** This is parallel work, deliberately.

#### Two tables, split by risk class (decided 2026-07-30)

Write credentials and read credentials live in **separate tables** — `ingestion_api_keys` and
`analytics_api_keys` — and **one key is never valid on both surfaces**.

- **The security model is asymmetric, so the boundary should be too.** Per "Load-bearing property
  preserved", a stolen write key is an *integrity* problem: it pollutes an audience's view and
  grants zero read power. A read credential is a *confidentiality* problem. Stage 6 puts one
  ingestion key per user/machine on dev boxes and game clients, embedded in
  `OTEL_EXPORTER_OTLP_HEADERS` — the most widely distributed and most exposed credentials in the
  deployment, each needing exactly one surface. A shared table makes every one of them a read
  credential too, which is the wrong blast radius. API keys also carry `allow_delegation: true`
  (`api_key.rs:126`), so a leaked key can attribute traffic to arbitrary users
  (`user_attribution.rs:154-174`).
- **The columns genuinely differ.** Only ingestion keys carry an `audience` (Stage 4); read scope is
  resolved from the caller's OIDC identity and never from a key. In one table `audience` would be
  meaningful for exactly one discriminant value — a discriminated union in nullable columns, where
  the discriminant carries no information the table name doesn't.
- **Postgres grants enforce it, not application logic.** The ingestion role holds `INSERT` on
  `ingestion_api_keys` and **never** on `analytics_api_keys`, so a defaulting bug anywhere in the
  mint path (0c) cannot issue a read credential — fail-closed by schema rather than by code that has
  to stay correct. Each validating service needs only `SELECT` plus column-level
  `UPDATE (last_used_at)` on the one table it reads.
- **Shared code, not a shared relation.** The hash lookup, the `moka` cache and the `last_used_at`
  write are identical, so one `DbApiKeyAuthProvider` parameterized by table serves both. That is
  where the single-implementation value lives.
- **Rejected: one table with a `scopes` column.** Once "never both" is a rule the column is always a
  singleton, and it downgrades a schema-level boundary to an application-level check on the mint
  path. Note the env layout can *already* express disjoint sets per surface
  (`MICROMEGAS_INGESTION_API_KEYS` vs `MICROMEGAS_ANALYTICS_API_KEYS`, `default_provider.rs:53-59`),
  so the split preserves an existing capability rather than inventing one — but see 0d for the one
  property it deliberately breaks.
- **Consequence worth noting: `analytics_api_keys` may be transitional.** Read grants derive from the
  IdP `groups` claim (Stage 1, step 5) and an API key carries no claim, so a key-authenticated
  query's readable set is implicit-groups-only — empty in a privacy deployment. Analytics keys are
  therefore either a migration artifact or exist purely for the delegation path. Separate tables make
  that outcome cheap: the read table can be deprecated or dropped without touching the ingestion hot
  path.
- **Open item:** `object-cache-srv` is a fourth key-validating surface (`cli.rs:59`, same
  `parse_key_ring`). It is read-class — it serves raw payload ranges, and Prong B guards
  `get_payload` — but its keys are service-held rather than user-held. Decide whether it validates
  against `analytics_api_keys` or gets its own table before implementing 0a.

#### Key management is an HTTP API, not SQL (decided 2026-07-30)

Create/revoke/list are OIDC-authenticated routes on the ingestion service (0c), pulled forward from
Stage 6 — **the table alone does not deliver Stage 0's claimed value**, since without an endpoint an
operator still cannot revoke anything without hand-written SQL against Postgres. Stage 6 then extends
the create route with audience resolution rather than introducing it.

**Rejected: admin-gated lakehouse UDFs** (`revoke_api_key(...)`, `import_api_key(...)`) on the
flight-sql path, despite the good precedent for admin-gated mutating functions (#1382,
`query.rs:150-165`, and API keys can never be admin — `api_key.rs:124`). Two reasons:

1. **Query text is logged and micromegas ingests its own logs.**
   `flight_sql_service_impl.rs:330` emits `sql={sql:?}` at info, and `:841` logs prepared-statement
   text, so any key material passed as a SQL literal lands in `log_entries` — readable by anyone with
   query access, and strictly worse than the env var this stage is replacing. Hashing client-side and
   passing only the digest would neutralize this, but then:
2. **A write UDF hands the *read* service write access to the key tables**, trading away the
   schema-level boundary above for operator convenience. And since the mint API is needed for Stage 6
   regardless, the UDF path buys nothing that the HTTP route does not already provide.

Steps:

0a. **Two tables** in the telemetry DB (migration precedent: `sql_telemetry_db.rs:5-12`), same shape:

```sql
CREATE TABLE ingestion_api_keys (
  key_hash     BYTEA PRIMARY KEY,      -- sha256 of the full key string
  name         VARCHAR NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL,
  last_used_at TIMESTAMPTZ,
  revoked_at   TIMESTAMPTZ
);
-- analytics_api_keys: identical, and it never gains the Stage 4 audience column
```

Do **not** store the key in cleartext — a plaintext column is strictly worse than the env var
(backups, replicas, read access, query logs). `key_hash` as primary key gives the O(1) lookup. The
import requirement (existing key strings keep working, 0d) rules out imposing a `key_id.secret`
shape, so lookup is by hash of the whole string. SHA-256 without a KDF is safe **only** because
these are high-entropy random keys, not passwords — Argon2 would be both unindexable and too slow
per request. Pair this with rotating any legacy key that is not actually random.

0b. `DbApiKeyAuthProvider::new(pool, table)` composed via the existing `MultiAuthProvider`; env
    keyring and DB keyring compose during transition, so nothing breaks mid-migration. Requires
    threading a connection pool into `default_provider::provider_with_prefix`, which is a pure env
    factory today (`default_provider.rs:51`) — the first real API change in the auth crate; three
    call sites, and the two that pass an empty prefix (`telemetry-ingestion-srv/src/main.rs:51`,
    `flight_sql_server.rs:226`) must now name their table explicitly, since `""` cannot be inferred.
    Cache lookups in a bounded `moka` with a short TTL (same pattern as the §4 caches): a per-request
    DB hit on the ingestion hot path is not acceptable. **State the consequence as a property, not
    an accident: revocation takes effect within the cache TTL** — the endpoint below writes
    `revoked_at` and cannot invalidate remote caches.

0c. **Key-management API** on the ingestion service (and the monolith by inheritance),
    OIDC-authenticated and admin-gated:
    - `POST /auth/api_keys` — generate a fresh random key, store its hash, return the cleartext
      **once**. Writes `ingestion_api_keys`; no audience until Stage 4.
    - `DELETE /auth/api_keys/<id>` — set `revoked_at`. This is the operation that carries the stage
      (the 2am revoke with no redeploy).
    - `GET /auth/api_keys` — name, created_at, last_used_at, revoked_at. **Never the hash**; there is
      no reason to hand out the lookup value even though it is not reversible.

    **Analytics keys are not mintable through this API.** They are few, manually issued (0d, or
    direct SQL by an operator with DB access) and stay out of every HTTP write path: issuing read
    credentials from the fleet-facing service is the wrong direction for the asymmetry, and keeping
    them out is what confines the ingestion role's grants to one table.

    The route mildly expands the surface of the most exposed process. Accepted deliberately — it is
    where the DB grant belongs, OIDC is required, and no API key can be admin (`api_key.rs:124`), so
    no key can mint another.

0d. **Import tool** (python, per repo scripting convention) — a one-shot migration for legacy key
    strings, the one thing the mint route cannot do (it generates fresh keys). Deletable once each
    deployment has run it. Reads the env keyring, computes hashes, inserts rows, and **requires an
    explicit destination table per key with no default.** The prefixed vars map cleanly:
    `MICROMEGAS_INGESTION_API_KEYS` → ingestion, `MICROMEGAS_ANALYTICS_API_KEYS` → analytics.

    **The unprefixed fallback is the one behavior change in this stage.** In every split deployment
    both `telemetry-ingestion-srv/src/main.rs:51` and `flight_sql_server.rs:226` read the unprefixed
    `MICROMEGAS_API_KEYS`, so *every existing key is currently valid on both surfaces*. "Never both"
    cannot preserve that: a genuinely dual-use key must become two keys, and any client that used one
    key for both ingestion and queries must be updated. This is the single place the
    zero-client-change claim does not hold, and the Stage 7 migration guide must say so explicitly
    rather than leaving operators to discover it.

### Stage 1 — Policy seam + identity threading (no enforcement yet)
1. **Policy traits + AbAC impls.** Add `MintPolicy`, `ReadPolicy`, `ReadScope` in `rust/auth/src/`
   (e.g. `policy.rs`); add `AudienceMintPolicy` / `AudienceReadPolicy` (§1–2). No `Self*` impls — per-user
   is the AbAC engine with empty grants.
2. **AuthContext fields.** Add `bound_audience: Option<String>` and `groups: Vec<String>` to
   `AuthContext` (`rust/auth/src/types.rs`); populate `None`/`[]` everywhere except the key path
   (Stage 4) and OIDC. **Groups claim (low effort, confirmed):** add `groups: Option<Vec<String>>`
   to the `Claims` struct (`oidc.rs:193-227`) — no `#[serde(deny_unknown_fields)]`, so it is
   backward-compatible and absent-claim-safe; populate at the OIDC construction site
   (`oidc.rs:536-545`). Flat top-level array covers Auth0/Azure AD/Google (the confirmed targets);
   Keycloak's nested `realm_access.roles` is not a current target and would need a nested helper.
3. **Thread identity.** Add `read_scope` param to `make_session_context` (`query.rs:194`) and feed
   `register_lakehouse_functions` (`query.rs:96`). Resolve scope via `ReadPolicy` in
   `flight_sql_service_impl` and pass through both call sites (`:372`, `:842`). **Close the two
   identity holes** (§6): resolve identity on the prepared-statement path, and never derive
   `ReadScope` from client-claimed attribution; carry `groups` across the `AuthService` boundary.
4. **Config factory.** Parse the grant knobs (Config surface) next to
   `default_provider::provider_with_prefix`; `from_env` precedent
   `static_tables_configurator.rs:44-54`. Unset ⇒ enforcement inactive (transitional).
5. **Policy source (decided): IdP `groups` claim + `MICROMEGAS_IMPLICIT_GROUPS` only.** No local
   grants table in v1 — confidentiality rests solely on OIDC plus operator config; no TCB
   additions. Precedent: the `MICROMEGAS_ADMINS` allowlist (`oidc.rs:264-394`).
   **Consequence — write/read collapse to membership:** membership in `G` grants *both* `read:G`
   and `write:G`. Separately grantable write-only/read-only needs a richer source (a second role
   claim, or a Postgres grants table putting its editors in the TCB) and stays a **pure addition**
   behind the same seams.

### Stage 2 — Enforcement Prong A (inactive until configured)
6. Add `OwnershipRewrite` in `rust/analytics/src/lakehouse/ownership_rewrite.rs`, constructed from
   `ReadScope` + the unstamped-audience + public-view-set config. Inject the audience predicate
   (with `coalesce` semantics, §4) on the `processes` view, the semi-join on `process_id`-keyed
   views, and `view_instance` (caught as a `TableScan<MaterializedView>`). Branch per view set via
   `MaterializedView::get_view_set_name()` (§5b). **Register unconditionally** — unlike
   `TableScanRewrite`, which is added only when `query_range.is_some()` (`query.rs:206`).
7. **Test with audience stamped manually** (before ingestion stamping exists): seed processes with
   a `micromegas.audience` property; assert cross-audience queries return nothing, same-audience
   returns its own rows, unstamped rows follow the `MICROMEGAS_UNSTAMPED_AUDIENCE` rule, and the
   daemon (`ReadScope::All`) returns everything.

### Stage 3 — Enforcement Prong B (inactive until configured)
8. Thread `ReadScope` into each affected function. Arg-addressed (`process_spans`,
   `perfetto_trace_chunks`, `parse_block`, **`get_payload`**) verify the named process's audience
   at async scan time, failing closed; `list_partitions` row-filters by readable audience incl.
   the `'global'`-row rule (§4); the five mutating functions are registered only for maintenance
   contexts, admin sessions, or under `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS=true` (the admin arm
   is issue #1377 and may land ahead of this stage). Build the `moka` caches
   (`process_id → audience`, `stream_id → process_id`, `block_id → process_id`). Internal
   maintenance contexts get `ReadScope::All`; the three user-reachable recursive context sites
   inherit the caller's scope (§5).

### Stage 4 — Audience on keys (the open-deployment migration vehicle)

Reduced to the part that genuinely needs the AbAC seam (revised 2026-07-30); the key store itself
is Stage 0. Depends on Stage 0 (the tables) and Stage 1 (the audience shape).

9. Add `audience VARCHAR NOT NULL` to `ingestion_api_keys` — **a column, not a mapping table.** The
   binding is 1:1 and immutable (§3: `resolve_audience` runs once at mint and the result is recorded
   on the key; `bound_audience` is single-valued), so `NOT NULL` is fail-closed by construction,
   whereas a 1:1 side table would add a join to the hot auth path and admit a key-with-no-audience
   state that ingestion must remember to reject. A separate table only becomes the right shape if
   audiences gain their own metadata/grants or a key can bind more than one. `analytics_api_keys`
   gets **no** audience column — read scope comes from the caller's OIDC identity, never from a key.
   The ingestion `DbApiKeyAuthProvider` now produces `AuthContext { bound_audience: Some(audience),
   email: Some(...), allow_delegation: false, is_admin: false }`.
10. Extend the Stage 0 import tool to assign an audience: existing keys land as `group:everyone`
    (or a per-key choice) — still zero client changes; this is how open deployments migrate.
    Keys imported before this stage get the configured default on backfill.

### Stage 5 — Ingestion stamping
11. Read `AuthContext.bound_audience` in native + OTLP handlers; write `micromegas.audience` onto
    the process; demote client-supplied owner fields to display metadata. **Defines the OTLP /
    Firehose auth story**: OTLP handlers currently have no auth wiring at all, and Firehose routes
    are merged outside the protected router (`ingestion.rs:151-156`) — both must carry an
    authenticated `bound_audience` before stamping is meaningful there.

### Stage 6 — Audience resolution on mint + setup script (enables real per-user keys)
12. Extend the Stage 0 `POST /auth/api_keys` route with `MintPolicy::resolve_audience`
    (`AudienceMintPolicy`, §1): the request may name a `requested` audience, the policy vets it, and
    the resolved value is written to the key's `audience` column. The route, its OIDC auth and its
    DB grant already exist from Stage 0 — this stage adds only the policy call.
13. Setup script: OIDC device-code/loopback flow → mint → write OTLP exporter env
    (`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS=authorization=Bearer <key>`).

### Stage 7 — Activation, docs, integration tests
14. Make the isolation config **required at startup** (no default — startup error if unset);
    mkdocs isolation page + deployment/migration guide for the two profiles; two-audience
    integration tests per Testing Strategy, including **open-profile equivalence** (nothing
    hidden, maintenance functions present, `'global'` rows visible).

**Deployment stories:**
- *Team/open*: upgrade → import keys into the two tables, choosing a destination per key (Stage 0;
  any key that was used for **both** ingestion and queries splits into two keys — the one
  client-visible change) → stamp the ingestion keys `group:everyone` (Stage 4) → set the three knobs
  → identical behavior forever; no flip, no backfill, nothing disappears.
- *Privacy*: key store + management API (Stage 0) → audience on ingestion keys (Stage 4) → audience
  resolution on mint (Stage 6) → users mint personal ingestion keys (data stamped `user:<email>`) →
  set restrictive config (no implicit groups, no unstamped audience) → per-user isolation; team
  sharing via the IdP groups claim.

### Later — (optional) physical boundary
15. Promote `micromegas.audience` to a first-class `audience` column; propagate through views;
    enable partition pruning and per-audience object-storage prefixing.

## Files to Modify

- Auth: `rust/auth/src/types.rs` (`bound_audience`, `groups`), `rust/auth/src/policy.rs` (new —
  traits + `Audience*` impls), `rust/auth/src/default_provider.rs` (policy factory / grant knobs),
  `rust/auth/src/oidc.rs` (groups claim, Stage 1), `rust/auth/src/user_attribution.rs` (never feed
  client-claimed identity into scope resolution, Stage 1), `rust/auth/src/api_key.rs` + new
  `db_api_key.rs` (Stage 0 — one provider parameterized by table; `audience` column on
  `ingestion_api_keys` only, Stage 4).
- Analytics (Prong A): `rust/analytics/src/lakehouse/ownership_rewrite.rs` (new),
  `rust/analytics/src/lakehouse/query.rs` (`make_session_context` + `register_lakehouse_functions`
  signatures), `rust/analytics/src/lakehouse/processes_view.rs` (audience exposure if promoted).
- Analytics (Prong B — UDTF/UDF guards): `rust/analytics/src/lakehouse/process_spans_table_function.rs`,
  `perfetto_trace_table_function.rs`, `parse_block_table_function.rs`,
  `list_partitions_table_function.rs`, and their execution plans (scan-time audience check);
  the `get_payload` UDF (same guard, scalar form); `retire_partitions_table_function.rs`,
  `materialize_partitions_table_function.rs`, `regenerate_partitions` and the
  `retire_partition_by_file` / `retire_partition_by_metadata` UDFs (registration gate instead of an
  audience check). Internal-context sites in `view.rs`, `merge.rs`, `sql_batch_view.rs`,
  `export_log_view.rs`, `batch_partition_merger.rs`, `metadata.rs` (`ReadScope::All`); recursive
  context sites in the three UDTF execution plans (inherit caller scope).
- Query service: `rust/public/src/servers/flight_sql_service_impl.rs` (resolve scope, pass through
  both call sites incl. prepared statements).
- Ingestion: `rust/public/src/servers/ingestion.rs`, `rust/public/src/servers/otlp.rs` (incl.
  auth wiring; Firehose route placement), `rust/ingestion/src/sql_telemetry_db.rs` (audience
  storage).
- Key store, key-management routes (create/revoke/list) + monolith wiring:
  `rust/ingestion/src/sql_telemetry_db.rs` (the two tables), `rust/public/src/servers/…`,
  `rust/monolith/src/main.rs`, import script (python, per repo scripting convention). Possibly
  `rust/object-cache-srv/src/object_cache_srv.rs` + `cli.rs`, pending the open item in Stage 0 on
  which table that service validates against.

## Trade-offs

- **Set-valued rule from day one** vs. a per-user equality now, generalize later. Chosen: set-valued.
  The singleton `IN` costs nothing at runtime and is the one decision that prevents a rewrite; a
  boolean `owner = caller` special-case is exactly the corner to avoid.
- **`ReadScope::All` variant** vs. a wildcard principal string. Chosen: explicit enum — no sentinel
  that could collide with a real audience or be forged into a filter.
- **Everyone-group over a wildcard read grant** (decided 2026-07-30) for open deployments. A
  user-grantable `ReadScope::All` would be exactly today's behavior, but it forks the model into
  "filtered" and "unfiltered" deployments. Chosen instead: open = `group:everyone` implicit
  membership + `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` — one uniform data model where every
  deployment runs the same filtered path. The behavioral deltas that choice creates (unstamped
  legacy data, `'global'` partition rows, mutating functions) are each closed by a dedicated knob
  (Config surface) so open deployments still see byte-for-byte today's behavior.
- **No default policy / required config at GA** (decided 2026-07-30) vs. defaulting to `self` or to
  open. A `self` default breaks every existing deployment on upgrade (all data invisible); an open
  default makes the privacy posture opt-in forever. Chosen: no default — transitional
  "unset = inactive" while stages ship, then a startup error forcing a conscious operator choice.
- **Query-time coalesce for unstamped data** vs. a backfill script vs. waiting out retention.
  Chosen: `MICROMEGAS_UNSTAMPED_AUDIENCE` (query-time attribution). No data mutation, no
  re-materialization of `processes` partitions (which a property backfill would require), works the
  instant enforcement turns on. Privacy deployments leave it unset (fail-closed).
- **Eternal write keys / no `minted_by` in v1.** Accepts that revoking a subject's `write(→G)` does
  not retroactively invalidate keys already minted for G — the key *is* the frozen grant; to undo it
  you revoke the key. This matches the stated use case. If retroactive write-revocation is ever
  needed, add `minted_by` to `ingestion_api_keys` and revoke by `(minted_by, audience)` — an additive
  change.
- **Separate key tables per risk class** vs. one table with a scope column (decided 2026-07-30).
  Chosen: separate. Costs a second migration and forces dual-use keys to split at import (the one
  client-visible change in Stage 0); buys a boundary enforced by Postgres grants rather than by
  application logic on the mint path, and avoids an `audience` column that is meaningful for only
  half the rows. Rationale in Stage 0.
- **Key management over HTTP** vs. admin-gated lakehouse UDFs (decided 2026-07-30). Chosen: HTTP.
  The UDF route would put key material in `sql={sql:?}` logs that micromegas itself ingests
  (`flight_sql_service_impl.rs:330`) and would require granting the read service write access to the
  key tables; the mint API is needed for Stage 6 anyway.
- **Policy source (decided): IdP `groups` claim + implicit-groups config only.** Keeps
  confidentiality resting on OIDC plus operator config; no TCB additions. Trade-off accepted:
  membership grants both read and write for a group (no independent write-only/read-only). A local
  grants table (more expressive, but its editors join the TCB) is a deferred pure addition, not
  part of v1.
- **Reserved property vs. first-class column** for the audience (v1 vs the later physical
  boundary): row-level filter now with zero migration, physical pruning later.
- **Public views opt-in (§5b)** vs. keeping every aggregate private. Chosen: opt-in allowlist,
  default empty. Reuses Prong A's existing per-view-set branch (`get_view_set_name`), so it adds a
  config knob rather than a new enforcement seam, and stays fail-closed until an operator names a
  view set. Deferrable past v1 with no rework.

## Security

- Confidentiality = OIDC + `ReadPolicy` per query; write-key theft is integrity-only.
- No write→read escalation (audience label ≠ read grant).
- Metadata tables/functions **must** be covered by **both** prongs or they leak process names,
  machine names, and `otel.resource.*` properties even while log bodies are hidden. Prong A covers the
  views; Prong B covers the span/metadata UDTFs the analyzer physically cannot filter. This is the
  primary correctness risk and the focus of testing.
- The five mutating functions (`retire_partitions`, `materialize_partitions`,
  `regenerate_partitions`, `retire_partition_by_file`, `retire_partition_by_metadata`) are not read
  paths; they are excluded from user sessions (registered only for maintenance contexts, admin
  sessions — issue #1377, which also closes the pre-isolation hole where every authenticated
  caller can invoke them — or under the explicit `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS=true`
  opt-in that open deployments use to keep them for non-admins) rather than audience-filtered — an
  integrity/availability control, not a confidentiality one. Without it, a non-admin could name
  another principal's `process_id` via `retire_partitions`' `view_instance_id` argument to destroy
  their partitions.
- **Identity holes closed in Stage 1** (would otherwise be full enforcement bypasses): the
  prepared-statement path resolves no identity (`flight_sql_service_impl.rs:842`), and
  `validate_and_resolve_user_attribution_grpc` falls back to client-claimed identity when the
  `x-auth-subject` header is absent (`user_attribution.rs:125-133`) — `ReadScope` is derived from
  the authenticated `AuthContext` only, never from client-claimed attribution.
- `MICROMEGAS_IMPLICIT_GROUPS` and `MICROMEGAS_UNSTAMPED_AUDIENCE` are deliberate, operator-owned
  confidentiality relaxations (like §5b): setting them widens what every authenticated caller can
  read. Both are unset in a privacy deployment; the engine is fail-closed without them.
- No admin query-path read bypass — admin FlightSQL sessions are filtered like any other. Cross-
  principal reads for operators are an out-of-band capability (direct object-store/parquet access),
  intentionally outside the query path. API keys can never be admin.
- Group grants add a single trust dependency: the IdP's `groups` claim (plus operator-set implicit
  groups). No local policy store, so the TCB gains no new members.
- Public views (§5b) are an explicit, opt-in confidentiality relaxation: a listed view set is
  readable by every authenticated caller, so only genuinely aggregated / non-PII view sets may be
  listed. The default allowlist is empty (fail-closed); the raw global `log_entries` / `measures`
  instances must never be listed, and the arg-addressed process-scoped UDTFs are never exempted.

## Testing Strategy

- **Key store + management API (Stage 0, independent of everything below):** a DB key authenticates
  and an unknown key is rejected; a revoked key stops authenticating within the cache TTL (assert the
  stated revocation-latency property, don't leave it implicit); env keyring and DB keyring compose — a
  key in either authenticates during the transition; the import tool round-trips existing prefixed
  `MICROMEGAS_*_API_KEYS` entries so the *same key strings* still authenticate on their own surface
  afterwards (the zero-client-change claim, so it deserves a real test); no cleartext key is stored —
  assert the column holds the hash.
  **Surface separation (the load-bearing property of the split):** a key in `ingestion_api_keys` is
  rejected by flight-sql and a key in `analytics_api_keys` is rejected by ingestion — assert both
  directions, since a provider constructed against the wrong table is the failure mode the two-table
  design exists to prevent. Assert the mint route writes `ingestion_api_keys` only and that no route
  inserts into `analytics_api_keys`.
  **Management routes:** create returns a key that then authenticates; the cleartext is returned once
  and never retrievable afterwards; list omits the hash column; revoke is idempotent; every route
  rejects an API-key-authenticated caller (admin requires OIDC, `api_key.rs:124`).
- **Unit:** `AudienceMintPolicy` rejects `requested` outside the mintable set and defaults to
  `user:<email>`; `AudienceReadPolicy` returns `{user:} ∪ claim groups ∪ implicit groups` and the
  singleton when both group sources are empty. Prong A: `OwnershipRewrite` injects the expected
  predicate per table kind (snapshot the rewritten logical plan), including `view_instance` and the
  `coalesce` form when `MICROMEGAS_UNSTAMPED_AUDIENCE` is set. Prong B: each guarded UDTF/UDF
  (incl. `get_payload`) rejects an unowned `process_id`/`block_id` and `list_partitions`
  row-filters — assert both fail closed; assert all five mutating functions are absent
  ("function not found") from a registration built with any non-`All` `ReadScope` for a non-admin
  session unless `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS=true`, and present for an admin session
  regardless of the knob (#1377). Public views (§5b): with a view
  set on the allowlist, `OwnershipRewrite` injects no predicate for it and `list_partitions` shows
  its `'global'` rows; with an empty allowlist behavior is unchanged (every set filtered).
- **Integration (privacy profile):** two audiences seeded; assert each sees only its own rows
  across `processes`, `log_entries`, `measures`, spans, `view_instance`, `list_partitions`; assert
  the `process_id` semi-join blocks naming another audience's process directly; assert unstamped
  rows are hidden; assert the daemon (`ReadScope::All`) sees everything and that an **admin user
  session is still filtered** (no bypass); assert the prepared-statement path is filtered
  identically to `do_get`.
- **Integration (open profile — equivalence with today):** with `MICROMEGAS_IMPLICIT_GROUPS=everyone`,
  `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone`, `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS=true` and
  a mix of stamped (`group:everyone`) and unstamped data: every caller sees every row, `'global'`
  partition rows are listed, and the mutating functions are registered — byte-for-byte the
  pre-isolation behavior.
- **Equivalence (per-user plan):** confirm the executed plan in the privacy profile matches the
  intended per-user filter (a singleton `IN`), i.e. no behavioral difference from a hand-written
  per-user design.
- **Group grants:** group member reads group data; write-only producer (`write(→G)`, no
  `read(→G)`) cannot read G including its own writes; membership change reflected on next query.
- Rust: `cargo test`, `cargo clippy --workspace -- -D warnings`, `cargo fmt`; CI via
  `python3 build/rust_ci.py`.

## Documentation

- New page under `mkdocs/docs/` for the isolation model: the three-relation model, the grant knobs
  and the two deployment profiles (open / privacy), the required-at-GA activation story, the
  `MICROMEGAS_PUBLIC_VIEW_SETS` allowlist (§5b, with its non-PII caveat), and the
  confidentiality/integrity properties.
- Update any auth/deployment docs to mention the two key tables and why they are separate, the
  key-management routes (create/revoke/list) and the revocation-latency property, the import tool
  **including the dual-use-key split**, the setup script, and the groups-claim configuration.

## Resolved Decisions

Resolved by research (kept here for the record; details in Appendix A):
- ~~**Table-function coverage.**~~ **Resolved:** two-pronged — analyzer rule for `MaterializedView`
  scans incl. `view_instance` (Prong A); construction-time guard threaded with `ReadScope` for the
  span/metadata UDTFs (Prong B), with the audience check at async scan time. A single analyzer rule is
  provably insufficient (owner id absent from schema, opaque in provider, filters ignored).
- ~~**Audience identifier shape.**~~ **Resolved:** value-prefix `user:<email>` / `group:<id>` in a
  single dotted-namespace property `micromegas.audience` (matches `otel.resource.*` convention; no
  collision possible).
- ~~**Audience storage for v1.**~~ **Resolved:** reserved property `micromegas.audience`; in-tree
  usage of `property_get` in WHERE predicates is equality only; the `IN (...)` form relies on
  DataFusion's dictionary-type coercion (`property_get` returns `Dictionary(Int32, Utf8)`). Promote to
  a column in the later physical-boundary stage.
- ~~**Groups-claim feasibility.**~~ **Resolved:** one-line additive `Claims`/`AuthContext` change,
  backward-compatible; Auth0/Azure AD/Google flat arrays; `MICROMEGAS_ADMINS` is the config precedent.
- ~~**Grant source.**~~ **Decided: IdP `groups` claim (+ implicit-groups config) only** (no
  local grants table in v1). Keeps confidentiality on OIDC and the TCB unchanged; accepted
  trade-off is that membership grants both read and write for a group. A grants table (or a second
  write-role claim) is a deferred pure addition. See Stage 1 step 5.
- ~~**Admin read bypass.**~~ **Decided: no query-path bypass.** `is_admin` does not map to
  `ReadScope::All`; admin sessions are filtered like any other. Operators needing cross-principal
  reads use direct object-store/parquet access, which they already have — a query bypass would add
  attack surface and audit burden for no confidentiality gain. Only the maintenance daemon is
  unfiltered. See §5.
- ~~**`list_view_sets` exposure.**~~ **Decided: stays unfiltered** — view-set schema/definitions only,
  no PII or per-principal data. Only `list_partitions` is row-filtered. See §4 Prong B.
- ~~**`retire_partitions` / `materialize_partitions` exposure.**~~ **Decided (revised 2026-07-30):
  registered for maintenance contexts, admin sessions (issue #1377), or under
  `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS=true`.** Both were missing from
  the original Prong B audit despite being registered unconditionally alongside the other UDTFs;
  the 2026-07-30 audit added `regenerate_partitions` and the `retire_partition_by_file` /
  `retire_partition_by_metadata` UDFs to the set. All mutate lakehouse state, so none gets an
  audience read-filter — instead `register_lakehouse_functions` skips registering them for
  non-admin user sessions unless the deployment opts in (the knob open deployments set to keep
  them for non-admins). The admin arm also closes a pre-isolation hole (today every authenticated
  caller can invoke them) and may land first. See §4 Prong B and Appendices A–B.
- ~~**Scan-time check cost.**~~ **Resolved:** `process_id → audience` is immutable, so an
  invalidation-free size-bounded `moka` cache (backed by `find_process`) makes the check an O(1)
  in-memory lookup on warm hits, one indexed PG query per process ever on cold miss. `ReadScope` is
  free (from the JWT `groups` claim). See §4 "Prong B performance".

Decided 2026-07-30 (deployment-staging revision, from issue #1334 follow-up discussion):
- **No mode enum, no default policy.** The former `MICROMEGAS_ISOLATION_POLICY=self|audience` knob and
  its `self` default are gone. One AbAC engine configured by grants; per-user isolation is the
  empty-grants configuration, not a separate mode. Transitional unset-config = enforcement
  inactive; at GA the config is required at startup.
- **Open deployments = everyone-group configuration** (chosen over a user-grantable
  `ReadScope::All` wildcard): implicit `everyone` membership, imported keys stamp
  `group:everyone`, unstamped data coalesces to `group:everyone`. One uniform filtered path for
  every deployment.
- **Existing API keys are imported into the DB key store** (same key strings, audience
  `group:everyone`) — the migration vehicle for team deployments; zero client changes except for
  dual-use keys (see below). The key store therefore moves early in the ordering — **Stage 0**,
  ahead of the policy seam and depending on nothing (revised 2026-07-30); only the `audience` column
  stays behind the seam, as Stage 4.
- **Write and read keys live in separate tables** (`ingestion_api_keys` / `analytics_api_keys`,
  decided 2026-07-30, issue #1383): one key is never valid on both surfaces. The risk is asymmetric
  (write = integrity, read = confidentiality) and Stage 6 distributes ingestion keys to thousands of
  machines, so the boundary is enforced by Postgres grants — the ingestion role never holds `INSERT`
  on the analytics table. Only ingestion keys carry an `audience`. Consequence: a key currently used
  for both ingestion and queries (the unprefixed `MICROMEGAS_API_KEYS` fallback) must split into two
  at import — the one place zero-client-change does not hold.
- **Key management is an OIDC-authenticated HTTP API on the ingestion service**, not admin-gated
  lakehouse UDFs (decided 2026-07-30). Create/revoke/list move into **Stage 0**, since the table
  alone does not deliver the revoke-without-redeploy value; Stage 6 only adds audience resolution to
  the existing create route. UDFs were rejected because query text is logged into micromegas's own
  `log_entries` (`flight_sql_service_impl.rs:330`) and because a write UDF would grant the read
  service write access to the key tables.
- **Unstamped data: query-time coalesce knob** (`MICROMEGAS_UNSTAMPED_AUDIENCE`), not a backfill
  and not a retention wait. Unset = hidden (fail-closed, privacy profile).
- **Mutating functions: registration gate — maintenance ∨ admin ∨ deployment opt-in**
  (`MICROMEGAS_USER_MAINTENANCE_FUNCTIONS`) rather than unconditionally maintenance-only. Admin
  sessions always get them (issue #1377 — standalone, closes today's
  any-authenticated-caller hole, may land before the isolation stages); the knob keeps them
  available to non-admins in open deployments.
- **Prong B coverage extended** after the 2026-07-30 drift audit: `regenerate_partitions`,
  `retire_partition_by_file`, `retire_partition_by_metadata` join the mutating set; `get_payload`
  gets the arg-addressed read guard. See Appendix B.
- **Identity holes are in scope for Stage 1**: prepared-statement path identity resolution and the
  client-claimed-attribution fallback (never feeds `ReadScope`).

All design decisions are closed but one: **which key table `object-cache-srv` validates against**
(its own, or `analytics_api_keys`) — see the open item in Stage 0; it needs an answer before 0a is
implemented, and it does not block any other stage. Remaining work is implementation, staged per the
Implementation Steps (Stage 0 and Stage 1 are both unblocked and independent of each other).

## Appendix A — Research findings (2026-07-21)

Grounded against the current tree; file:line refs verified.

**Table functions (DataFusion 54.0).** UDTFs are registered via `ctx.register_udtf` in
`register_lakehouse_functions` (`query.rs:102-155`) and resolve at SQL-planning time into
`LogicalPlan::TableScan` nodes wrapping a `DefaultTableSource(provider)` — so an `AnalyzerRule` *can*
see them. But:
- `view_instance` returns a `MaterializedView` (`view_instance_table_function.rs:76`) → already
  rewritten by `TableScanRewrite`; Prong A handles it.
- `process_spans` (`ProcessSpansTableProvider`, `process_spans_table_function.rs:366`),
  `perfetto_trace_chunks`, `parse_block`, `list_partitions`, `list_view_sets` do **not** expose their
  owner id in the output schema, and bake `process_id`/`stream_id` opaquely into the provider at plan
  time via `exp_to_string` (`process_spans_table_function.rs:110`, `perfetto:71`). `scan()` even
  ignores `_filters` (`process_spans_table_function.rs:384`). ⇒ predicate injection is impossible for
  these; **guard-at-construction (Prong B)** is the only uniform enforcement point. `call_with_args`
  is synchronous → pass a pre-resolved `ReadScope`, defer the metadata-dependent check to async scan.
- `process_thread_spans_table_function.rs~` is a dead backup (not compiled/registered) — ignore.
- `retire_partitions` (`query.rs:119-122`) and `materialize_partitions` (`query.rs:131-137`) are the
  remaining two of the eight registered UDTFs and were absent from the original audit above. Neither
  is a read. `retire_partitions` deletes `lakehouse_partitions` rows for a
  `(view_set_name, view_instance_id)` pair (`write_partition.rs:116`) — destructive — and
  `view_instance_id` is a `process_id` for process-scoped view sets, so it has the same opaque,
  unchecked argument shape as `process_spans`. `materialize_partitions` takes no per-process id — it
  materializes a *global* view (`view_factory.get_global_view`) over an insert-time range — so it
  can't target another principal's data but is still an unbounded write with no read-session use
  case. **Decided:** gate registration of both on `ReadScope::All` (maintenance-only) rather than
  extending the audience check to them — they're integrity/availability concerns, not confidentiality
  ones.
- Identity is already resolved at `flight_sql_service_impl.rs:317` but currently used only for audit;
  it is not passed to `make_session_context`/`register_lakehouse_functions`.

**OIDC claims (`oidc.rs`).** `Claims` struct at `:193-227`; `get_email()` priority chain at
`:232-240`; no `#[serde(deny_unknown_fields)]` ⇒ adding `groups`/`roles` is additive and
absent-safe. `is_admin` = allowlist match on `sub`/email from `MICROMEGAS_ADMINS`
(`load_admin_users` `:264-269`, check `:390-394`) — the precedent for group→capability config.
Targets: Auth0, Azure AD, Google (Keycloak nested claims not currently targeted). `AuthContext`
(`types.rs:14-37`) has no groups field yet — must be added for AbAC grants.

**Properties.** `micromegas_property = (key TEXT, value TEXT)` (`sql_telemetry_db.rs:17`),
`processes.properties micromegas_property[]` (`:39`) — arbitrary strings, no per-key typing.
`property_get` returns `Dictionary(Int32, Utf8)`, case-insensitive key match, `NULL` when absent
(`rust/datafusion-extensions/src/properties/property_get.rs:48,87-92`); used in WHERE across the
codebase (e.g. `rust/public/src/client/query_processes.rs:73`). No
`micromegas.` reserved-key convention exists yet, but `otel.resource.*` is the established dotted
namespace (`otel-ingestion/src/block.rs:467-475`); OTel `process.owner`/`host.name` already land as
`otel.resource.*` properties (demote to display-only). No user/group value discriminator exists ⇒
adopt `user:`/`group:` prefixes.

## Appendix B — Drift audit (2026-07-30)

Re-verification of Appendix A against HEAD `2e95770` (branch `privacy`). Confirmed unchanged unless
listed. Nothing from the design vocabulary (`ReadScope`, `MintPolicy`, `micromegas.audience`,
`bound_audience`, …) is implemented yet — zero hits in `rust/`.

- **Function registry grew.** `register_lakehouse_functions` is now `query.rs:96-180` and registers
  **nine** UDTFs, not eight: `view_instance` (:103), `list_partitions` (:112), `list_view_sets`
  (:116), `retire_partitions` (:120), `perfetto_trace_chunks` (:124), `materialize_partitions`
  (:132), **`regenerate_partitions` (:139 — new, mutating)**, `parse_block` (:146), `process_spans`
  (:155). It also registers three UDFs the original audit didn't cover: **`get_payload` (:165)** —
  an async raw-payload **read** by block id, needing the same audience guard as `parse_block` — and
  the destructive **`retire_partition_by_file` / `retire_partition_by_metadata` (:168, :170)**,
  which join the mutating set.
- **Identity holes.** The prepared-statement path (`flight_sql_service_impl.rs:842`,
  `do_action_create_prepared_statement`) builds its session context with **no identity resolution**
  and passes `query_range = None`. `validate_and_resolve_user_attribution_grpc`
  (`rust/auth/src/user_attribution.rs:108`) **falls back to client-claimed identity** when
  `x-auth-subject` is absent (`:125-133`). Identity crosses the tower `AuthService` boundary as
  stringified gRPC metadata (`x-auth-subject`, `x-auth-email`, `x-allow-delegation`) — no
  groups/audience carried today.
- **No maintenance chokepoint.** The daemon (`telemetry-maintenance-srv/src/main.rs:40` →
  `servers/maintenance.rs:296`) never calls `make_session_context`; internal contexts are built
  per-view at ~10 sites hardcoding `NoOpSessionConfigurator` (`view.rs:109` — with an empty
  `ViewFactory` at :107, `merge.rs:101`, `sql_batch_view.rs:87,154`, `export_log_view.rs:118,171`,
  `batch_partition_merger.rs:133`, `metadata.rs:182,287`). Three context-building sites are
  reachable from user queries and must inherit the caller's scope:
  `parse_block_table_function.rs:81`, `process_spans_table_function.rs:254`,
  `perfetto_trace_execution_plan.rs:232`.
- **`TableScanRewrite` registration is conditional** on `query_range.is_some()` (`query.rs:206`);
  `OwnershipRewrite` must be unconditional.
- **Ingestion auth gaps.** No ingestion handler extracts `Extension<AuthContext>` (available since
  `rust/auth/src/axum.rs:75` inserts it); `otlp.rs` has no auth wiring at all; Firehose routes are
  merged outside the protected router (`ingestion.rs:151-156`). Stamping (Stage 5) must define
  their auth story.
- **Line-ref updates.** `validate_and_resolve_user_attribution_grpc` at
  `flight_sql_service_impl.rs:318` (was :317); `make_session_context` call sites at :372 and :842
  (were :371/:841); `make_session_context` itself at `query.rs:194`; `retire_partitions` at
  `query.rs:120`, `materialize_partitions` at `query.rs:132`.
- **Config factory precedent** for the grant knobs: `static_tables_configurator.rs:44-54`
  (`from_env` returning a no-op when unset).
