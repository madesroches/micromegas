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
unguarded query path keeps its predicate automatically. This is primarily a query-planning
cleanup: in the ordinary case no gap is closed and none is opened. It does give up one narrow,
already-fail-closed-elsewhere backstop — a retention-window collision that lets Prong A deny an
access Prong B allows — detailed in Security below.

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
  (`view_instance_table_function.rs:83`), which is the **only** production site that passes
  `Some(guard)` into `MaterializedView::new` (the one other is the stub fixture in
  `audience_guard_tests.rs`).

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
`per_process_audience` is built from `__processes__partitions`, a time-partitioned, append-only
`SqlBatchView` materialization off the `blocks` view (`processes_view.rs`), populated only by the
maintenance daemon's batch pass (`SqlBatchView::jit_update` is a no-op) and never retroactively
edited once a time range is materialized. `delete_old_data` (`rust/analytics/src/delete.rs:152-169`) deletes a
process's row from live `processes` once it is old and stream-empty, and in the same pass calls
`retire_expired_partitions` over the same `expiration` cutoff, which deletes every
`lakehouse_partitions` row with `end_insert_time < expiration` (`write_partition.rs:94-101`) --
including `__processes__partitions` and the instance's own JIT partitions
(`net_spans`/`otel_spans`/`images`/`async_events`). The two cutoffs aren't identical, though:
partitions retire on `end_insert_time < expiration` while the source blocks are deleted on
`insert_time <= expiration`, so the boundary, day-sized partition can survive for up to one
partition width after its source row is gone -- the same bound `tasks/completed/1482_audience_column_plan.md`
and `tasks/completed/1371_udtf_udf_guards_plan.md` document for this identical mechanism, not an
indefinite survival. This produces a genuine, if narrow and time-bounded,
allow-direction skew that only the collision case makes reachable: if a client-chosen `stream_id`
happens to equal a `process_id` that has since aged out of `processes`, the collision that used to
be `Ambiguous` (both rows present) resolves cleanly to the surviving `streams` row once the
`processes` row is gone, so Prong B authorizes the stream's own owner — who is not the aged-out
process's owner — to open `view_instance` for that id. Today's semi-join still denies them: it reads
the stale-but-persisted `__processes__partitions` row, which still carries the original process's
audience, and filters out any caller whose audience doesn't match it. Dropping the predicate gives
up exactly this one backstop, bounded to roughly one partition width after the process row itself
ages out of `processes` (see the partition-retirement bound above); the rest of the skew (a
freshly-arrived process not yet visible in `__processes__partitions`) is invisible to Prong A in
the deny direction, not the allow direction.

`thread_spans` has the same process/stream anchor shift as the four above, but through a different
pair of accessors: Prong A's `exists_for_stream` resolves the **owning process's** stamp (it joins
`__streams__partitions` to `per_process_audience` on `process_id`), while Prong B's
`IdKind::ProcessOrStream` arm resolves the **stream's own** `streams.audience` directly (or, on a
process_id/stream_id collision, both, fail-closed as above). These can also differ for a reason
that is not a collision: `insert_stream` accepts any `process_id` unconditionally and stamps the
stream with the caller's own audience, not the process's. That shift is covered by `blocks_view`'s
mismatch predicate: `ThreadSpansView::jit_update` generates partitions through `BlocksView`, whose
mismatch predicate drops any block whose own stamp differs from either its `streams` row or its
`processes` row (`audience_column_mismatch`, `blocks_view.rs` -- it never compares the two rows to
each other). A block under a stream/process disagreement cannot match both, so it never reaches
this scan regardless of which stamp Prong A or Prong B reads. The one exception is a legacy
NULL-stamped block, which the predicate's NULL-tolerant pass-through lets in -- that is the
already-documented NULL-anchor window, not a new gap. This is not treated as a realistic exposure
and is not a reason to keep the predicate.

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

### 2. `predicate_for` skips the three subquery predicates for a guarded instance

The skip must not swallow the fail-closed `Err` for an unrecognised view set. Rather than
splitting classification out from predicate construction, this falls out of `predicate_for`'s
existing dispatch as-is: the §4/`process_id`, §async_events, and §thread_spans arms each get one
early-return guard; the §7 public arm, the §5 `audience`-column arm, and the fallthrough `Err` arm
are untouched.

