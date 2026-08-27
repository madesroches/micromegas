# Drop `OwnershipRewrite`'s Redundant Audience Predicate for Guarded View Instances Plan (#1530)

## Overview

`OwnershipRewrite` (Prong A) injects an audience-resolving subquery into every scan of a view set
that has no physical `audience` column of its own: a `process_id IN (per_process_audience)`
semi-join for `net_spans`/`otel_spans`/`images`, and a literal-valued `EXISTS` for
`async_events`/`thread_spans`. All five of those view sets are reachable only through
`view_instance(view_set, id)`, which `AudienceGuard::authorize_view_instance` (Prong B) already
authorizes against Postgres before any row is read. The injected predicate therefore re-derives a
conclusion Prong B has already established, at the cost of a time-unbounded scan of
`__processes__partitions`, an `Aggregate`, and a `LeftSemi` join on every such query.

This plan removes that predicate for scans that are provably Prong-B-authorized, keyed on a
structural property of the scan (does it carry an instance guard, and is its instance id
non-`'global'`) rather than on a hardcoded view-set name list — so a view set that ever gains an
unguarded query path keeps its predicate automatically. It is a query-planning cleanup, not a
security change: no gap is closed and none is opened.

## Current State

### The five predicate shapes and where they apply

`OwnershipRewrite::predicate_for` (`rust/analytics/src/lakehouse/ownership_rewrite.rs`) dispatches
on the scanned view's file schema:

| View set | Branch | Injected predicate |
|---|---|---|
| `processes`, `streams`, `blocks`, `log_entries`, `measures`, `log_stats` | §5 | `audience IN (caller audiences)` — a bare `Filter` on the view's own column |
| `net_spans`, `otel_spans`, `images` | §4 | `CAST(process_id AS Utf8) IN (SELECT process_id FROM per_process_audience WHERE resolved_audience IN (...))` |
| `async_events` | §async_events | `EXISTS (SELECT … FROM per_process_audience WHERE process_id = '<instance id>' AND resolved_audience IN (...))` |
| `thread_spans` | §thread_spans | two-hop `EXISTS` through `__streams__partitions` |
| anything on `MICROMEGAS_PUBLIC_VIEW_SETS` | §7 | none |
| anything else | — | `Err(DataFusionError::Plan)` — fail closed |

`per_process_audience()` is
`Aggregate(GROUP BY process_id, MAX(audience))` over the raw, **time-unbounded**
`__processes__partitions`. It is built once per `analyze()` and shared across scan sites, but each
scan site that uses it plans its own copy of the subquery, which
`DecorrelatePredicateSubquery` turns into a join over the deployment's entire process inventory.

### All five predicate-carrying view sets are instance-only

None of `net_spans`, `otel_spans`, `images`, `async_events`, `thread_spans` has a global instance:

- They are registered with `add_view_set(...)` only, never `add_global_view(...)`
  (`view_factory.rs:339`, `355`, `361`, `367`, `373`), and `query.rs:317` registers only
  `view_factory.get_global_views()` as SQL-visible tables. `SELECT * FROM net_spans` does not
  resolve to a table.
- Each constructor rejects `"global"` outright: `images_view.rs:76`, `net_spans_view.rs:82`,
  `otel/spans_view.rs:79`, `async_events_view.rs:81`, `thread_spans_view.rs:84`.
- The only construction path left is `ViewInstanceTableFunction::call_with_args`
  (`view_instance_table_function.rs:83`), which is the **only** site that passes
  `Some(guard)` into `MaterializedView::new`.

### Prong B runs before any row is read

`MaterializedView::scan` (`materialized_view.rs:79-86`) calls
`guard.authorize_view_instance(view_set_name, view_instance_id)` *before* `jit_update` and before
`part_provider.fetch`. Its arms (`audience_guard.rs`):

