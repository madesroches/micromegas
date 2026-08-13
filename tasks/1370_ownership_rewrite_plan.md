# OwnershipRewrite — Query Enforcement Prong A Plan (#1370 — AbAC Stage 2)

## Overview

Stage 2 of the AbAC rollout (`tasks/data_isolation/audience_based_access_control_plan.md`, epic
#1334). It adds `OwnershipRewrite`, a mandatory `AnalyzerRule` that injects an audience predicate
into every `MaterializedView`-backed logical plan, using the `ReadScope` Stage 1 (#1369, landed as
`d0364c950`) already threads into `make_session_context` but does not yet consume. This is Prong A
of the two-pronged enforcement design (§4 of the AbAC plan) — Prong B (UDTF/UDF guards for the
span/metadata functions Prong A structurally cannot reach) is Stage 3 (#1371), a separate issue.

**Inactive until configured**, same as every stage before GA: with `MICROMEGAS_IMPLICIT_GROUPS` and
`MICROMEGAS_UNSTAMPED_AUDIENCE` both unset (today's default), every caller's resolved `ReadScope` is
either `All` (no provider configured) or the singleton `Audiences(["user:<email>"])`, and every
process is unstamped, so `OwnershipRewrite`'s predicate evaluates to "show nothing" for a caller who
sets no config, and to "show everything" only under `ReadScope::All`. **This means the rule changes
behavior the moment it is registered, for any deployment that already relies on
`ReadScope::Audiences` sessions seeing legacy, never-stamped data** — that combination did not exist
before Stage 2 (Stage 1 threads the scope but nothing consumes it), so there is no live deployment to
regress today, but it is the first stage where "register the rule" and "no behavior change" are not
automatically the same statement. Documented under Trade-offs and re-flagged for Stage 7's activation
docs.

## Current State

Verified against `d0364c950` (`main`, tip after Stage 1 merged). Stage 1 landed:

- `rust/auth/src/policy.rs` — `ReadPolicy`, `MintPolicy`, `ReadableAudiences`,
  `AudienceReadPolicy`/`AudienceMintPolicy`.
- `rust/analytics/src/lakehouse/read_scope.rs` — `ReadScope { All, Audiences(Arc<[String]>) }` and
  `CallerContext { read_scope, is_admin }` with `::internal()` / `::maintenance()` constructors.
- `rust/analytics/src/lakehouse/query.rs::make_session_context` takes `caller: CallerContext` and
  threads it into `register_functions`/`register_lakehouse_functions` (gating the five mutating
  UDTFs/UDFs on `is_admin`). **`read_scope` is accepted and dropped** — this module's own doc
  comment says so explicitly (`read_scope.rs:10-13`): "Today `ReadScope` is threaded down to
  `make_session_context` and dropped — Stage 2/3 must additionally arrange for it to reach the
  planner."
- `rust/public/src/servers/flight_sql_service_impl.rs::caller_context()` resolves a real
  `CallerContext` per request (both `do_get`, `:716`, and the prepared-statement path, `:1215`),
  deriving `read_scope` from `self.read_policy.resolve(auth_ctx)` when an `AuthContext` extension is
  present, `ReadScope::All` when absent (no provider configured), and denying (`Status::unavailable`
  / `Status::permission_denied`) on `Err` — never a scope. `FlightSqlServiceImpl` carries
  `read_policy: Arc<dyn ReadPolicy>` as a per-service field, set once at construction
  (`flight_sql_server.rs:279-287`, monolith `main.rs:251,304`).
- The three `TODO(#1371)` call sites (`metadata.rs:182,286`, `perfetto_trace_execution_plan.rs:254`,
  `parse_block_table_function.rs:83`, `process_spans_table_function.rs:254`) still pass
  `CallerContext::internal()` (`ReadScope::All`) even though they are reachable from a user query.
  **This is a known, already-tracked gap explicitly deferred to Stage 3/#1371, not something Stage 2
  introduces or is expected to fix.** Since `OwnershipRewrite` no-ops entirely under `ReadScope::All`
  (§4 below), these three sites stay unfiltered by Prong A until #1371 lands — exactly the exposure
  the TODO already documents, now via a second mechanism (Prong A) instead of none.
- No `micromegas.audience` property exists anywhere yet — ingestion stamping is Stage 5 (#1373).
  Stage 2's own tests must stamp it manually (issue text, step 7 below).

### `TableScanRewrite` is the only precedent, and it is deliberately narrower

`rust/analytics/src/lakehouse/table_scan_rewrite.rs` is the existing `AnalyzerRule`: it walks the
plan with `transform_up_with_subqueries`, matches `LogicalPlan::TableScan` whose source downcasts to
`MaterializedView` (`table_scan_rewrite.rs:37-43`), and wraps it in a `Filter` built from
`view.make_time_filter(...)`. Three things `OwnershipRewrite` must do differently:

1. **Registered unconditionally.** `TableScanRewrite` is added only when `query_range.is_some()`
   (`query.rs:229`, `if let Some(range) = &query_range`) — a query with no time range gets no
   time filter, which is correct (nothing to filter on). `OwnershipRewrite` has no such escape: a
   caller with a restricted `ReadScope` must never see unfiltered rows just because their query
   carries no explicit time range. The issue text is explicit about this asymmetry, and Stage 1's
   plan already flagged it as "the same shape of bug" (`1369_policy_seam_plan.md` §6, prepared
   statements).
2. **Not every `MaterializedView` gets the same predicate shape.** `TableScanRewrite`'s time filter
   is uniform across every view (`make_time_filter` is a `View` trait method every view implements
   the same way). The audience predicate is not: the `processes` view carries the audience as a
   property directly; other views need a semi-join or a literal-key check (see Design below) because
   they don't all carry a `process_id` **column** even when they are process-scoped.
3. **`ReadScope::All` must be a true no-op**, not "filter to a set containing everything" — `All` is
   the internal/maintenance marker (§5 of the AbAC plan) and never comes from a `ReadPolicy`, so
   `OwnershipRewrite::analyze` short-circuits to `Ok(plan)` unchanged when constructed with `All`,
   symmetric with how `register_lakehouse_functions` already treats `is_admin` as a gate rather than
   a filter.

### The view landscape is not uniformly "process_id-keyed" — verified per view set

The AbAC plan's §4 groups "streams, blocks, log_entries, measures, span views" together as
"process_id-keyed views (semi-join)". Reading every view's `get_file_schema()` shows this grouping is
imprecise in a way that matters for correctness — some of these views are process-**scoped** (every
row belongs to one process, addressed by `view_instance_id`) without carrying a `process_id`
**column** to semi-join on:

| View set | Schema has `process_id` column? | Reachable as | Scoping |
|---|---|---|---|
| `processes` | n/a — audience is a *property*, not a column | named global table, `view_instance('processes', id)` | the audience source itself |
| `streams` | yes (`streams_view.rs:28`) | named global table, `view_instance('streams', id)` | global (all streams) |
| `blocks` | yes (`blocks_view.rs:241`) | named global table, `view_instance('blocks', id)` | global (all blocks) |
| `log_entries` | yes (`log_entries_table.rs:27`) | named global table (`view_instance_id="global"`), `view_instance('log_entries', <process_id>)` | global **and** per-process |
| `measures` | yes (`metrics_table.rs:21`) | same as `log_entries` | global **and** per-process |
| `net_spans` | yes (`net_spans_table.rs:44`) | `view_instance('net_spans', <process_id>)` only — **rejects `"global"`** (`net_spans_view.rs:82-83`) | per-process only |
| `otel_spans` | yes (`otel/spans_table.rs:12`) | `view_instance('otel_spans', <process_id>)` only, no global instance (`view_factory.rs:337` comment) | per-process only |
| `async_events` | **no** (`async_events_table.rs` — "optimized for high-frequency data, excludes process info that can be joined", `:41-43`) | `view_instance('async_events', <process_id>)` only — **rejects `"global"`** (`async_events_view.rs:81-82`) | per-process only, but **no column to filter on** |
| `thread_spans` | **no** (`span_table.rs:50-80`, shared with `process_spans`) | `view_instance('thread_spans', <stream_id>)` only — `ThreadSpansView::new` rejects anything that doesn't parse as a UUID, and per the AbAC plan §4 this is "the one view set with no process_id-scoped alternative" | per-**stream** only, no global, no `process_id` **or** `stream_id` column |

This matters because a naive implementation that only knows "semi-join on `process_id` when the view
set name is in a hardcoded list" will compile, plan, and pass a smoke test, then silently return
**unfiltered** rows for `async_events` and `thread_spans` (no column exists to inject a semi-join
against, so a rule that only knows how to build column-based predicates has nothing to attach to and
must not fall through to "skip this view"). See Design §3–4 for the two extra branches this forces.

### `MaterializedView::schema()` is the branch signal, `get_view_set_name()`/`get_view_instance_id()` are the branch keys

`MaterializedView::schema()` (`materialized_view.rs:53-55`) returns `self.view.get_file_schema()` —
the real Arrow schema, checkable for a `process_id` field without hardcoding per-view-set knowledge
for *that* decision. `MaterializedView::get_view()` (`:46-48`) exposes the wrapped `Arc<dyn View>`,
whose `get_view_set_name()` / `get_view_instance_id()` (`view.rs:49,52`) give the branch keys the
AbAC plan's §4/§5b already call for ("Branch per view set via `MaterializedView::get_view_set_name()`").

## Design

### 1. Where `OwnershipRewrite` lives, and what it is constructed with

`rust/analytics/src/lakehouse/ownership_rewrite.rs`, registered in `lakehouse/mod.rs` next to
`table_scan_rewrite`. Like `TableScanRewrite`, it is a plain `AnalyzerRule` — no new crate
dependency, no I/O in `analyze()` (DataFusion's optimizer, not the analyzer, is what actually
executes the subqueries this rule injects — see §3).

```rust
#[derive(Debug)]
pub struct OwnershipRewrite {
    read_scope: ReadScope,
    unstamped_audience: Option<String>,
    public_view_sets: Vec<String>,
    processes_source: Arc<dyn TableSource>,
    streams_source: Arc<dyn TableSource>,
}
```

`processes_source` / `streams_source` are needed because building the semi-join/`EXISTS` subqueries
in §3–4 means constructing **fresh** `TableScan` nodes for `processes` (always) and `streams` (only
for the `thread_spans` two-hop case) from inside `analyze()`, which has no `SessionContext` to look
tables up in (`AnalyzerRule::analyze(&self, plan, options: &ConfigOptions)` — no session, no async).
The rule must therefore be handed the table sources it needs at **construction** time, not resolve
them lazily.

### 2. Construction site: after global-view registration, inside `make_session_context`

`processes` and `streams` are global views, registered as named tables by the `register_table` loop
already in `make_session_context` (`query.rs:243-253`) — **before** `configurator.configure(&ctx)`
and, today, before any analyzer rule beyond `TableScanRewrite`. `SessionContext::table_provider(name)`
(`datafusion::execution::context::SessionContext::table_provider`, async) returns the already-registered
`Arc<dyn TableProvider>` by name; wrap it in `datafusion::datasource::DefaultTableSource::new(..)` to
get a `TableSource`. So:

```rust
// query.rs, make_session_context, after the register_table loop (query.rs:253) and
// before configurator.configure (query.rs:255):
let processes_source: Arc<dyn TableSource> =
    Arc::new(DefaultTableSource::new(ctx.table_provider("processes").await?));
let streams_source: Arc<dyn TableSource> =
    Arc::new(DefaultTableSource::new(ctx.table_provider("streams").await?));
ctx.add_analyzer_rule(Arc::new(OwnershipRewrite::new(
    caller.read_scope.clone(),
    ownership_config.unstamped_audience.clone(),
    ownership_config.public_view_sets.clone(),
    processes_source,
    streams_source,
)));
```

registered **unconditionally** (no `query_range.is_some()` guard, matching the issue text and the
Current State §1 asymmetry above) — moved out from under the existing `if let Some(range) = &query_range`
block that currently only registers `TableScanRewrite` (`query.rs:228-230`).

This lookup can only fail if `"processes"`/`"streams"` are somehow not registered as global views —
not reachable through `default_view_factory`, but theoretically reachable through a caller-supplied
`ViewFactory` (`FlightSqlServerBuilder::with_view_factory_fn`) that omits them. Surface that as a
`Context`-wrapped error ("OwnershipRewrite requires `processes`/`streams` to be registered global
views") rather than a panic — `make_session_context` already returns `Result`.

### 3. `processes`'s own scan — direct filter, no subquery

The `processes` view carries the audience as a property, so its own `TableScan` gets the direct
`Filter` `TableScanRewrite` uses as a template, built instead from `property_get` + `coalesce` +
`IN`:

```rust
let audience_col = property_get_udf.call(vec![col("properties"), lit("micromegas.audience")]);
let effective = match &self.unstamped_audience {
    Some(u) => coalesce_udf.call(vec![audience_col, lit(u.clone())]),
    None => audience_col,
};
let predicate = effective.in_list(
    audiences.iter().map(|a| lit(a.clone())).collect(),
    false, // not negated
);
```

`property_get_udf` is `Arc::new(ScalarUDF::from(PropertyGet::new())).call(args)` — `ScalarUDF::call`
(`datafusion-expr::udf::ScalarUDF::call(&self, args: Vec<Expr>) -> Expr`) builds the `Expr` directly;
no session lookup needed since `OwnershipRewrite` can construct its own `PropertyGet` instance the
same way `register_extension_udfs` does (`datafusion-extensions/src/lib.rs:78`) rather than fetching
the one already registered on `ctx`. `coalesce_udf` is `datafusion::functions::expr_fn::coalesce`
(the built-in `coalesce` scalar function, `datafusion-functions::core::coalesce`). `property_get`
returns `Dictionary(Int32, Utf8)` (`property_get.rs:87-92`); `coalesce`'s branches must be
dictionary-vs-`Utf8`-comparable — DataFusion's implicit coercion handles this the same way the
existing equality usage in `query_processes.rs:73` already relies on it (per the AbAC plan §3).

`Some(mat_view)` where `mat_view.get_view().get_view_set_name().as_str() == "processes"` is the
match arm that produces this branch, whether reached via the named table or via
`view_instance('processes', id)`.

### 4. Process_id-**column** views — semi-join, one shared helper

For every other view whose `mat_view.schema()` contains a field named `process_id`
(`streams`, `blocks`, `log_entries`, `measures`, `net_spans`, `otel_spans` — see the table in Current
State), inject:

```
process_id IN (SELECT process_id FROM processes WHERE <processes predicate from §3>)
```

built with `LogicalPlanBuilder`:

```rust
let subquery = LogicalPlanBuilder::scan("processes", self.processes_source.clone(), None)?
    .filter(processes_predicate)?
    .project(vec![col("process_id")])?
    .build()?;
let predicate = in_subquery(col("process_id"), Arc::new(subquery));
```

`in_subquery` (`datafusion_expr::expr_fn::in_subquery(expr: Expr, subquery: Arc<LogicalPlan>) ->
Expr`) produces an **uncorrelated** `IN` subquery (it references no column from the outer plan) —
DataFusion's `DecorrelatePredicateSubquery` optimizer rule turns this into a `LeftSemi` join during
optimization, after the analyzer phase this rule runs in. This is why no I/O happens inside
`analyze()`: the rule only builds a syntactically valid logical plan; the actual `processes` scan and
the join execute later, during normal query execution, exactly like the existing time-range filter's
subplan does.

This same construction works whether the target is the **named** table (`streams`, `blocks`) or a
`view_instance(...)` call: the `MaterializedView`'s own schema decides the branch, not how it was
reached.

### 5. Process-scoped, no `process_id` column — literal check via `view_instance_id`

`async_events` is process-scoped (every instance is one process's events, and `"global"` is
explicitly rejected by the view constructor) but its physical schema has no `process_id` column to
project or join on. The **view_instance_id** for these instances *is* the process_id string
(`AsyncEventsView::new`, `async_events_view.rs:80-89`, parses it as a `Uuid`), and it is a **literal**
known at `analyze()` time via `mat_view.get_view().get_view_instance_id()` — the same accessor
`TableScanRewrite` doesn't need but `OwnershipRewrite` does for this branch. Inject a
whole-scan-gating `Filter` built from an `EXISTS` subquery instead of a semi-join, since there is no
row-level column to compare against:

```rust
let subquery = LogicalPlanBuilder::scan("processes", self.processes_source.clone(), None)?
    .filter(col("process_id").eq(lit(view_instance_id)).and(processes_predicate))?
    .build()?;
let predicate = exists(Arc::new(subquery)); // datafusion_expr::expr_fn::exists
```

wrapped around the whole `TableScan` exactly like `TableScanRewrite`'s time filter (`Filter::try_new(pred,
Arc::new(plan.clone()))`) — every row of the scan is either entirely visible or entirely hidden,
which is correct since every row of this instance belongs to the same one process.

### 6. Stream-scoped, no key column at all — two-hop literal check

`thread_spans` is scoped by `stream_id`, not `process_id`, and its schema (shared with
`process_spans`'s output, `span_table.rs`) has neither column. The `view_instance_id` **is** the
stream_id literal (`ThreadSpansView::new`, `thread_spans_view.rs:91`). Resolve it through `streams`
(which has both `stream_id` and `process_id` — `streams_view.rs:28`) into `processes`:

```rust
let subquery = LogicalPlanBuilder::scan("streams", self.streams_source.clone(), None)?
    .filter(col("stream_id").eq(lit(view_instance_id)))?
    .join(
        LogicalPlanBuilder::scan("processes", self.processes_source.clone(), None)?
            .filter(processes_predicate)?
            .build()?,
        JoinType::Inner,
        (vec!["process_id"], vec!["process_id"]),
        None,
    )?
    .build()?;
let predicate = exists(Arc::new(subquery));
```

This is the one construction this issue's own scope statement doesn't spell out (it only says
"semi-join on `process_id`-keyed views"), and it is the one place the AbAC plan's §4 and §5's Prong B
section talk past each other slightly: §4 lists Prong A as covering "`view_instance('<set>', <id>)`
... caught as a `TableScan<MaterializedView>` ... exactly like a named view," which — taken literally
— includes `thread_spans`; but the concrete `process_id`/`stream_id` **cache** machinery §4 describes
is scoped explicitly to Prong B ("Prong B performance", Stage 3). Resolving it at Stage 2 via a
literal-valued `EXISTS` subquery (§5, §6 above) rather than a runtime cache costs nothing extra here
— the `stream_id` is a plan-time literal either way — and means Stage 2 does not silently leave
`thread_spans` and `async_events` **more** exposed than every other view set while Stage 3's caches
are still pending. Flagged under Open Questions for reviewer sign-off, since the AbAC plan itself
does not say this in as many words.

### 7. Public view sets — skip the branch entirely

Before any of §3–6 run, check `self.public_view_sets.contains(mat_view.get_view().get_view_set_name().as_str())`;
if true, `Transformed::no(plan)` — no predicate at all, for any view kind. This is the one part of §4
the issue text names directly ("Branch per view set via `MaterializedView::get_view_set_name()`").
Default empty (§8), so inert unless configured — matches the AbAC plan §5b's "off by default,
fail-closed" framing. No enforcement of the AbAC plan's operator-responsibility constraint ("only
genuinely aggregated / non-PII view sets") beyond documentation — same posture the plan itself takes.

### 8. Config: `OwnershipRewriteConfig`, bundled into `CallerContext` rather than a new parameter

Two new knobs (`MICROMEGAS_UNSTAMPED_AUDIENCE`, `MICROMEGAS_PUBLIC_VIEW_SETS`) are **per-service**
config, resolved once at server startup — the same lifetime as `session_configurator` and
`read_policy`, not the per-request `read_scope`. The two ways to thread that into
`make_session_context`:

**(a) A new positional parameter** (`ownership_config: Arc<OwnershipRewriteConfig>`), mirroring how
Stage 1 added `caller: CallerContext` as `make_session_context`'s 6th parameter. Costs touching every
one of the ~13 call sites Stage 1's plan inventoried (`1369_policy_seam_plan.md`, Current State
table) a second time — all but two of them (the `do_get`/prepared-statement paths) are
internal/maintenance sites that would pass `Arc::new(OwnershipRewriteConfig::default())` verbatim,
since the config is inert wherever `read_scope` is already `All`.

**(b) A new field on `CallerContext`** (`ownership_config: Arc<OwnershipRewriteConfig>`), populated by
`CallerContext::internal()`/`::maintenance()` with `Arc::new(OwnershipRewriteConfig::default())`
internally and by `FlightSqlServiceImpl::caller_context()` from a new per-service field
(`self.ownership_config.clone()`), exactly the way `read_policy` already flows into `read_scope`
today. **Recommended** — it touches only the 3 files that construct `CallerContext` by struct literal
today (`read_scope.rs`'s definition and its two constructors; `flight_sql_service_impl.rs`'s
`caller_context()` resolver; and `analytics/tests/lakehouse_admin_gate_test.rs`, the one test that
builds `CallerContext { .. }` directly rather than via `::internal()`/`::maintenance()` — verified by
`grep -rln "CallerContext {"`, three hits total including the definition). Every other
`make_session_context` call site is untouched because it already goes through
`::internal()`/`::maintenance()`.

The trade-off against (b): `OwnershipRewriteConfig` is not really a property of *the caller* the way
`read_scope`/`is_admin` are — it is deployment config that happens to ride along. Accepted for the
same reason Stage 1 accepted bundling `is_admin` and `read_scope` into one struct in the first place
(`1369_policy_seam_plan.md` §3): the two are visited together at every real call site anyway, and a
struct with a slightly-impure field beats re-touching a parameter list that Stage 1 already grew
once. Flagged under Open Questions for reviewer sign-off since it is a judgment call the GH issue
text doesn't settle either way.

```rust
// rust/analytics/src/lakehouse/read_scope.rs
#[derive(Debug, Clone, Default)]
pub struct OwnershipRewriteConfig {
    pub unstamped_audience: Option<String>,
    pub public_view_sets: Vec<String>,
}

pub struct CallerContext {
    pub read_scope: ReadScope,
    pub is_admin: bool,
    pub ownership_config: Arc<OwnershipRewriteConfig>, // new
}
```

**Parsing.** Neither knob can reuse `rust/auth`'s `implicit_groups_var`/`parse_implicit_groups`
directly — `micromegas-analytics` does not depend on `micromegas-auth` (Stage 1's own §1, preserved
here: these are query-planner inputs, not authorization data, but the crate boundary argument is the
same one Stage 1 made). Duplicate the small pieces locally in `ownership_rewrite.rs`:

- `{prefix}_PUBLIC_VIEW_SETS` with fallback to `MICROMEGAS_PUBLIC_VIEW_SETS`, same
  comma-separated / reject-`[`-`]`-`"` encoding as `MICROMEGAS_IMPLICIT_GROUPS` (`policy.rs`'s
  `parse_implicit_groups`) — copy the validation rule, not the auth-crate function itself.
- `{prefix}_UNSTAMPED_AUDIENCE` with fallback to `MICROMEGAS_UNSTAMPED_AUDIENCE` — a single
  optional string, **validated well-formed** (`user:`/`group:`-prefixed) at parse time. An
  unprefixed value would silently never match any `ps` entry (every `ReadScope::Audiences` element
  is prefixed) — a configured-but-inert knob is exactly the failure mode `parse_implicit_groups`'s
  own validation exists to catch elsewhere, so mirror it here rather than accepting any string.

```rust
impl OwnershipRewriteConfig {
    pub fn from_env(prefix: &str) -> Result<Self> { ... }
}
```

Wired at the same two sites Stage 1 wired `AudienceReadPolicy::from_env`:
`FlightSqlServerBuilder::with_ownership_config()` (new builder method, mirroring
`with_read_policy`), defaulted inside `build()`'s `use_default_auth` branch via
`OwnershipRewriteConfig::from_env("")` and to `OwnershipRewriteConfig::default()` on the other two
branches when `self.ownership_config` is `None` (`flight_sql_server.rs:245-279`, same shape as the
existing `read_policy` resolution); the monolith calls
`OwnershipRewriteConfig::from_env("MICROMEGAS_ANALYTICS")` externally
(`main.rs:251`-style) and passes it via `.with_ownership_config(cfg)`.

## Implementation Steps

### Phase 1 — config and the analytics-side struct

1. `rust/analytics/src/lakehouse/read_scope.rs` — add `OwnershipRewriteConfig` (Design §8) and the
   `ownership_config` field on `CallerContext`; update `::internal()`/`::maintenance()`.
2. `rust/analytics/tests/lakehouse_admin_gate_test.rs:38-41` — the one non-constructor
   `CallerContext { .. }` literal gains `ownership_config: Arc::new(OwnershipRewriteConfig::default())`.

### Phase 2 — `OwnershipRewrite` itself

3. `rust/analytics/src/lakehouse/ownership_rewrite.rs` (new) — the struct (Design §1), the
   `processes`-direct-filter branch (§3), the process_id-column semi-join branch (§4), the
   process-scoped-no-column literal branch (§5, `async_events`), the stream-scoped two-hop literal
   branch (§6, `thread_spans`), the public-view-set skip (§7), and the `ReadScope::All` no-op. Doc
   comments carry the per-view-set schema table from Current State — the next added view set is the
   next reader who needs it.
4. `rust/analytics/src/lakehouse/mod.rs` — `pub mod ownership_rewrite;`.
5. `rust/analytics/src/lakehouse/query.rs::make_session_context` — resolve `processes_source` /
   `streams_source` after the global-view registration loop (Design §2), construct
   `OwnershipRewrite` from `caller.read_scope` + `caller.ownership_config`, and
   `ctx.add_analyzer_rule(...)` **unconditionally** (move it out from under the `query_range.is_some()`
   block that currently only guards `TableScanRewrite`, `query.rs:228-230`).

### Phase 3 — wiring config through the two real servers

6. `rust/public/src/servers/flight_sql_server.rs` — `ownership_config: Option<Arc<OwnershipRewriteConfig>>`
   field + `with_ownership_config()` builder method (mirrors `read_policy`, `:70,138`); resolve the
   default inside `build()` alongside the existing `read_policy` resolution (`:245-279`):
   `OwnershipRewriteConfig::from_env("")` on the `use_default_auth` branch, `::default()` on the
   other two.
7. `rust/public/src/servers/flight_sql_service_impl.rs` — new `ownership_config: Arc<OwnershipRewriteConfig>`
   field on `FlightSqlServiceImpl` + constructor parameter (mirrors `read_policy`, `:490,499`);
   `caller_context()` (`:533-555`) sets `CallerContext.ownership_config` from it on both branches
   (present and absent `AuthContext` extension) — this knob is not permission-sensitive the way
   `read_scope` is, so it does not participate in the absent-extension/`Err` distinction §2 of Stage
   1's plan cared about; it is copied verbatim regardless.
8. `rust/monolith/src/main.rs` — resolve `OwnershipRewriteConfig::from_env("MICROMEGAS_ANALYTICS")?`
   alongside the existing `AudienceReadPolicy::from_env("MICROMEGAS_ANALYTICS")?` call (`:251`) and
   pass it via `.with_ownership_config(cfg)` next to `.with_read_policy(policy)` (`:304`).
9. Update `flight_sql_service_impl.rs`'s and `flight_sql_server.rs`'s existing test helpers/fixtures
   that construct `FlightSqlServiceImpl`/the builder directly (search `FlightSqlServiceImpl::new(` —
   Stage 1 did not need to touch these since `read_policy` was its own addition at the same
   call sites, so this inventory needs to be redone against current `main`, not copied from Stage 1's
   plan).

### Phase 4 — tests (issue's own acceptance criteria, step 7)

10. New DB-backed test file, `rust/analytics/tests/ownership_rewrite_db_test.rs` (mirrors
    `net_spans_retire_overlap_db_test.rs`'s "requires a live `MICROMEGAS_SQL_CONNECTION_STRING`" /
    `MICROMEGAS_OBJECT_STORE_URI` convention): seed two processes via the real ingestion pipeline
    (or direct `processes`/`blocks` SQL inserts, matching how `sql_telemetry_db.rs`'s tables are
    shaped), manually set `micromegas.audience` in each process's `properties` (**before** the
    processes-view batch materialization runs, since ingestion stamping doesn't exist until Stage 5)
    to two different values; assert, through `make_session_context` with different `CallerContext`s:
    - a `ReadScope::Audiences(["user:a"])` session sees only process A's rows (from `processes`
      directly and via a `process_id`-keyed view, e.g. `log_entries`);
    - a `ReadScope::Audiences(["user:b"])` session sees only process B's rows;
    - a session whose scope contains neither audience sees nothing;
    - `ReadScope::All` (`CallerContext::maintenance()`) sees both;
    - with `MICROMEGAS_UNSTAMPED_AUDIENCE` configured, an unstamped third process is visible only to
      a caller whose scope includes that configured audience;
    - `async_events` and `thread_spans` (§5/§6's literal-check branches) enforce the same
      cross-audience denial as the column-based views — this is the coverage that would have missed
      a naive "process_id column or bust" implementation (Current State's schema table).
11. Unit-level planning tests (offline, no DB — mirroring `lakehouse_admin_gate_test.rs`'s pattern)
    for the public-view-set skip: a view set on `MICROMEGAS_PUBLIC_VIEW_SETS` plans with no injected
    filter (assert on the `EXPLAIN` output or the physical row count against a fixture with rows from
    two audiences) regardless of `ReadScope`.

## Files to Modify

- `rust/analytics/src/lakehouse/ownership_rewrite.rs` — **new**; the rule
- `rust/analytics/src/lakehouse/read_scope.rs` — `OwnershipRewriteConfig`, `CallerContext` field
- `rust/analytics/src/lakehouse/mod.rs` — register the module
- `rust/analytics/src/lakehouse/query.rs` — construct + register `OwnershipRewrite` unconditionally
- `rust/public/src/servers/flight_sql_server.rs` — `with_ownership_config()`, default resolution
- `rust/public/src/servers/flight_sql_service_impl.rs` — new field, constructor param,
  `caller_context()`
- `rust/monolith/src/main.rs` — resolve + wire `OwnershipRewriteConfig::from_env("MICROMEGAS_ANALYTICS")`
- `rust/analytics/tests/lakehouse_admin_gate_test.rs` — one `CallerContext` literal
- `rust/analytics/tests/ownership_rewrite_db_test.rs` — **new**
- `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs` — **new** (offline, planning-only)

## Trade-offs

- **`CallerContext` field vs. a new `make_session_context` parameter** for `OwnershipRewriteConfig`
  — see Design §8. Chosen: the `CallerContext` field, for a smaller diff footprint; flagged as an
  Open Question since it is a judgment call, not a settled decision.
- **Literal-valued `EXISTS` subqueries for `async_events`/`thread_spans` (§5/§6) vs. deferring those
  two view sets to Stage 3.** The issue text ("semi-join on `process_id`-keyed views") could be read
  as scoping Prong A to the views that actually have the column, leaving `async_events`/`thread_spans`
  unfiltered until Stage 3's caches land. Chosen: cover them now, because leaving two named,
  queryable view sets completely unfiltered — reachable by any caller who knows a `view_instance(...)`
  call, with no gate at all — is a bigger, more surprising hole than the `TODO(#1371)` sites (which
  are at least documented and bounded to three specific internal recursive contexts) and costs no
  runtime machinery to close (the key is a plan-time literal either way). Flagged as an Open Question
  for reviewer sign-off since the AbAC plan text does not resolve this explicitly.
- **No enforcement of the public-view-set "aggregated / non-PII only" constraint in code.** Matches
  the AbAC plan §5b's own posture — it is an operator-responsibility allowlist, not a
  code-checkable property (the plan gives an example of a view set that must never be listed — a raw
  global `log_entries` instance — precisely because nothing about its schema marks it as unsafe to
  expose).
- **Duplicating the comma-separated-list parser instead of depending on `micromegas-auth`.** Same
  reasoning Stage 1 gave for keeping `ReadScope` out of `micromegas-auth`'s crate boundary (§1 of
  `1369_policy_seam_plan.md`): a few duplicated lines is cheaper than a new dependency edge for a
  crate published without one today.

## Documentation

Stage 2 ships no *operator-visible* behavior change under default (unset) config, matching every
prior stage's posture, but it is the first stage where "unset" and "no behavior change" require the
caveat in Overview (a `ReadScope::Audiences` session with legacy unstamped data is a new combination).
No mkdocs page yet (Stage 7 owns the isolation page). What needs writing:

- Doc comments on `OwnershipRewrite` carrying: the per-view-set schema table (Current State), why
  `ReadScope::All` short-circuits, why `processes` gets a direct filter while everything else gets a
  subquery, and the async_events/thread_spans literal-check rationale (§5/§6) — this is exactly the
  kind of non-obvious "why" that will not survive the next contributor's skim of `view_factory.rs`
  without it written down here.
- `tasks/data_isolation/audience_based_access_control_plan.md` — record, once implemented: the exact
  `async_events`/`thread_spans` treatment (§5/§6), since the plan's own §4 doesn't fully resolve it;
  the `CallerContext`-vs-new-parameter decision (§8); and that `OwnershipRewriteConfig`'s two knobs
  are parsed in `micromegas-analytics`, not `micromegas-auth` (mirrors Stage 1's own "parse where
  consumed" note about `MICROMEGAS_UNSTAMPED_AUDIENCE`/`MICROMEGAS_PUBLIC_VIEW_SETS`,
  `1369_policy_seam_plan.md` §5).
- `CHANGELOG.md` per the `pr` skill's convention.

## Testing Strategy

- DB-backed cross-audience tests (Implementation Steps, step 10) are the issue's own acceptance
  criteria verbatim: cross-audience queries return nothing, same-audience returns its own rows,
  unstamped rows follow the `MICROMEGAS_UNSTAMPED_AUDIENCE` knob, `ReadScope::All` sees everything —
  extended to explicitly cover the two schema-less view sets (`async_events`, `thread_spans`) the
  naive reading of the issue text would miss.
- Offline planning tests (step 11) for the public-view-set allowlist, following
  `lakehouse_admin_gate_test.rs`'s no-live-DB pattern.
- `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 build/rust_ci.py`.
- Manual smoke: run the monolith with `MICROMEGAS_IMPLICIT_GROUPS`/`MICROMEGAS_UNSTAMPED_AUDIENCE`
  left unset (today's default) and confirm existing queries against real ingested data are
  byte-for-byte unaffected — the one behavior-preserving path that matters before this merges, given
  Overview's caveat that "unset" and "no behavior change" are not *quite* the same statement anymore.

## Open Questions

1. **`OwnershipRewriteConfig` on `CallerContext` vs. a new `make_session_context` parameter**
   (Design §8) — recommended: the `CallerContext` field, for footprint; needs reviewer sign-off since
   the GH issue text doesn't settle it.
2. **Covering `async_events`/`thread_spans` in Stage 2 via literal `EXISTS` subqueries (§5/§6) vs.
   deferring them to Stage 3** alongside the process_id/stream_id caches the AbAC plan's "Prong B
   performance" section describes. Recommended: cover them now (Trade-offs) — needs reviewer
   sign-off since the AbAC plan text can be read either way.
3. **Should `view_instance('processes', <id>)` even be reachable/tested?** No code path in the
   current design forbids it (`ViewInstanceTableFunction::call_with_args` calls
   `view_factory.make_view("processes", id)`, which — since `processes` is a `SqlBatchView`, not
   built through a `ViewMaker`/`add_view_set` entry — will fail at `make_view` with "view set
   'processes' not found" today, since `processes` is only ever registered via `add_global_view`, not
   `add_view_set`). If so, `OwnershipRewrite`'s `get_view_set_name() == "processes"` branch (Design
   §3) is reachable only through the named table, and the plan's mention of it applying "whether
   reached via the named table or via `view_instance`" is moot — worth confirming during
   implementation rather than carrying dead code.
