# OwnershipRewrite — Query Enforcement Prong A Plan (#1370 — AbAC Stage 2)

## Overview

Stage 2 of the AbAC rollout (`tasks/data_isolation/audience_based_access_control_plan.md`, epic
#1334). It adds `OwnershipRewrite`, a mandatory `AnalyzerRule` that injects an audience predicate
into every `MaterializedView`-backed logical plan, using the `ReadScope` Stage 1 (#1369, landed as
`d0364c950`) already threads into `make_session_context` but does not yet consume. This is Prong A
of the two-pronged enforcement design (§4 of the AbAC plan) — Prong B (UDTF/UDF guards for the
span/metadata functions Prong A structurally cannot reach) is Stage 3 (#1371), a separate issue.

**Inactive only when no auth provider is configured — not universally inactive.** `ReadScope::All`
comes from the *absence* of an `AuthContext` extension on the request — `caller_context()` resolves
`ReadScope::All` whenever no provider is configured, regardless of what
`MICROMEGAS_IMPLICIT_GROUPS`/`MICROMEGAS_UNSTAMPED_AUDIENCE` are set to
(`flight_sql_service_impl.rs:539-552`). In that case `OwnershipRewrite` no-ops and behavior is
unchanged. **But any deployment that already runs with auth enabled — the normal production
posture, not a hypothetical — is a live deployment this stage regresses.** Stage 1 already resolves
`ReadScope::Audiences([...])` for every request carrying an `AuthContext`
(`flight_sql_service_impl.rs:539-552`), and the monolith installs a read policy whenever
`roles.flightsql && !args.disable_auth` (`monolith/src/main.rs:251`), defaulting to
`AudienceReadPolicy::from_env("")` (`flight_sql_server.rs:271`). Since no data carries
`micromegas.audience` until Stage 5 (#1373), such a deployment goes from full visibility today
(nothing yet filters `ReadScope::Audiences` sessions) to **zero visible rows** the instant
`OwnershipRewrite` is registered — unless the escape hatch below is configured as a **pair** before
the upgrade. Setting `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` alone does not restore
visibility: `AudienceReadPolicy::resolve` (`auth/src/policy.rs:203-211`) builds each caller's
resolved audience set from `identity_and_group_audiences` (`:88-103` — `user:<email>` ∪ groups claim
∪ implicit groups) plus `caller.read_audiences`, so with `MICROMEGAS_IMPLICIT_GROUPS` left unset a
human caller's set is just `{user:<email>}`; the coalesced `group:everyone` default then fails the
`IN` check and the deployment still goes to zero rows. The required pair is
**`MICROMEGAS_IMPLICIT_GROUPS=everyone` and `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone`**, and
(per the "existing knob" correction below) both must be set in the same deploy that introduces
`OwnershipRewrite`, not on the old binary beforehand. This is a breaking change for every
auth-enabled deployment; treated accordingly under Trade-offs, Testing Strategy (an auth-enabled
smoke/upgrade check exercising the pair, not just the auth-unset path), and Documentation (a
CHANGELOG upgrade note stating the pair), and re-flagged for Stage 7's activation docs.

**`MICROMEGAS_UNSTAMPED_AUDIENCE` is a new knob shipped by this stage, not a pre-existing one.**
`grep -rn "UNSTAMPED_AUDIENCE" --include="*.rs"` across the repo returns zero hits today — nothing
reads it before Design §8's `OwnershipRewriteConfig::from_env` lands, so it cannot be "set on the old
binary before upgrading" the way a genuinely pre-existing knob could. It becomes live only in the
same deploy that registers `OwnershipRewrite`, so the escape-hatch pair above is config-then-rollout
ordering *within* this stage's own deploy, not a step performable against today's `main`.

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

`processes`, `streams` and `log_stats` are `SqlBatchView`s, which register **two** tables each
(`sql_batch_view.rs:328-351`): the raw `MaterializedView` under `__<name>__partitions`, and the
merged (`GROUP BY` + `first_value`-per-column) query result under the user-visible bare name via
`df.into_view()`. `ctx.table_provider("processes")` therefore returns the merged-query view, not a
plain scan — relevant to Design §2/§4 below. Both `__processes__partitions` and
`__streams__partitions` (and `__log_stats__partitions`) end up as ordinary user-queryable named
tables as a side effect of this registration shape, which is otherwise undocumented here.

| View set | Schema has `process_id` column? | Reachable as | Scoping |
|---|---|---|---|
| `processes` | n/a — audience is a *property*, not a column | named global table only — **not** reachable via `view_instance` (`processes` is registered via `add_global_view`, not `add_view_set`; see the corrected Open Questions/§3 note below) | the audience source itself |
| `streams` | yes (`streams_view.rs:28`) | named global table only — **not** reachable via `view_instance`, same reason as `processes` | global (all streams) |
| `blocks` | yes (`blocks_view.rs:241`) | named global table only — **not** reachable via `view_instance`, same reason as `processes` | global (all blocks) |
| `log_entries` | yes (`log_entries_table.rs:27`) | named global table (`view_instance_id="global"`), `view_instance('log_entries', <process_id>)` | global **and** per-process |
| `measures` | yes (`metrics_table.rs:21`) | same as `log_entries` | global **and** per-process |
| `net_spans` | yes (`net_spans_table.rs:44`) | `view_instance('net_spans', <process_id>)` only — **rejects `"global"`** (`net_spans_view.rs:82-83`) | per-process only |
| `otel_spans` | yes (`otel/spans_table.rs:12`) | `view_instance('otel_spans', <process_id>)` only, no global instance (`view_factory.rs:337` comment) | per-process only |
| `images` | yes (`images_table.rs:16-20`) | `view_instance('images', <process_id>)` only — **rejects `"global"`** (`images_view.rs:73-75`), registered via `add_view_set` (`view_factory.rs:290-303`) | per-process only |
| `log_stats` | yes (inherited from `log_entries`'s `process_id`, `log_stats_view.rs:35`) | named global table only, registered via `add_global_view` (`view_factory.rs:316`), not `add_view_set` | global (aggregated across processes) |
| `async_events` | **no** (`async_events_table.rs` — "optimized for high-frequency data, excludes process info that can be joined", `:41-43`) | `view_instance('async_events', <process_id>)` only — **rejects `"global"`** (`async_events_view.rs:81-82`) | per-process only, but **no column to filter on** |
| `thread_spans` | **no** (`span_table.rs:50-80`, shared with `process_spans`) | `view_instance('thread_spans', <stream_id>)` only — `ThreadSpansView::new` rejects anything that doesn't parse as a UUID, and per the AbAC plan §4 this is "the one view set with no process_id-scoped alternative" | per-**stream** only, no global, no `process_id` **or** `stream_id` column |

`log_stats` is a `SqlBatchView` like `processes`/`streams` (Current State's earlier paragraph already
covers this), so it registers the same two-table shape: `__log_stats__partitions` (raw
`MaterializedView`) plus the merged `log_stats` view — both are process_id-column views for §4's
purposes, since `process_id` survives the `SqlBatchView`'s `GROUP BY`.

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

### 2. Construction site: after global-view registration, inside `make_session_context`, gated on `read_scope`

Do **not** resolve the sources via `ctx.table_provider("processes"/"streams")`. Two independent
problems with that: (a) `SessionContext::table_provider` requires the name to already be registered,
which — per the corrected Current State landscape above — is not reachable through every in-tree
`ViewFactory` (`SqlBatchView::new` builds its own internal `make_session_context` call with
`ViewFactory::new(vec![blocks_view])`, `sql_batch_view.rs:107-116`; `View::merge_partitions`'s default
uses `ViewFactory::new(vec![])`, `view.rs:107`/`merge.rs:255`; and the same shape appears in
`analytics/tests/lakehouse_admin_gate_test.rs:42`, `sql_partition_spec_sort_order_tests.rs:138`, and
`log_stats_ordering_tests.rs:190`) — so an unconditional lookup would error on server startup itself;
and (b) even where `"processes"` **is** registered, it names the `SqlBatchView` **merged** query
result (`__processes__partitions` is the raw scan — see Current State), so every injected
subquery would re-run the `GROUP BY` / `first_value` merge, and the registered table's
`MaterializedView` is built with the session's own `query_range`, which must never bound the audience
lookup (§3's "time-unbounded" note below) — a caller's own rows must not disappear because their
`processes`-view partition falls outside their query's time window.

Both problems disappear by building the sources directly, since `make_session_context` already has
everything `register_table` uses, without going through the session at all:

```rust
// query.rs, make_session_context, after the register_table loop (query.rs:253) and
// before configurator.configure (query.rs:255):
if caller.read_scope != ReadScope::All {
    // ReadScope::All is the internal/maintenance marker (Current State §3) — OwnershipRewrite
    // would no-op for it anyway, so skip resolving sources and registering the rule entirely
    // rather than requiring every ReadScope::All caller's ViewFactory to carry processes/streams.
    let processes_view = view_factory
        .get_global_view("processes")
        .context("OwnershipRewrite requires the `processes` global view to be registered")?;
    let streams_view = view_factory
        .get_global_view("streams")
        .context("OwnershipRewrite requires the `streams` global view to be registered")?;
    // query_range: None, always — the audience lookup must be time-unbounded. This alone is not
    // enough: `OwnershipRewrite` must also be registered *after* `TableScanRewrite` (query.rs:229),
    // because `TableScanRewrite::analyze` walks the whole plan with `transform_up_with_subqueries`
    // and wraps *any* `TableScan<MaterializedView>` it finds — including the
    // `__processes__partitions`/`__streams__partitions` scans injected below — in a `Filter` built
    // from `view.make_time_filter(begin, end)`. Analyzer rules run strictly in registration order
    // (`Analyzer::execute_and_check`), so if `OwnershipRewrite` ran first, `TableScanRewrite` would
    // still find and time-bound its injected subqueries on its next pass. This is also, incidentally,
    // the raw per-partition scan (equivalent to `__processes__partitions`/`__streams__partitions`),
    // not the merged query.
    let processes_source: Arc<dyn TableSource> = Arc::new(DefaultTableSource::new(Arc::new(
        MaterializedView::new(
            lakehouse.clone(),
            reader_factory.clone(),
            processes_view,
            part_provider.clone(),
            None,
        ),
    )));
    let streams_source: Arc<dyn TableSource> = Arc::new(DefaultTableSource::new(Arc::new(
        MaterializedView::new(
            lakehouse.clone(),
            reader_factory.clone(),
            streams_view,
            part_provider.clone(),
            None,
        ),
    )));
    ctx.add_analyzer_rule(Arc::new(OwnershipRewrite::new(
        caller.read_scope.clone(),
        caller.ownership_config.unstamped_audience.clone(),
        caller.ownership_config.public_view_sets.clone(),
        processes_source,
        streams_source,
    )));
}
```

`MaterializedView::new` is synchronous and does no I/O (`materialized_view.rs:29-41`), so this needs
no `.await` and adds nothing to the "no I/O in `analyze()`" property §1 already relies on. This also
resolves the missing-table case cleanly: it is only attempted when `caller.read_scope != ReadScope::All`,
so `ReadScope::All` callers (server startup, `SqlBatchView`'s and `View::merge_partitions`'s internal
contexts, and the tests cited above) never hit the lookup at all — the claim that the missing-table
case is unreachable in-tree was wrong for the general case but is exactly right once resolution is
scoped to non-`All` scopes. A caller-supplied `ViewFactory`
(`FlightSqlServerBuilder::with_view_factory_fn`) that omits `processes`/`streams` **and** is used under
a restricted `ReadScope` still fails fast via the `Context`-wrapped error above, rather than a panic.
**This is a hard startup/request failure for any non-`All`-scope caller whose `ViewFactory` lacks
`processes`/`streams`, and it is not merely hypothetical:** `start_server` in
`rust/public/tests/read_policy_threading_tests.rs:79` builds `Arc::new(ViewFactory::new(vec![]))`,
and every test in that file configures an `ApiKeyAuthProvider`, so `caller_context()` resolves
`ReadScope::Audiences(..)`. Without a fix, `make_session_context` now fails before any SQL is
planned for that file's tests — see Implementation Steps step 9 for the required fix.

**Cost of `query_range: None`, and the choice of `processes`/`streams` as the audience source.**
Building `processes_source`/`streams_source` this way — a `MaterializedView` over
`LivePartitionProvider` with `query_range: None` — has two consequences worth stating as decisions
rather than leaving implicit:

1. *Unbounded, uncached, per-`TableScan` cost.* `MaterializedView::scan` passes `query_range` straight
   to `part_provider.fetch(...)` (`materialized_view.rs:61-84`), and `None` makes
   `LivePartitionProvider` issue the partition-metadata query with no time predicate at all
   (`partition_cache.rs:346-400`) — every `processes` (and, for `thread_spans`, every `streams`)
   partition ever written. The injected `property_get`-based predicate cannot prune this:
   `supports_filters_pushdown` reports `Inexact` for it. And because §3–6 inject an independent
   subquery at every `TableScan` site the traversal visits, and DataFusion does not
   common-subexpression-eliminate identical injected subqueries across a plan, a single query joining
   `log_entries` and `measures` scans `processes`'s entire history twice, not once.
   **Decided:** compute the `processes` audience filter (§3) and the resolved-per-process
   `per_process_audience`/`resolved_predicate` subplan the §4 semi-join is built from (see below) once
   per `analyze()` call, before `transform_up_with_subqueries` runs, and reuse the same
   `Expr`/`Arc<LogicalPlan>` at every site the traversal visits — this bounds the §4 branch (the
   majority of the schema table below) to one
   `processes` scan per query regardless of how many process_id-keyed tables it touches. §5/§6's
   `EXISTS` subqueries still build one subquery per scan site, since each embeds a different
   `view_instance_id` literal and cannot be shared. This mitigates the cost; it does not eliminate the
   unbounded, uncached scan itself.
2. *Materialization lag vs. Prong B.* `processes`/`streams` are `SqlBatchView`s materialized only by
   the maintenance daemon's `materialize_all_views` pass (`public/src/servers/maintenance.rs:106-210`;
   `SqlBatchView::jit_update` is a no-op, `sql_batch_view.rs:307-313`) — unlike the JIT per-process
   views, nothing materializes them on demand. A restricted caller's audience is therefore resolved
   against however stale the daemon's last pass left `processes`, and against nothing at all if the
   daemon is down or not deployed — including for the caller's own just-ingested data. This diverges
   from Prong B, which resolves the same `process_id → audience` mapping from Postgres directly via
   `find_process` plus an invalidation-free cache
   (`audience_based_access_control_plan.md` §4, "Prong B performance"), so during that lag window the
   two prongs can disagree about the same process's audience.

   Both costs share one root cause: Prong A reads the mapping through a `MaterializedView` scan
   instead of through the Postgres-backed, cached point lookup Prong B already builds. **Decided:**
   accept both for Stage 2 rather than build a shared cache now — consuming Prong B's cache from
   `OwnershipRewrite` would need `analyze()` (synchronous, no session, no I/O — §1) to read a cache
   whose population is driven from outside the query path, and that cache is Prong B's own Stage 3
   (#1371) deliverable, not something this issue's scope (a pure `AnalyzerRule`) should build a second
   time. Recorded here, not silently accepted: Stage 3 should evaluate having `OwnershipRewrite`
   consume Prong B's cache instead of scanning `processes`/`streams`, once that cache exists, so both
   prongs converge on one source of truth. Until then, Stage 2 ships with a documented lag window and
   per-query scan cost (mitigated per point 1 above) rather than a silently-assumed-consistent, free
   lookup.

**Any-row semantics of filtering raw partitions, and why it must be resolved per process, not per
row.** `__processes__partitions` is the pre-merge `SqlBatchView` output: its transform query only
`GROUP BY process_id` *within* each source partition (`max_partition_delta_from_source:
TimeDelta::days(1)`, `processes_view.rs:74-88`), so a long-lived process still accumulates multiple
`__processes__partitions` rows across partitions over time — collapsing those into one row per
process is exactly what the separate `merge_query` (the `processes` named table, deliberately bypassed
by §2's `query_range: None` decision above) exists to do. Filtering the raw partitions directly, the
way §4's `process_id IN (SELECT process_id FROM __processes__partitions WHERE <pred>)` and §5/§6's
`EXISTS` subqueries do below, therefore means "does *any* row for this `process_id` pass the
predicate," not "does this process's (current) audience pass the predicate" — a fail-open divergence
from the AbAC parent plan's post-merge `SELECT process_id FROM processes`
(`audience_based_access_control_plan.md:317-319`) that matters concretely: with
`MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` configured (the Overview's mandated escape hatch) plus
Stage 5 stamping, a process's *pre-stamping* partition rows (unstamped, coalesced to the escape-hatch
audience) would keep making that process visible to `group:everyone` forever after it is stamped with
a real, narrower audience, because an `IN`/`EXISTS` check admits a process the moment any one of its
historical rows matches.

**Decided: resolve one audience per process before filtering (§3–§6), not per row — the fail-closed
option, applied uniformly, including `processes`'s own scan.** The semi-join/`EXISTS` subqueries built
in §3–§6 do not filter `__processes__partitions` rows directly; they first collapse it to one row per
`process_id` via `Aggregate(GROUP BY process_id, MAX(audience_col) AS resolved_audience)`. `MAX` over a
nullable column ignores `NULL`s, so a process with any stamped (non-null) partition row resolves to
that stamped value rather than to an unstamped default, closing the leak above — including for
`processes`'s own merged view: `df.into_view()` inlines the raw `__processes__partitions` rows below its
own `GROUP BY`/`first_value` merge, so a per-row filter on `processes`'s own scan would leak exactly the
way a naive raw-row filter for §4–§6 would have, keeping a process's merged row visible to the escape-hatch audience
forever after a later, narrower stamp. This is still built from the raw, time-unbounded partitions
(§2's `query_range: None` decision is unchanged) and still computed once per `analyze()` call and reused
at every scan site the traversal visits (the "once per query" mitigation, point 1 above) — it adds one
`Aggregate` node to that shared subplan, not a new per-site cost. It assumes a process is stamped with
at most one distinct audience over its lifetime (true under Stage 5's design); Stage 3/#1371 should
revisit this if that assumption changes. `processes`'s own scan (§3) uses the same `process_id IN
(subquery)` construction as §4, filtering the outer scan's own `process_id` column against the shared
`per_process_audience`/`resolved_predicate` subplan — there is no separate per-row branch.

### 3. `processes`'s own scan — same `process_id IN (subquery)` construction as §4

The `processes` view carries the audience as a property. Rather than filtering the outer scan's own
rows against their own `property_get`'d audience directly (which would leak stale rows the way §2's
"Any-row semantics" note describes), build `audience_col` — `property_get`, cast to `Utf8`, **no
`coalesce` yet** — and use it only as the (nullable) input to the shared, resolved-per-process
aggregate; the `coalesce` with `unstamped_audience` is applied *after* the aggregate, to
`resolved_audience`, not before it — applying `coalesce` per-row and then taking `MAX` would let the
constant default outrank a real stamped value under plain string ordering (e.g. `"user:alice"` sorts
below `"group:everyone"`), silently resolving a stamped process to the wrong audience. Filter the
outer `TableScan` by `process_id IN (subquery)` against that aggregate, the same shape §4 uses:

```rust
let audience_col = cast(
    property_get_udf.call(vec![col("properties"), lit("micromegas.audience")]),
    DataType::Utf8,
);
// per_process_audience: Aggregate(GROUP BY process_id, MAX(audience_col) AS resolved_audience) over
// `__processes__partitions` — built once per `analyze()` call; see §4 for the full construction,
// shared verbatim here rather than duplicated. `MAX` over the nullable, pre-coalesce `audience_col`
// ignores NULLs, so a stamped row always wins over an unstamped one within the same process.
let resolved_audience = match &self.unstamped_audience {
    Some(u) => coalesce_udf.call(vec![col("resolved_audience"), lit(ScalarValue::Utf8(Some(u.clone())))]),
    None => col("resolved_audience"),
};
let resolved_predicate = if audiences.is_empty() {
    lit(false) // see empty-audience-set note below
} else {
    resolved_audience.in_list(
        audiences
            .iter()
            .map(|a| lit(ScalarValue::Utf8(Some(a.clone()))))
            .collect(),
        false, // not negated
    )
};
let subquery = LogicalPlanBuilder::from(per_process_audience.clone())
    .filter(resolved_predicate.clone())?
    .project(vec![col("process_id")])?
    .build()?;
let predicate = in_subquery(col("process_id"), Arc::new(subquery));
```

`property_get_udf` is `Arc::new(ScalarUDF::from(PropertyGet::new())).call(args)` — `ScalarUDF::call`
(`datafusion-expr::udf::ScalarUDF::call(&self, args: Vec<Expr>) -> Expr`) builds the `Expr` directly;
no session lookup needed since `OwnershipRewrite` can construct its own `PropertyGet` instance the
same way `register_extension_udfs` does (`datafusion-extensions/src/lib.rs:78`) rather than fetching
the one already registered on `ctx`. `coalesce_udf` is `datafusion::functions::expr_fn::coalesce`
(the built-in `coalesce` scalar function, `datafusion-functions::core::coalesce`). `property_get`
returns `Dictionary(Int32, Utf8)` (`property_get.rs:87-92`), so every expression built here is
explicitly cast/typed to `Utf8` rather than left for implicit coercion: `SessionState::add_analyzer_rule`
appends to `analyzer.rules` (`datafusion-54.1.0/src/execution/session_state.rs:375`), and
`Analyzer::new()`'s built-ins are `[ResolveGroupingFunction, TypeCoercion]`
(`datafusion-optimizer-54.1.0/src/analyzer/mod.rs:88-91`) — `OwnershipRewrite` runs strictly *after*
`TypeCoercion`, so expressions it injects get no coercion pass at all. (The `query_processes.rs:73`
precedent that motivated "implicit coercion handles this" doesn't carry over: that is SQL *text*
parsed before the analyzer runs, so it does go through `TypeCoercion`.) The explicit `cast` above and
the `ScalarValue::Utf8` literals make the `coalesce` and `in_list` operands agree without relying on
that pass. No outer `cast(col("process_id"), ...)` is needed here the way §4 needs one: both sides of
this `IN` subquery come from the same `processes` table (`__processes__partitions`'s `process_id` is
already `Utf8`), unlike §4's outer views whose `process_id` is `Dictionary(Int32, Utf8)`.

**Empty audience set.** `ReadScope::Audiences` can resolve to an empty list — `identity_and_group_audiences`
adds nothing for an API-key `AuthContext` with no email, no groups claim and no implicit groups
(`auth/src/policy.rs:88-103`). Rather than emitting `resolved_audience IN ()` and leaving its behavior
to DataFusion, short-circuit to `lit(false)` (show nothing) whenever `audiences` is empty, as shown
above — this is the fail-closed reading of "caller has no audiences," and the same short-circuit
governs `resolved_predicate` everywhere it is reused below (§4–§6).

`Some(mat_view)` where `mat_view.get_view().get_view_set_name().as_str() == "processes"` is the
match arm that produces this branch. `processes` is reachable only via the named global table —
it is registered via `add_global_view`, not `add_view_set`, so `view_instance('processes', id)` is not
a valid call (`ViewFactory::make_view` only looks in `view_sets`, `view_factory.rs:259-265`); see the
corrected Current State landscape table and the resolved Open Questions.

### 4. Process_id-**column** views — semi-join, one shared helper

For every other view whose `mat_view.schema()` contains a field named `process_id`
(`streams`, `blocks`, `log_entries`, `measures`, `net_spans`, `otel_spans`, `images`, `log_stats` —
see the table in Current State), inject:

```
process_id IN (
    SELECT process_id FROM (
        SELECT process_id, MAX(<audience_col>) AS resolved_audience
        FROM __processes__partitions GROUP BY process_id
    ) WHERE <coalesce+IN predicate from §3, applied to resolved_audience>
)
```

per §2's "resolve one audience per process, not per row" decision — **not** a bare
`WHERE <processes predicate from §3>` filter over the raw per-row scan, which would admit a process
via any one of its historical (possibly pre-stamping, unstamped) partition rows. Built with
`LogicalPlanBuilder`:

```rust
let per_process_audience = LogicalPlanBuilder::scan(
    "__processes__partitions",
    self.processes_source.clone(),
    None,
)?
.aggregate(
    vec![col("process_id")],
    vec![max(audience_col.clone()).alias("resolved_audience")],
)?
.build()?; // audience_col is the same property_get(properties, "micromegas.audience") expr as §3,
           // reused here as the aggregate's input rather than as a row-level filter
let resolved_predicate = /* same coalesce(..., unstamped_audience) + IN(audiences) shape as §3's
                             `resolved_audience`/`resolved_predicate`, built over
                             col("resolved_audience") (already Utf8, no property_get/cast needed at
                             this layer) instead of audience_col */;
let subquery = LogicalPlanBuilder::from(per_process_audience)
    .filter(resolved_predicate)?
    .project(vec![col("process_id")])?
    .build()?;
let predicate = in_subquery(cast(col("process_id"), DataType::Utf8), Arc::new(subquery));
```

The outer `process_id` is cast to `Utf8` for the same analyzer-ordering reason as §3: it is
`Dictionary(Int32, Utf8)` in `log_entries` (`log_entries_table.rs:27`), `measures`
(`metrics_table.rs:21`), `net_spans` (`net_spans_table.rs:44`) and `otel_spans`
(`otel/spans_table.rs:12`), while `processes.process_id` (the subquery's projected column) is `Utf8` —
`InListExpr`/`InSubqueryExec` assert the two sides' data types match rather than coercing them
(`datafusion-physical-expr-54.1.0/src/expressions/in_list.rs:234-239`), and this rule runs after
`TypeCoercion` (§3), so the cast must be explicit here too. The scan is named
`"__processes__partitions"` (the raw per-partition `MaterializedView`, not the `SqlBatchView`-merged
`processes` view — see Current State) purely for readability; `self.processes_source` is passed
directly, so no session-side name lookup happens. The `per_process_audience`/`resolved_predicate`
subplan is the one built once per `analyze()` call and reused at every process_id-column scan site
(§2's cost mitigation) — §5/§6 below reuse it too, in place of filtering `processes_predicate` over raw
rows directly.

`in_subquery` (`datafusion_expr::expr_fn::in_subquery(expr: Expr, subquery: Arc<LogicalPlan>) ->
Expr`) produces an **uncorrelated** `IN` subquery (it references no column from the outer plan) —
DataFusion's `DecorrelatePredicateSubquery` optimizer rule turns this into a `LeftSemi` join during
optimization, after the analyzer phase this rule runs in. This is why no I/O happens inside
`analyze()`: the rule only builds a syntactically valid logical plan; the actual `processes` scan and
the join execute later, during normal query execution, exactly like the existing time-range filter's
subplan does.

This same construction works regardless of how the outer view is reached: as the named global table
(`streams`, `blocks`, `log_stats` — global only, no `view_instance` access, per the corrected Current
State table) or via `view_instance('log_entries'|'measures'|'net_spans'|'otel_spans'|'images', id)` —
the `MaterializedView`'s own schema decides the branch, not how it was reached.

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
let subquery = LogicalPlanBuilder::from(per_process_audience.clone()) // §4's shared aggregate, reused
    .filter(col("process_id").eq(lit(view_instance_id)).and(resolved_predicate.clone()))?
    .build()?;
let predicate = exists(Arc::new(subquery)); // datafusion_expr::expr_fn::exists
```

Filtering `col("process_id").eq(lit(view_instance_id))` against `per_process_audience` (already
one row per process) rather than against raw `__processes__partitions` rows directly matters for the
same reason as §4: a literal-`process_id` filter over the raw partitions would still exhibit the
any-row leak §2 describes (an unstamped historical row for this process would independently satisfy
`processes_predicate` even after the process is stamped). Reusing the aggregate here costs nothing
extra since §4 already builds it once per `analyze()` call.

wrapped around the whole `TableScan` exactly like `TableScanRewrite`'s time filter (`Filter::try_new(pred,
Arc::new(plan.clone()))`) — every row of the scan is either entirely visible or entirely hidden,
which is correct since every row of this instance belongs to the same one process.

### 6. Stream-scoped, no key column at all — two-hop literal check

`thread_spans` is scoped by `stream_id`, not `process_id`, and its schema (shared with
`process_spans`'s output, `span_table.rs`) has neither column. The `view_instance_id` **is** the
stream_id literal (`ThreadSpansView::new`, `thread_spans_view.rs:91`). Resolve it through `streams`
(which has both `stream_id` and `process_id` — `streams_view.rs:28`) into `processes`:

```rust
let subquery = LogicalPlanBuilder::scan("__streams__partitions", self.streams_source.clone(), None)?
    .filter(col("stream_id").eq(lit(view_instance_id)))?
    .join(
        LogicalPlanBuilder::from(per_process_audience.clone()) // §4's shared aggregate, reused
            .filter(resolved_predicate.clone())?
            .build()?,
        JoinType::Inner,
        (vec!["process_id"], vec!["process_id"]),
        None,
    )?
    .build()?;
let predicate = exists(Arc::new(subquery));
```

Joining through `per_process_audience` here instead of a raw, per-row `processes_predicate` filter
closes the same any-row leak §2/§4 describe: the stream's owning process resolves to one audience
value, not to whichever of its historical partition rows happens to satisfy the predicate.

This is the one construction this issue's own scope statement doesn't spell out (it only says
"semi-join on `process_id`-keyed views"), and it is the one place the AbAC plan's §4 and §5's Prong B
section talk past each other slightly: §4 lists Prong A as covering "`view_instance('<set>', <id>)`
... caught as a `TableScan<MaterializedView>` ... exactly like a named view," which — taken literally
— includes `thread_spans`; but the concrete `process_id`/`stream_id` **cache** machinery §4 describes
is scoped explicitly to Prong B ("Prong B performance", Stage 3).

**Decided: cover both `async_events` and `thread_spans` in Stage 2**, via the literal-valued `EXISTS`
subqueries in §5/§6, rather than deferring them to Stage 3. Two reasons settle this, not just a
preference: (1) the AbAC plan's Prong B caches (`process_id`/`stream_id` moka caches) are scoped to
`list_partitions` **row filtering** — a different problem from a plan-time literal `view_instance_id`,
so deferring these two branches would not actually be "waiting for the caches," it would just be
leaving two named, queryable view sets unfiltered for a stage with nothing to do with them; and (2)
the AbAC plan's §5/§5b posture is explicitly fail-closed, and Current State already establishes that
`OwnershipRewrite` "must not fall through to skip this view" — leaving `async_events`/`thread_spans`
unfiltered would be exactly that fall-through, for two view sets reachable by any caller who knows a
`view_instance(...)` call. Resolving it now costs nothing extra — the `stream_id`/`process_id` is a
plan-time literal either way, no runtime cache required.

### 7. Public view sets — skip the branch entirely

Before any of §3–6 run, check `self.public_view_sets.contains(mat_view.get_view().get_view_set_name().as_str())`;
if true, `Transformed::no(plan)` — no predicate at all, for any view kind. This is the one part of §4
the issue text names directly ("Branch per view set via `MaterializedView::get_view_set_name()`").
Default empty (§8), so inert unless configured — matches the AbAC plan §5b's "off by default,
fail-closed" framing. No enforcement of the AbAC plan's operator-responsibility constraint ("only
genuinely aggregated / non-PII view sets") beyond documentation — same posture the plan itself takes.

**Fallback for anything matching none of §3–§7.** The branches above key on
`view_set_name == "processes"`, a `process_id` schema field, and the two named exceptions
(`async_events`, `thread_spans`); a future view set, or one from a caller-supplied `ViewFactory`
(`FlightSqlServerBuilder::with_view_factory_fn`), can match none of them. Per Current State's "must
not fall through to skip this view," this case must not silently produce an unfiltered scan. Fail
loudly instead of quietly hiding rows: return `Err(DataFusionError::Plan(format!("OwnershipRewrite:
no audience rule defined for view set '{view_set}'")))` from `analyze()`, naming the unhandled view
set. A named plan error surfaces the gap at development/test time as a build-breaking omission in the
new view set's own PR, rather than as silent, hard-to-diagnose empty results that could be mistaken
for correct access denial.

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
today. **Decided: (b)**, settled by the parent plan rather than left as a judgment call — the AbAC
plan's §5b states the public allowlist (the same shape of per-service config as
`OwnershipRewriteConfig`) "is resolved once per request alongside `ReadScope` and threaded to both
prongs," and its §6 puts per-service *objects* (`ReadPolicy`, `session_configurator`) on the service
while resolved per-request values ride the context — exactly option (b). Stage 1 already made this
same call for the same reason, bundling two authorization inputs (`read_scope`, `is_admin`) into one
struct rather than growing `make_session_context`'s parameter list (`read_scope.rs:34-50`). Option (b)
also touches only the 3 files that construct `CallerContext` by struct literal today (`read_scope.rs`'s
definition and its two constructors; `flight_sql_service_impl.rs`'s `caller_context()` resolver; and
`analytics/tests/lakehouse_admin_gate_test.rs`, the one test that builds `CallerContext { .. }`
directly rather than via `::internal()`/`::maintenance()` — verified by `grep -rln "CallerContext {"`,
three hits total including the definition). Every other `make_session_context` call site is untouched
because it already goes through `::internal()`/`::maintenance()`.

The trade-off against (b): `OwnershipRewriteConfig` is not really a property of *the caller* the way
`read_scope`/`is_admin` are — it is deployment config that happens to ride along. Accepted for the
same reason Stage 1 accepted bundling `is_admin` and `read_scope` into one struct in the first place
(`1369_policy_seam_plan.md` §3): the two are visited together at every real call site anyway, and a
struct with a slightly-impure field beats re-touching a parameter list that Stage 1 already grew
once.

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
   `processes`-own-scan branch (§3, same `process_id IN (subquery)` construction as §4), the
   process_id-column semi-join branch (§4), the
   process-scoped-no-column literal branch (§5, `async_events`), the stream-scoped two-hop literal
   branch (§6, `thread_spans`), the public-view-set skip (§7), and the `ReadScope::All` no-op. Doc
   comments carry the per-view-set schema table from Current State — the next added view set is the
   next reader who needs it.
4. `rust/analytics/src/lakehouse/mod.rs` — `pub mod ownership_rewrite;`.
5. `rust/analytics/src/lakehouse/query.rs::make_session_context` — after the global-view registration
   loop (Design §2), when `caller.read_scope != ReadScope::All`: build `processes_source` /
   `streams_source` directly from `view_factory.get_global_view(...)` + `MaterializedView::new(...,
   query_range: None)` (not via `ctx.table_provider`, per Design §2), construct `OwnershipRewrite` from
   `caller.read_scope` + `caller.ownership_config`, and `ctx.add_analyzer_rule(...)`. **Must be added
   after the `query_range.is_some()` block that registers `TableScanRewrite` (`query.rs:228-230`)** —
   not merely placed anywhere independent of it — because `TableScanRewrite::analyze` traverses
   subqueries and would time-bound the audience lookup `OwnershipRewrite` injects if it ran on a later
   pass (Design §2). The two rules still gate on different inputs (time range vs. read scope); only
   their relative registration order is constrained.

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
9. Update the call sites verified against `d0364c950`: `FlightSqlServiceImpl::new(` is called from
   `rust/public/src/servers/flight_sql_server.rs:281` (production, picks up the new field naturally
   via Phase 3's wiring) and `rust/public/tests/read_policy_threading_tests.rs:81` (the one test
   fixture that needs an `ownership_config` argument added). Struct-literal `CallerContext { .. }`
   outside `read_scope.rs` exists only at `flight_sql_service_impl.rs:551` and
   `analytics/tests/lakehouse_admin_gate_test.rs:38` (already covered by step 2).
   Additionally, `start_server` in `rust/public/tests/read_policy_threading_tests.rs:79` builds its
   `ViewFactory` as `Arc::new(ViewFactory::new(vec![]))`, and every test in that file configures an
   `ApiKeyAuthProvider` — so `caller_context()` resolves a non-`All` `ReadScope::Audiences(..)` for
   all of them. Per Design §2, `make_session_context` now hard-fails on `get_global_view("processes"
   /"streams")` for such a scope, which breaks
   `read_scope_resolves_from_auth_context_not_claimed_attribution` (:290),
   `prepared_statement_resolves_the_same_scope_as_do_get` (:328),
   `auth_context_with_groups_survives_the_real_tonic_stack` (:363), and
   `unconfigured_deployment_resolves_a_scope_and_query_results_are_unaffected` (:402) before any SQL
   is planned. Fix `start_server` to build its `ViewFactory` with real `processes`/`streams` global
   views registered — mirroring `default_view_factory(runtime, lake)`
   (`analytics/src/lakehouse/view_factory.rs:269-337`), or the `make_processes_view`/
   `make_streams_view` calls used directly in `analytics/tests/thread_spans_ordering_db_test.rs:311,321`
   — **not** `lakehouse_admin_gate_test.rs`'s `ViewFactory::new(vec![])`, which registers no
   `SqlBatchView` at all (step 11 makes this same point about that file) — so these tests' restricted
   scopes resolve successfully; this is a required fixture change, not an optional cleanup. Both
   precedents are `async` and only *plan* the `SqlBatchView`'s transform query (`SqlBatchView::new`
   calls `ctx.sql(...)` to plan, never to execute, `sql_batch_view.rs:107-125`), so a `connect_lazy`
   Postgres pool is sufficient for this offline fixture.

### Phase 4 — tests (issue's own acceptance criteria, step 7)

10. New DB-backed test file, `rust/analytics/tests/ownership_rewrite_db_test.rs` (mirrors
    `net_spans_retire_overlap_db_test.rs`'s "requires a live `MICROMEGAS_SQL_CONNECTION_STRING`" /
    `MICROMEGAS_OBJECT_STORE_URI` convention): seed two processes via the real ingestion pipeline
    (or direct `processes`/`blocks` SQL inserts, matching how `sql_telemetry_db.rs`'s tables are
    shaped), manually set `micromegas.audience` in each process's Postgres `properties` row to two
    different values (since ingestion stamping doesn't exist until Stage 5) **before the `blocks`
    view's partitions are materialized** — not merely before the `processes` view's own
    materialization. `BlocksView::data_sql` snapshots `processes.properties` from Postgres into the
    `blocks` parquet partitions at materialization time (`blocks_view.rs:59-70`, schema field
    `processes.properties`), and the `processes` `SqlBatchView`'s transform query reads
    `first_value("processes.properties") ... FROM blocks` — i.e. from the already-materialized
    `blocks` partitions, never from Postgres directly (`processes_view.rs:27-45`). Setting the
    audience after `blocks` materializes but before `processes` materializes would still bake in no
    audience at all; assert, through `make_session_context` with different `CallerContext`s:
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
11. New file `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs` (offline, no DB,
    no seeded data — plan-shape assertions only, unlike step 10). Unlike
    `lakehouse_admin_gate_test.rs`'s `ViewFactory::new(vec![])`, build the session with a
    `ViewFactory` that registers `processes`/`streams` (Design §2/Issue 1 requires this for
    `OwnershipRewrite` to even be constructible) alongside a test view set, under a restricted
    `ReadScope::Audiences([...])` so the rule is actually registered and active. Assert purely on the
    analyzed `LogicalPlan`/`EXPLAIN` text: a view set named in `MICROMEGAS_PUBLIC_VIEW_SETS` plans
    with no injected `Filter`/`InSubquery`/`Exists` node, while a non-public view set does — no row
    data or DB fixture involved. Add two more plan-shape assertions in this same offline harness,
    covering the plan's two other fail-closed guards that otherwise have no test coverage:
    - a view set present in the `ViewFactory` but absent from `analyze()`'s branch table (§3–§7) — a
      minimal test-only view set matching none of the `processes`/`process_id`-column/`async_events`/
      `thread_spans`/public-set arms — makes `analyze()` return `Err` with §7's named
      `DataFusionError::Plan("OwnershipRewrite: no audience rule defined for view set '...'")`, not an
      unfiltered plan;
    - a session constructed with `ReadScope::Audiences(Arc::from([]))` (the empty set
      `identity_and_group_audiences` can legitimately produce, §3) plans a `lit(false)` predicate — not
      an unfiltered scan and not an `IN ()` left to DataFusion — for an ordinary process_id-column view.

## Files to Modify

- `rust/analytics/src/lakehouse/ownership_rewrite.rs` — **new**; the rule
- `rust/analytics/src/lakehouse/read_scope.rs` — `OwnershipRewriteConfig`, `CallerContext` field;
  update the stale "`ReadScope` is dropped" doc comments at `:10-13` and `:44-45` to say it is now
  consumed by `OwnershipRewrite` (Prong A)
- `rust/analytics/src/lakehouse/mod.rs` — register the module
- `rust/analytics/src/lakehouse/query.rs` — construct + register `OwnershipRewrite` when
  `caller.read_scope != ReadScope::All`
- `rust/public/src/servers/flight_sql_server.rs` — `with_ownership_config()`, default resolution
- `rust/public/src/servers/flight_sql_service_impl.rs` — new field, constructor param,
  `caller_context()`
- `rust/monolith/src/main.rs` — resolve + wire `OwnershipRewriteConfig::from_env("MICROMEGAS_ANALYTICS")`;
  update the stale doc comment at `:249`
- `rust/auth/src/policy.rs` — update the stale "nothing consumes `ReadScope` yet" doc comments at
  `:5-6` and `:187`
- `rust/analytics/tests/lakehouse_admin_gate_test.rs` — one `CallerContext` literal
- `rust/public/tests/read_policy_threading_tests.rs` — `ownership_config` argument (step 9),
  `ViewFactory` fixture fix (Design §2/Issue 1), and update the stale doc comments at `:7-9` and
  `:399`
- `rust/analytics/tests/ownership_rewrite_db_test.rs` — **new**
- `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs` — **new** (offline, planning-only)
- `mkdocs/docs/admin/flight-sql.md` — `MICROMEGAS_UNSTAMPED_AUDIENCE`/`MICROMEGAS_PUBLIC_VIEW_SETS`/
  `MICROMEGAS_IMPLICIT_GROUPS` rows
- `mkdocs/docs/admin/monolith.md` — same three rows
- `CHANGELOG.md` — the escape-hatch upgrade note, and the breaking-API note for the new
  `FlightSqlServiceImpl::new` parameter and the new public `CallerContext` field (see Documentation)
- `tasks/data_isolation/audience_based_access_control_plan.md` — record the resolved
  `async_events`/`thread_spans` treatment, the `CallerContext`-field decision, and the
  parsed-in-`micromegas-analytics` note (see Documentation)

## Trade-offs

- **`processes`/`streams` as the audience source: an unbounded `MaterializedView` scan, not Prong
  B's Postgres-backed cache (Design §2).** Every restricted query pays at least one full,
  uncached, unpruned `processes` (and, for `thread_spans`, `streams`) partition scan — mitigated
  from "once per touched table" to "once per query" by computing the §3/§4 subplans once per
  `analyze()` call and reusing them, but not eliminated — and Prong A's visibility lags however far
  behind the maintenance daemon's last `materialize_all_views` pass the deployment is (or sees
  nothing if the daemon isn't running), diverging from Prong B's Postgres point lookup during that
  window. Accepted for Stage 2 because a shared cache is Prong B's Stage 3 (#1371) deliverable, not
  new machinery this issue should build a second copy of; flagged for Stage 3 to have
  `OwnershipRewrite` consume that cache once it exists, so both prongs agree on one source of truth.
- **`CallerContext` field vs. a new `make_session_context` parameter** for `OwnershipRewriteConfig`
  — see Design §8. Decided: the `CallerContext` field (option (b)), settled by the AbAC plan's §5b/§6
  (per-request resolved values ride the context; per-service objects live on the service) and by
  Stage 1's own precedent bundling `read_scope`/`is_admin` the same way — not left as an open call.
- **Literal-valued `EXISTS` subqueries for `async_events`/`thread_spans` (§5/§6) vs. deferring those
  two view sets to Stage 3.** The issue text ("semi-join on `process_id`-keyed views") could be read
  as scoping Prong A to the views that actually have the column, leaving `async_events`/`thread_spans`
  unfiltered until Stage 3's caches land. Decided: cover them now, because leaving two named,
  queryable view sets completely unfiltered — reachable by any caller who knows a `view_instance(...)`
  call, with no gate at all — is a bigger, more surprising hole than the `TODO(#1371)` sites (which
  are at least documented and bounded to three specific internal recursive contexts), costs no
  runtime machinery to close (the key is a plan-time literal either way), and the AbAC plan's Prong B
  caches solve a different problem entirely (`list_partitions` row filtering, not plan-time literals)
  — so deferring would not actually be "waiting for the caches."
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
  `ReadScope::All` short-circuits, why every branch — including `processes`'s own scan — resolves one
  audience per process via the shared subquery rather than filtering raw rows (§2/§3), and the
  async_events/thread_spans literal-check rationale (§5/§6) — this is exactly the kind of non-obvious
  "why" that will not survive the next contributor's skim of `view_factory.rs`
  without it written down here.
- Update the six in-code doc-comment sites that currently assert "nothing consumes `ReadScope` yet"
  (accurate for Stage 1, falsified once `OwnershipRewrite` lands) to instead say `ReadScope` is now
  consumed by `OwnershipRewrite` (Prong A), with Prong B (the UDTF/UDF guards) still pending #1371:
  `rust/analytics/src/lakehouse/read_scope.rs:10-13` and `:44-45`, `rust/auth/src/policy.rs:5-6` and
  `:187`, `rust/monolith/src/main.rs:249`, and `rust/public/tests/read_policy_threading_tests.rs:7-9`
  and `:399`.
- `tasks/data_isolation/audience_based_access_control_plan.md` — record, once implemented: the exact
  `async_events`/`thread_spans` treatment (§5/§6), since the plan's own §4 doesn't fully resolve it;
  the `CallerContext`-vs-new-parameter decision (§8); and that `OwnershipRewriteConfig`'s two knobs
  are parsed in `micromegas-analytics`, not `micromegas-auth` (mirrors Stage 1's own "parse where
  consumed" note about `MICROMEGAS_UNSTAMPED_AUDIENCE`/`MICROMEGAS_PUBLIC_VIEW_SETS`,
  `1369_policy_seam_plan.md` §5).
- `mkdocs/docs/admin/flight-sql.md` and `mkdocs/docs/admin/monolith.md` — add
  `MICROMEGAS_UNSTAMPED_AUDIENCE`, `MICROMEGAS_PUBLIC_VIEW_SETS`, **and `MICROMEGAS_IMPLICIT_GROUPS`**
  rows to each page's existing `MICROMEGAS_*` environment-variable table (the same tables documenting
  `MICROMEGAS_ADMINS`, `MICROMEGAS_STATIC_TABLES_URL`, etc. today), with a pointer to Stage 7's
  isolation page for the full activation story. `MICROMEGAS_IMPLICIT_GROUPS` is a pre-existing knob
  (`rust/auth/src/policy.rs`) that today is documented only in `CHANGELOG.md:31` (`grep -rn
  IMPLICIT_GROUPS mkdocs/` returns nothing) — since the Overview's required escape-hatch pair is
  `MICROMEGAS_IMPLICIT_GROUPS=everyone` **and** `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone`, adding
  only the two new knobs would leave half that pair undiscoverable from these pages. Cross-reference
  the pair from the CHANGELOG upgrade note below. These pages, not the CHANGELOG, are the
  operator-facing reference for `MICROMEGAS_*` knobs — Stage 1's "no doc page yet" precedent was safe
  because Stage 1 was inert, but Stage 2 silently empties every query result for auth-enabled
  deployments unless this pair is configured (Overview), so it must be discoverable from the admin
  pages an operator actually reads, not only from the CHANGELOG.
- `CHANGELOG.md` per the `pr` skill's convention — must include an explicit upgrade note: any
  deployment running with auth enabled must set **both** `MICROMEGAS_IMPLICIT_GROUPS=everyone` and
  `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone`, in the same deploy that ships `OwnershipRewrite` —
  setting `MICROMEGAS_UNSTAMPED_AUDIENCE` alone does not restore visibility (Overview) — or every
  `ReadScope::Audiences` caller goes to zero visible rows the moment this ships. Also record, as a
  **Minor breaking change** (same convention Stage 1 used, `CHANGELOG.md:31`): `FlightSqlServiceImpl::new`
  gains a required `ownership_config` parameter (step 7), and the public, struct-literal-constructed
  `CallerContext` gains a new public field, `ownership_config` (step 1).

## Testing Strategy

- DB-backed cross-audience tests (Implementation Steps, step 10) are the issue's own acceptance
  criteria verbatim: cross-audience queries return nothing, same-audience returns its own rows,
  unstamped rows follow the `MICROMEGAS_UNSTAMPED_AUDIENCE` knob, `ReadScope::All` sees everything —
  extended to explicitly cover the two schema-less view sets (`async_events`, `thread_spans`) the
  naive reading of the issue text would miss.
- Offline planning tests (step 11) for the public-view-set allowlist: no live DB, plan-shape
  assertions only, over a `ViewFactory` that (unlike `lakehouse_admin_gate_test.rs`) registers
  `processes`/`streams` so a restricted `ReadScope` actually activates the rule. The same offline
  harness also asserts the plan's two other fail-closed guards, which otherwise ship with no coverage:
  an unhandled view set (one matching none of §3–§7's branches) produces §7's named `Plan` error rather
  than an unfiltered scan, and an empty `ReadScope::Audiences(Arc::from([]))` plans a `lit(false)`
  predicate rather than an unfiltered scan or a bare `IN ()`.
- `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 build/rust_ci.py`.
- Manual smoke, auth-unset path: run the monolith with `MICROMEGAS_IMPLICIT_GROUPS`/
  `MICROMEGAS_UNSTAMPED_AUDIENCE` left unset (today's default) and confirm existing queries against
  real ingested data are byte-for-byte unaffected.
- Manual smoke, auth-enabled path (the one the auth-unset check above cannot exercise): run with an
  `AudienceReadPolicy` active (auth on, **both** `MICROMEGAS_IMPLICIT_GROUPS=everyone` and
  `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` set per the upgrade note in Documentation —
  `MICROMEGAS_UNSTAMPED_AUDIENCE` alone is not sufficient, Overview) and confirm a caller under
  `ReadScope::Audiences` still sees their own legacy, never-stamped data — this is the regression
  Overview now flags as a breaking change for every auth-enabled deployment, not a hypothetical.

## Open Questions

None — all resolved during review:

1. ~~`OwnershipRewriteConfig` on `CallerContext` vs. a new `make_session_context` parameter~~ —
   settled as option (b), the `CallerContext` field (Design §8, Trade-offs).
2. ~~Covering `async_events`/`thread_spans` in Stage 2 via literal `EXISTS` subqueries (§5/§6) vs.
   deferring them to Stage 3~~ — settled: cover both now (§5/§6, Trade-offs).
3. ~~Should `view_instance('processes', <id>)` even be reachable/tested?~~ — settled: no. `processes`
   (and `streams`, `blocks`) are registered via `add_global_view`, never `add_view_set`, so
   `ViewFactory::make_view`/`ViewInstanceTableFunction` cannot reach them
   (`view_factory.rs:259-265`); `view_instance('processes'|'streams'|'blocks', id)` is simply not a
   valid call, named-table access is the only path, and no dead code is needed for it (Current State
   landscape table, Design §3).