1. `ReadScope::All` → pass (Prong A is a no-op for that scope anyway).
2. view set on `public_view_sets` → pass (Prong A's §7 already emits no predicate for it).
3. `view_instance_id == "global"` → pass, deliberately, because global instances are row-filtered
   rather than call-guarded.
4. instance id parses as a `Uuid` → `authorize(id, IdKind::ProcessOrStream)`, a live Postgres point
   query, fail-closed on `Unknown`/`Ambiguous`/resolution error.
5. anything else → the same uniform denial.

So for a scan that carries a guard and whose instance id is not `'global'`, arm (4) or (5) runs:
either the caller is authorized for that exact process/stream id, or the scan errors out.

### Rows are confined to the instance

Partitions are keyed by `(view_set_name, view_instance_id, file_schema_hash)` in
`part_provider.fetch` (`materialized_view.rs:91-99`), and each of the five views materializes only
the blocks of the process/stream named by its instance id. A scan of instance *X* can therefore only
return rows belonging to *X*.

### Answering the issue's open question: JIT-partition caching

The issue flags as unverified whether a cached JIT partition could be reused across a request
scoped to a different credential. It can, and it does not matter:

- Materialization is server-side and credential-independent — `jit_update` runs under
  `CallerContext::internal()` and writes the same bytes regardless of who triggered it. There is no
  per-credential dimension in the partition key, and none is needed.
- Reuse is confined to the same `view_instance_id`, and access to that id is re-checked by Prong B
  on **every** request (the `AudienceIndex` cache is keyed on `(IdKind, Uuid)`, TTL-bounded, and
  holds the id's *owner*, not a caller's verdict).

So partition reuse across credentials never widens what a caller can read, and the semi-join has no
remaining scenario to catch on this path.

### What the semi-join actually still defends against

Prong B does not anchor on the *process's* stamp for all five view sets — `authorize_view_instance`
calls `authorize(id, IdKind::ProcessOrStream)` for every guarded instance, and
`owner_query_sql(ProcessOrStream)` is a `UNION ALL` over `processes.audience` **and**
`streams.audience`. For a process-scoped instance (`net_spans`/`otel_spans`/`images`/`async_events`)
Prong B may therefore authorize off a *streams* row with the same id, while Prong A's
`per_process_audience`/`exists_for_process` always resolves the *process's* materialized row — the
same anchor shift the paragraph below documents for `thread_spans`, present in the other four too.
Ingestion does not prevent the collision: `check_stream_audience_conflict`
(`rust/ingestion/src/web_ingestion_service.rs`) only compares an incoming `stream_id` against
existing `streams` rows, so a client-chosen `stream_id` may equal another audience's `process_id`.
What makes this safe is `merge_owner_rows`' fail-closed `Ambiguous` merge: when the id resolves to
*both* a `processes` row and a `streams` row with different audiences, `is_readable` requires every
resolved audience to be independently readable, so a collision denies rather than authorizing off
whichever row happens to match the caller.

Net of the collision case, what remains is Prong A / Prong B disagreement, and the two read very
different things: Prong B resolves a live point query against `processes`/`streams`; Prong A's
`per_process_audience` is built from `__processes__partitions`, a time-partitioned, append-only JIT
materialization off `blocks` (`processes_view.rs`) that is never retroactively edited once a time
range is materialized. `delete_old_data` (`rust/analytics/src/delete.rs`) deletes a process's row
from live `processes` once it is old and stream-empty, but that deletion neither retracts the
process's already-materialized `__processes__partitions` rows nor the instance's own JIT partitions
(`net_spans`/`otel_spans`/`images`/`async_events`), which is exactly the "retention swept the
`processes` row, everything downstream of it survives" case. This produces a genuine, if narrow,
allow-direction skew that only the collision case makes reachable: if a client-chosen `stream_id`
happens to equal a `process_id` that has since aged out of `processes`, the collision that used to
be `Ambiguous` (both rows present) resolves cleanly to the surviving `streams` row once the
`processes` row is gone, so Prong B authorizes the stream's own owner — who is not the aged-out
process's owner — to open `view_instance` for that id. Today's semi-join still denies them: it reads
the stale-but-persisted `__processes__partitions` row, which still carries the original process's
audience, and filters out any caller whose audience doesn't match it. Dropping the predicate gives
up exactly this one backstop; the rest of the skew (a freshly-arrived process not yet visible in
`__processes__partitions`) is invisible to Prong A in the deny direction, not the allow direction.

`thread_spans` has the same process/stream anchor shift as the four above, but through a different
pair of accessors: Prong A's `exists_for_stream` resolves the **owning process's** stamp (it joins
`__streams__partitions` to `per_process_audience` on `process_id`), while Prong B's
`IdKind::ProcessOrStream` arm resolves the **stream's own** `streams.audience` directly (or, on a
process_id/stream_id collision, both, fail-closed as above). These can also differ for a reason
that is not a collision: `insert_stream` accepts any `process_id` unconditionally and stamps the
stream with the caller's own audience, not the process's. That shift is covered by `blocks_view`'s
mismatch predicate: `ThreadSpansView::jit_update` generates partitions through `BlocksView`, whose
mismatch predicate excludes a block whose stream disagrees with the owning process's row, so a
stream/process audience mismatch never reaches this scan regardless of which stamp Prong A or Prong
B reads. This is not treated as a realistic exposure and is not a reason to keep the predicate.