```rust
let skip = mat_view.instance_is_audience_guarded();
// existing dispatch, otherwise untouched:
//   §7 public arm      -> unchanged
//   §5 audience arm    -> unchanged
//   §4 process_id arm     -> `if skip { return Ok(None); }` before today's body
//   §async_events arm     -> `if skip { return Ok(None); }` before today's body
//   §thread_spans arm     -> `if skip { return Ok(None); }` before today's body
//   fallthrough Err    -> unchanged
```

Three inserted lines plus the one accessor from Design §1: no new types, no function split, no
reordering, no change to any predicate expression.

Fail-closed for an unrecognised view set is preserved **by construction**, not by an argument about
classification order — the `Err` arm is never touched.

The §5 `audience`-column arm is left alone for a simpler reason than "its rows aren't confined to
the instance": its filter is a bare column comparison with no subquery and no join, so there is
nothing worth skipping. The operative rule is "skip the predicates that cost a subquery", which is
exactly the three arms above.

The property this relies on — a guarded instance's rows can never legitimately span more than one
process — holds for today's five view sets but isn't independently verified. That's recorded as a
short comment on the `skip` line itself: a future guarded view set whose instance can legitimately
span more than one process would need to revisit this before its arm gets the same skip. It's a
note for whoever adds the next branch, not a contract enforced anywhere.

### 3. What stays

`per_process_audience`, `in_subquery_plan`, `exists_for_process`, `exists_for_stream`, and
`OwnershipRewrite`'s `processes_source`/`streams_source` constructor arguments all stay. Under
today's `default_view_factory` they become unreachable for the five view sets, but they remain the
fail-closed default for any scan of a `process_id`-column view set that is *not* a guarded instance
— for example a deployment that registers a `process_id`-carrying view as a global table via
`add_global_view`, which is public API. Deleting them would make that case plan an unfiltered scan.
`exists_for_process`/`exists_for_stream` are retained on the same fail-closed-fallback basis but no
longer have direct test coverage: constructing an unguarded registration for them requires fixtures
that cost more than the coverage is worth. `per_process_audience`/`in_subquery_plan` keep their
coverage via the retained global `ProcessIdOnlyView` test (Phase 3 step 4).

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

## Scope: all five instance-only view sets

The issue names `net_spans`/`otel_spans`/`images` (the §4 semi-join). The identical argument holds
for `async_events`/`thread_spans`: same instance-only registration, same `"global"` rejection, same
guarded-only construction path, same partition-level instance confinement. Their `EXISTS` is cheaper
than the semi-join but still plans a scan of `__processes__partitions` (plus
`__streams__partitions` for `thread_spans`) and, after `DecorrelatePredicateSubquery`, a
`LeftSemi Join`.

**This plan covers all five — confirmed, not merely proposed.** One structural rule over five view
sets is simpler than a rule that special-cases three of them, and it leaves no "why is
`async_events` different?" question behind.

## Implementation Steps

### Phase 1 — the accessor

1. `rust/analytics/src/lakehouse/materialized_view.rs`: add `instance_is_audience_guarded()` with
   the doc comment from Design §1.

### Phase 2 — the rewrite rule

2. `rust/analytics/src/lakehouse/ownership_rewrite.rs`: add the `skip` boolean via
   `instance_is_audience_guarded()` and the three `if skip { return Ok(None); }` guards in the
   §4/process_id, §async_events, and §thread_spans arms of `predicate_for`, per Design §2 — no new
   types, no function split, no reordering.
3. Same file: update the module doc's branch table — add the skip rule as a row, note that the
   §4/§async_events/§thread_spans predicates now apply only to a scan that is *not* a guarded
   instance, and drop the "What remains open" bullet's implication that these five are filtered
   per-scan by Prong A. Two earlier paragraphs make the same claim and need the same treatment:
   the "physical `audience` column" section's closing sentence (the five "keep the
   `process_id`/`EXISTS` machinery below, which still resolves through `__processes__partitions`")
   and the "One audience per row" section's closing sentence (the §4 semi-join and the
   §async_events/§thread_spans `EXISTS` shapes "still resolve through `per_process_audience`").
   Keep the description behavioural: no issue numbers, no stage labels.