It notably does **not** defend against cross-audience block injection: an attacker's block naming a
victim's `process_id` lands in the victim's instance, so `process_id IN (victim's audience)` passes.
That case is closed at `blocks_view` materialization by the mismatch predicate, not here.

## Design

### 1. `MaterializedView` exposes whether Prong B will authorize this scan

`instance_guard` is private and `OwnershipRewrite` sees the `MaterializedView` only through a
downcast. Add one accessor whose name states the property, not the field:

```rust
impl MaterializedView {
    /// Whether `AudienceGuard::authorize_view_instance` will resolve this view's instance id
    /// against the caller's scope, and deny, before `scan` yields a row. True only for a
    /// caller-named, non-`'global'` `view_instance(...)`: those take the guard's Uuid arm (or its
    /// fail-closed fallthrough). `'global'` is excluded because the guard passes it unconditionally
    /// -- global instances are row-filtered instead.
    ///
    /// Kept in step with `AudienceGuard::authorize_view_instance`'s arms; changing those means
    /// revisiting this.
    pub fn instance_is_audience_guarded(&self) -> bool {
        self.instance_guard.is_some() && self.view.get_view_instance_id().as_str() != "global"
    }
}
```

The guard's public-view-set arm needs no mirroring here: `OwnershipRewrite`'s §7 already returns
`Ok(None)` for a public view set before any of this is reached, and both read
`caller.isolation_config.public_view_sets`.

### 2. `predicate_for` classifies first, then decides

The skip must not swallow the fail-closed `Err` for an unrecognised view set. Split
classification from predicate construction so the skip applies only to branches whose
instance-confinement has been verified:

```rust
/// Which of the module doc's branches a scanned view falls into. Classification is separate from
/// predicate construction so the "already authorized by Prong B" skip can apply to exactly the
/// three instance-confined branches and nothing else -- an unrecognised view set still fails
/// closed even when it is reached through a guarded `view_instance(...)`.
enum AudienceBranch {
    /// §7
    Public,
    /// §5 -- carries a physical `audience` column.
    AudienceColumn(Field),
    /// §4 -- `process_id` column, no `audience` column.
    ProcessIdColumn,
    /// §async_events -- process-scoped, instance id is the process id.
    ProcessInstance,
    /// §thread_spans -- stream-scoped, instance id is the stream id.
    StreamInstance,
}

impl AudienceBranch {
    /// Whether every row this branch's scan can return belongs to the single telemetry id named
    /// by the view's `view_instance_id` -- the property that makes Prong B's authorization of
    /// that one id sufficient, with no per-row predicate needed. False for `AudienceColumn`: its
    /// global instances span every process, and its per-process instances are filtered by a bare
    /// column comparison that costs nothing to keep.
    fn rows_confined_to_instance(&self) -> bool {
        matches!(self, Self::ProcessIdColumn | Self::ProcessInstance | Self::StreamInstance)
    }
}
```

`predicate_for` becomes:

```rust
let branch = self.classify(&view)?;               // Err for an unrecognised view set
if branch.rows_confined_to_instance() && mat_view.instance_is_audience_guarded() {
    return Ok(None);
}
self.build_predicate(branch, table_name, view, ...)
```

`classify` holds today's dispatch verbatim (public list → `audience` field → `process_id` field →
`async_events` → `thread_spans` → `Err`); `build_predicate` holds today's four predicate shapes
verbatim. No predicate expression changes.

### 3. What stays

`per_process_audience`, `in_subquery_plan`, `exists_for_process`, `exists_for_stream`, and
`OwnershipRewrite`'s `processes_source`/`streams_source` constructor arguments all stay. Under
today's `default_view_factory` they become unreachable for the five view sets, but they remain the
fail-closed default for any scan of a `process_id`-column view set that is *not* a guarded instance
— for example a deployment that registers a `process_id`-carrying view as a global table via
`add_global_view`, which is public API. Deleting them would make that case plan an unfiltered scan.

### 4. Plan-shape effect

Before (`SELECT * FROM view_instance('net_spans', '<pid>')`, restricted scope):

```
LeftSemi Join: CAST(net_spans.process_id AS Utf8) = __processes__partitions.process_id
  TableScan: net_spans
  Projection: process_id
    Filter: resolved_audience IN (...)
      Aggregate: groupBy=[process_id], aggr=[max(audience)]
        TableScan: __processes__partitions      <-- whole process inventory, time-unbounded
```

After:

```
TableScan: net_spans
```

## Scope: three view sets or five?

The issue names `net_spans`/`otel_spans`/`images` (the §4 semi-join). The identical argument holds
for `async_events`/`thread_spans`: same instance-only registration, same `"global"` rejection, same
guarded-only construction path, same partition-level instance confinement. Their `EXISTS` is cheaper
than the semi-join but still plans a scan of `__processes__partitions` (plus
`__streams__partitions` for `thread_spans`) and, after `DecorrelatePredicateSubquery`, a
`LeftSemi Join`.

**This plan covers all five.** One structural rule over five view sets is simpler than a rule that
special-cases three of them, and it leaves no "why is `async_events` different?" question behind.
Narrowing to three is a one-line change to `rows_confined_to_instance` if that is preferred.

## Implementation Steps

### Phase 1 — the accessor

1. `rust/analytics/src/lakehouse/materialized_view.rs`: add `instance_is_audience_guarded()` with
   the doc comment from Design §1.

### Phase 2 — the rewrite rule

2. `rust/analytics/src/lakehouse/ownership_rewrite.rs`: introduce `AudienceBranch` +
   `classify()` + `build_predicate()`, moving today's dispatch and predicate construction across
   unchanged.
3. Same file: apply the skip in `predicate_for` per Design §2.
4. Same file: update the module doc's branch table — add the skip rule as a row, note that the
   §4/§async_events/§thread_spans predicates now apply only to a scan that is *not* a guarded
   instance, and drop the "What remains open" bullet's implication that these five are filtered
   per-scan by Prong A. Keep the description behavioural: no issue numbers, no stage labels.

### Phase 3 — tests

5. `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs`:
   - Keep `non_public_process_id_only_view_plans_with_an_injected_semi_join` as-is — its
     `ProcessIdOnlyView` is registered as a **global** view, so it is exactly the fail-closed
     regression test that the skip is keyed on the guard and not on the schema.
   - Add its mirror: register a second `process_id`-column view set through `add_view_set` and
     assert `view_instance('<it>', '<pid>')` plans with **no** `Filter` and no `LeftSemi Join`.
     Together the two prove the key is the query path, not the view.
   - Flip `async_events_view_instance_plans_with_an_injected_exists` and
     `thread_spans_view_instance_plans_with_an_injected_two_hop_exists` to assert the absence of an
     injected predicate, renaming them accordingly.
   - Update the `default_view_factory` inventory test (`~line 520`) from a two-way split to a
     three-way one, keyed on the `AudienceBranch` (Design §2), not on the access path:
     `AudienceColumn` — any view carrying the physical `audience` column, whether reached as a
     global table or through `view_instance(...)` (`log_entries`/`measures` are registered both
     ways and must still assert the bare `Filter` shape there) — → bare `Filter` on `audience`;
     no `audience` column, reached as a global table → `LeftSemi Join`; no `audience` column,
     reached through a guarded `view_instance(...)` → no injected predicate at all. The panic
     message must keep pointing at "a view set is missing a branch in `OwnershipRewrite`" for the
     planning failure case, since `classify`'s `Err` is unchanged.
   - Add a case asserting an unrecognised view set reached through `view_instance(...)` still
     `Err`s — the guard must not turn `classify`'s fail-closed fallback into a silent pass.