### Phase 3 — tests

4. `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs`:
   - Keep `non_public_process_id_only_view_plans_with_an_injected_semi_join` as-is — its
     `ProcessIdOnlyView` is registered as a **global** view, so it is exactly the fail-closed
     regression test that the skip is keyed on the guard and not on the schema.
   - Add a case against a **real** view set instead of a synthetic mirror: `default_view_factory`
     already registers `net_spans` via `add_view_set`, so `view_instance('net_spans', '<uuid>')` is
     plannable with no new fixture. Assert it plans with **no** `Filter` and no `LeftSemi Join`.
     This plus the retained global `ProcessIdOnlyView` test above is what proves the key is the
     query path, not the view — no synthetic second `process_id`-column view set (and the
     `ViewMaker` wrapper it would need) is required.
   - Flip `async_events_view_instance_plans_with_an_injected_exists` and
     `thread_spans_view_instance_plans_with_an_injected_two_hop_exists` (the guarded
     `view_instance(...)` forms) to assert the absence of an injected predicate, renaming them
     accordingly.
   - Update the `default_view_factory` inventory test
     (`real_view_factory_covers_every_registered_view_set`) to a two-way split keyed on the
     `audience` column plus whether the scan is a guarded instance: a view carrying the physical
     `audience` column → bare `Filter` on `audience` (whether reached globally or through
     `view_instance(...)`; `log_entries`/`measures` are registered both ways and must still assert
     the bare `Filter` shape there); no `audience` column reached through a guarded
     `view_instance(...)` → no injected predicate at all. The panic message must keep pointing at
     "a view set is missing a branch in `OwnershipRewrite`" for the planning failure case, since the
     dispatch's fallthrough `Err` arm is unchanged.
5. `rust/analytics/tests/ownership_rewrite_db_test.rs`: no test-behavior changes — its assertions
   (Prong B's uniform denial text, and the owning caller's `> 0` / `ReadScope::All` row-count
   checks for `async_events` and `thread_spans`) already serve as the regression net for this
   change: rerun this file unmodified and confirm they still pass. Its comments do need updating,
   though: the module doc (`:13-17`) and the in-body comment before the `view_instance(...)`
   section (`:408-417`) both currently say Prong A's own row-filtering behavior for
   `async_events`/`thread_spans` "is exercised separately, by the plan-shape tests in
   `ownership_rewrite_public_view_set_tests.rs`" — after Phase 3 step 4 those tests assert the
   opposite for the guarded-instance path. Reword both comments to say Prong A injects no predicate
   at all for a guarded `view_instance(...)` scan of these two view sets.
6. `rust/analytics/tests/audience_guard_tests.rs`: add a unit test pinning
   `instance_is_audience_guarded()` against `authorize_view_instance`'s arms — guard present +
   `'global'` → false; guard present + UUID → true; guard absent → false. This is the coupling
   Design §1 calls out. The file's existing `JitUpdateMustNotRunView` stub hardcodes a fresh
   `Uuid` instance id; give its `new` an instance-id parameter so the same stub serves the
   `'global'` case.

### Phase 4 — docs and changelog

7. `mkdocs/docs/admin/authentication.md`, "Audience Filtering Activation": rewrite the sentence
   "the `net_spans`/`otel_spans`/`images` view sets (which don't carry the column) keep the
   semi-join through `processes`, and `async_events`/`thread_spans` keep their literal `EXISTS`
   shapes" to describe current behaviour — those five view sets are reachable only through
   `view_instance(...)`, where the call-level audience check on the instance id is the enforcement,
   and Prong A adds no per-row predicate. Also soften the earlier claim that "Prong A already
   row-filters every `view_instance` scan the same as the named-table form", which is now true only
   for the view sets carrying an `audience` column. Also update the "Residual gap" warning box's
   bullet (`:351-356`) — "**Five process/stream-anchored view sets** … and **the per-process JIT
   `view_instance` path** still resolve their audience *label* through the owning process's/stream's
   row rather than a genuine per-row column of their own" — which mirrors the same claim in
   `ownership_rewrite.rs`'s module doc that Phase 2 step 3 rewrites there; keep the two in sync.