6. `rust/analytics/tests/ownership_rewrite_db_test.rs`: no code changes here. This file's only
   `view_instance(...)` sections are `log_entries`, `async_events`, and `thread_spans` — it has no
   `net_spans` section, and none of the three §4 view sets (`net_spans`/`otel_spans`/`images`) are
   seeded in it. Its existing cross-audience assertions (Prong B's uniform denial text) and the
   owning caller's `> 0` / `ReadScope::All` row-count checks for `async_events` and `thread_spans`
   already serve as the regression net for this change: rerun this file unmodified and confirm
   they still pass.
7. `rust/analytics/tests/audience_guard_tests.rs`: add a unit test pinning
   `instance_is_audience_guarded()` against `authorize_view_instance`'s arms — guard present +
   `'global'` → false; guard present + UUID → true; guard absent → false. This is the coupling
   Design §1 calls out.

### Phase 4 — docs and changelog

8. `mkdocs/docs/admin/authentication.md`, "Audience Filtering Activation": rewrite the sentence
   "the `net_spans`/`otel_spans`/`images` view sets (which don't carry the column) keep the
   semi-join through `processes`, and `async_events`/`thread_spans` keep their literal `EXISTS`
   shapes" to describe current behaviour — those five view sets are reachable only through
   `view_instance(...)`, where the call-level audience check on the instance id is the enforcement,
   and Prong A adds no per-row predicate. Also soften the earlier claim that "Prong A already
   row-filters every `view_instance` scan the same as the named-table form", which is now true only
   for the view sets carrying an `audience` column.
9. `rust/analytics/src/lakehouse/audience_guard.rs`: update the module doc's claim that Prong B's
   `view_instance(...)` guard exists only to close a cost/availability residual "because Prong A
   already row-filters every `view_instance` scan the same as the named-table form" — after this
   change that is true only for the view sets carrying a physical `audience` column; for the other
   five, Prong B's guard is their sole confidentiality enforcement, not a redundant belt-and-braces
   check. Add the reverse-side pointer to `OwnershipRewrite`'s `instance_is_audience_guarded()` so
   a reader of either file finds the other. Describe this behaviourally, matching Phase 2 step 4's
   convention: no issue numbers, no stage labels.
10. `CHANGELOG.md`, Unreleased: one entry under the analytics/query section describing the dropped
   predicate, the view sets affected, and that it is a planning cleanup: no caller gains access to
   another audience's rows, though a legitimate owner may now see rows that the daemon-materialized
   `processes` snapshot's lag previously hid from the dropped predicate (#1530).

## Files to Modify

- `rust/analytics/src/lakehouse/materialized_view.rs` — new accessor
- `rust/analytics/src/lakehouse/ownership_rewrite.rs` — branch enum, skip rule, module doc
- `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs` — plan-shape expectations
- `rust/analytics/tests/ownership_rewrite_db_test.rs` — no changes; rerun as the regression net
- `rust/analytics/tests/audience_guard_tests.rs` — accessor/guard-arm coupling test
- `rust/analytics/src/lakehouse/audience_guard.rs` — module doc, reverse-side coupling note
- `mkdocs/docs/admin/authentication.md` — enforcement description
- `CHANGELOG.md` — Unreleased entry

No SQL-surface change: no view/table/UDF names, schemas, or column orders are touched.

## Trade-offs

**Structural key (guard presence) vs. a view-set name list.** The issue proposes special-casing
three view sets by name. A name list fails *open* if one of those view sets ever gains an unguarded
query path — for instance a deployment calling `add_global_view(net_spans_maker.make_view(pid))`,
which is public API today. Keying on `instance_is_audience_guarded()` makes the default (no guard →
keep the predicate) the safe one, and needs no edit when a new instance-only view set is added.
The cost is a coupling between `OwnershipRewrite` and `AudienceGuard`'s arm structure, pinned by the
Phase 3 step 7 test and a doc comment on both sides.

**Deleting `per_process_audience` and friends.** Tempting — with the skip in place they are
unreachable under `default_view_factory`, and dropping them would let `OwnershipRewrite::new` shed
its `processes_source`/`streams_source` arguments and `query.rs` shed the two `MaterializedView`
constructions that feed them. Rejected: they are the fail-closed fallback for the unguarded-scan
case above, and removing them converts that case from "over-filtered" to "unfiltered".

**Replacing the semi-join with a literal `process_id = '<instance id>'` filter** instead of removing
it. Cheap (prunable, no subquery) and it verifies confinement rather than assuming it. Rejected as
redundant: partitions are already keyed by `view_instance_id`, so the filter can only ever be a
tautology, and it would be wrong for a future view instance that legitimately spans processes.

**Keeping the predicate as defence-in-depth against Prong A/B skew.** Rejected: the two prongs read
the same per-row stamp and differ only in materialization/retention lag, which does not produce a
false *allow*; and the semi-join provides no protection against the injection scenarios that are
actually closed elsewhere.

## Security

No authorization decision changes. For every reachable query:

- In the ordinary case, no caller gains access to another audience's rows. A caller authorized for
  instance *X* saw *X*'s rows before, further narrowed by any Prong A/Prong B materialization-lag
  skew (Current State); after, they see exactly *X*'s rows with that skew gone — a legitimate owner
  may now see rows that Prong A's `processes`-materialization lag previously hid. This can only add
  visibility for the authorized owner, never grant it to anyone else. The one exception is the
  retention-window collision documented in Current State: a client-chosen `stream_id` equal to a
  `process_id` that has since aged out of `processes` lets Prong B authorize that stream's own
  owner for the surviving `net_spans`/`otel_spans`/`images`/`async_events` instance partitions of
  the aged-out process, which today's semi-join still denies (it reads the audience the
  never-retracted `__processes__partitions` row still carries) and which dropping the predicate
  gives up.
- Prong B anchors on `IdKind::ProcessOrStream` — `processes.audience` or `streams.audience`,
  whichever row exists — for all five view sets, not only `thread_spans`; the fail-closed
  `Ambiguous` merge in `merge_owner_rows` is what makes that anchor safe whenever both rows exist
  (see Current State and the bullet above for the one case where only one does). `thread_spans` is
  additionally covered on its own legitimate-divergence path: `insert_stream` may stamp a stream
  with the caller's own audience regardless of its process's, but a stream stamped differently from
  its owning process is already excluded by `blocks_view`'s mismatch predicate before this scan
  runs, so that particular anchor shift changes no caller's visible rows.
- A caller not authorized for *X* was denied by Prong B at `scan` before and is denied at `scan`
  after — including the empty-`ReadScope::Audiences` case, where `is_readable` returns false rather
  than the predicate's `lit(false)` producing an empty result.
- A scan reaching Prong A without a guard (no such path in `default_view_factory`) keeps its
  predicate.

The residual gaps recorded in `ownership_rewrite.rs`'s module doc — legacy NULL-anchor rows, the
per-instance audience label being resolved through the owning process rather than a per-row column
— are unchanged by this work.

## Performance

Per affected query, this removes: one time-unbounded `TableScan` of `__processes__partitions`
(plus `__streams__partitions` for `thread_spans`), one `Aggregate` over the deployment's whole
process inventory, and one `LeftSemi Join`. The saving scales with process count, not with the
result size, so it is largest exactly where it hurts most — a small `view_instance('thread_spans',
…)` query on a deployment with a large process history.

## Testing Strategy

- `cargo test -p micromegas-analytics --test ownership_rewrite_public_view_set_tests` — offline
  plan-shape assertions; the primary evidence that the predicate is gone for guarded instances and
  still present for global scans.
- `cargo test -p micromegas-analytics --test ownership_rewrite_db_test` — DB-backed, two seeded
  audiences; evidence that the *rows* a scoped caller sees are unchanged and cross-audience access
  is still denied with the uniform not-found text.
- `cargo test -p micromegas-analytics --test audience_guard_tests` — accessor/guard-arm coupling.
- `cargo test -p micromegas-analytics` and `cargo clippy --workspace -- -D warnings` for the rest.
- Manual sanity check against the local test env: run `view_instance('thread_spans', <stream_id>)`
  under an authenticated (non-`--disable-auth`) session and confirm via `EXPLAIN` that no
  `LeftSemi Join` appears, and that the same query under a foreign audience still fails with
  `view_instance: '<id>' not found or not accessible`.

## Open Questions

None blocking. One decision recorded rather than asked: the scope covers all five instance-only
view sets rather than the three named in the issue (see **Scope** above); narrowing is a one-line
change if that is not wanted.