8. `rust/analytics/src/lakehouse/audience_guard.rs`: update the module doc's claim that Prong B's
   `view_instance(...)` guard exists only to close a cost/availability residual "because Prong A
   already row-filters every `view_instance` scan the same as the named-table form" — after this
   change that is true only for the view sets carrying a physical `audience` column; for the other
   five, Prong B's guard is their sole confidentiality enforcement, not a redundant belt-and-braces
   check. Add the reverse-side pointer to `OwnershipRewrite`'s `instance_is_audience_guarded()` so
   a reader of either file finds the other. Describe this behaviourally, matching Phase 2 step 3's
   convention: no issue numbers, no stage labels.
9. `CHANGELOG.md`, Unreleased: one new entry under the analytics/query section describing the
   dropped predicate, the view sets affected, and that it is a planning cleanup: no caller gains
   access to another audience's rows, though a legitimate owner may now see rows that the
   daemon-materialized `processes` snapshot's lag previously hid from the dropped predicate, and
   the one narrow, retention-bounded backstop given up. Note also that a caller with an
   empty `ReadScope::Audiences` now gets the guard's not-found error where the folded-away
   `lit(false)` predicate previously produced an empty result (#1530). Also amend, in place, the two prior
   still-`## Unreleased` entries this change falsifies — matching the file's own precedent of
   editing an unreleased entry rather than only appending (e.g. the existing `**Amended (#1482,
   still `## Unreleased`)**` and `**Amended (#1486, still `## Unreleased`)**` notes): the #1482
   entry's sentence "The `process_id IN (subquery)` semi-join described above now applies only to
   `net_spans`, `otel_spans`, and `images` … `async_events` and `thread_spans` keep their
   literal-valued `EXISTS` shapes unchanged" needs an amendment noting that all five view sets now
   plan with no injected predicate for a guarded `view_instance(...)` scan; the #1486 entry's
   sentence "Prong A already row-filters every `view_instance` scan the same as the named-table
   form" needs the same treatment, since that is now true only for the view sets carrying the
   physical `audience` column. Two more still-`## Unreleased` entries carry the same falsified
   claims and get the same in-place amendment: the standalone #1486 **Analytics** entry's "only to
   have `OwnershipRewrite` (Prong A) return it zero rows afterward" (there is no Prong A fallback
   for the five any more — Prong B's denial is the only enforcement), and the #1482 **Analytics**
   entry's closing sentence that the five "keep today's semi-join/`EXISTS` enforcement shapes,
   resolving through `__processes__partitions` as before". The #1486 entry's `'global'` and
   public-view-set exemption sentences stay accurate and are left alone.

## Files to Modify

- `rust/analytics/src/lakehouse/materialized_view.rs` — new accessor
- `rust/analytics/src/lakehouse/ownership_rewrite.rs` — skip lines, module doc
- `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs` — plan-shape expectations
- `rust/analytics/tests/ownership_rewrite_db_test.rs` — no test-behavior changes (rerun as the
  regression net); module doc and one in-body comment updated to match the new behavior
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
The cost is a coupling between `OwnershipRewrite` and `AudienceGuard`'s arm structure: one accessor
mirroring one guard arm, documented by its doc comment and pinned by the Phase 3 step 6 test.

**Deleting `per_process_audience` and friends.** Tempting — with the skip in place they are
unreachable under `default_view_factory`, and dropping them would let `OwnershipRewrite::new` shed
its `processes_source`/`streams_source` arguments and `query.rs` shed the two `MaterializedView`
constructions that feed them. Rejected: they are the fail-closed fallback for the unguarded-scan
case above, and removing them converts that case from "over-filtered" to "unfiltered".

**Replacing the semi-join with a literal `process_id = '<instance id>'` filter** instead of removing
it. Cheap (prunable, no subquery) and it verifies confinement rather than assuming it. Rejected as
redundant: partitions are already keyed by `view_instance_id`, so under every registration this
skip actually applies to, the filter can only ever be a tautology. The property this filter would
otherwise verify at plan time — a guarded instance's rows never legitimately span more than one
process — is exactly what the comment on the `skip` line (Design §2) already flags as needing
revisiting if a future guarded view set breaks it; that is an argument for the comment, not for
adding the literal filter.

**Keeping the predicate as defence-in-depth against Prong A/B skew.** Rejected: the divergence
between the two prongs is real (Current State) and does produce a genuine, if narrow, false
*allow* -- but that allow-direction skew is reachable only through the retention-window collision,
itself already fail-closed for as long as both a `processes` and a `streams` row exist (Current
State's `merge_owner_rows`/`Ambiguous` reasoning), and bounded to roughly one partition width once
one of them ages out (Current State). Keeping a time-unbounded scan on every guarded-instance query
to cover a residual that narrow is not a good trade; the semi-join also provides no protection
against the injection scenarios that are actually closed elsewhere (`blocks_view`'s mismatch
predicate).

## Security

No caller gains access to another audience's rows, with the one narrow exception recorded
below. For every reachable query:

- In the ordinary case, no caller gains access to another audience's rows. A caller authorized for
  instance *X* saw *X*'s rows before, further narrowed by any Prong A/Prong B materialization-lag
  skew (Current State); after, they see exactly *X*'s rows with that skew gone — a legitimate owner
  may now see rows that Prong A's `processes`-materialization lag previously hid. This can only add
  visibility for the authorized owner, never grant it to anyone else. The one exception is the
  retention-window collision documented in Current State: a client-chosen `stream_id` equal to a
  `process_id` that has since aged out of `processes` lets Prong B authorize that stream's own
  owner for the surviving `net_spans`/`otel_spans`/`images`/`async_events` instance partitions of
  the aged-out process, which today's semi-join still denies (it reads the audience the
  `__processes__partitions` row still carries for up to roughly one partition width after the
  process row itself is retired, per Current State) and which dropping the predicate gives up.
- Prong B anchors on `IdKind::ProcessOrStream` — `processes.audience` or `streams.audience`,
  whichever row exists — for all five view sets, not only `thread_spans`; the fail-closed
  `Ambiguous` merge in `merge_owner_rows` is what makes that anchor safe whenever both rows exist
  (see Current State and the bullet above for the one case where only one does). `thread_spans` is
  additionally covered on its own legitimate-divergence path: `insert_stream` may stamp a stream
  with the caller's own audience regardless of its process's, but a block under a stream stamped
  differently from its owning process is already excluded by `blocks_view`'s mismatch predicate (a
  block's own stamp cannot match both rows) before this scan runs, so that particular anchor shift
  changes no caller's visible rows.
- A caller not authorized for *X* was denied by Prong B at `scan` before and is denied at `scan`
  after — including the empty-`ReadScope::Audiences` case, where `is_readable` returns false rather
  than the predicate's `lit(false)` producing an empty result.
- A scan reaching Prong A without a guard (no such path in `default_view_factory`) keeps its
  predicate.

The residual gaps recorded in `ownership_rewrite.rs`'s module doc — legacy NULL-anchor rows, the
per-instance audience label being resolved through the owning process rather than a per-row column
— are unchanged in substance by this work; only their wording moves from "resolved by Prong A's
predicate" to "resolved by Prong B's instance check" (Phase 2 step 3, Phase 4 step 7).

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

No manual sanity check against the local test env: the documented split-mode
`local_test_env/ai_scripts/start_services.py` launches `flight-sql-srv` with `--disable-auth`,
which resolves to `ReadScope::All` and skips `OwnershipRewrite` entirely, so it cannot show the
`EXPLAIN` shape this change affects. (An OIDC-configured monolith or
`start_services_with_oidc.py` session could, but it is not required.) The offline plan-shape test
above already asserts exactly this shape change against a real (non-`All`) `ReadScope`.

## Open Questions

None. The one scope question — three view sets or five — is settled: all five instance-only view
sets get the skip, including `async_events` and `thread_spans` (see **Scope** above).
