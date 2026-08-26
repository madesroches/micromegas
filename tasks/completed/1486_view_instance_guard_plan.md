# `view_instance` Scan-Time Audience Guard Plan (#1486, AbAC Stage 3 residual)

## Overview

`MaterializedView::scan` runs `View::jit_update` before anything filters the caller's read
(`rust/analytics/src/lakehouse/materialized_view.rs:70`), so a `ReadScope::Audiences` caller who
names a view instance belonging to another audience can make the server materialize partitions
for data it will then return zero rows of. This is the availability/cost residual accepted in
AbAC Stage 3 (#1371 §7) and recorded in `tasks/completed/1371_udtf_udf_guards_plan.md` and
`rust/analytics/src/metadata.rs`. The epic plan
(`tasks/data_isolation/audience_based_access_control_plan.md`) does not mention this residual yet
— its `### Stage 3 — Enforcement Prong B` section predates it.

This plan closes it by adding a scan-time audience check on the `view_instance(...)` entry point
only: `AudienceGuard` gains a `authorize_view_instance` rule, `MaterializedView` gains an
optional guard field that only `ViewInstanceTableFunction` populates, and `MaterializedView::scan`
runs the check **before** `jit_update`. Nothing about Prong A, the implicitly-registered global
tables, or internal/maintenance callers changes.

## Current State

### The gap

```
view_instance('thread_spans', <stream in another audience>)
  → ViewInstanceTableFunction::call_with_args        (view_instance_table_function.rs:52)
      → ViewFactory::make_view                       (parses the id, no authorization)
      → MaterializedView::new                        (:76)
  → MaterializedView::scan                           (materialized_view.rs:62)
      → view.jit_update(...)                         (:70)   ← materializes; unguarded
      → part_provider.fetch(...)                     (:74)
  → OwnershipRewrite's injected Filter above the scan          ← returns 0 rows
```

`jit_update` reaches `find_stream_from_view` (`rust/analytics/src/metadata.rs:181`, its inner
`CallerContext::internal()` at `:204`) and `find_process_with_latest_timing` (`:308`, `:326`), both under `CallerContext::internal()`
(`ReadScope::All`). Neither returns caller-visible rows, so this is cost and error-text exposure,
not a leak. #1371 §7 deliberately left it: threading a `CallerContext` through the `View` trait's
`jit_update` would touch ~10 impls to gate reads that produce no caller-visible output.

### What already exists and is reused

- `AudienceGuard` (`rust/analytics/src/lakehouse/audience_guard.rs`): `authorize` (single-id,
  witness-returning, :384), `readable_ids` (:414), `global_rows_visible` (:444),
  `read_scope` (:375). Built once per request in `query.rs::register_lakehouse_functions` (:137)
  and shared by `Arc` across `process_spans`, `perfetto_trace_chunks`, `parse_block`,
  `get_payload` and `list_partitions`' row filter.
- `IdKind::ProcessOrStream`: the exact resolution `list_partitions` already uses for a
  `view_instance_id` — one `UNION ALL` round trip over `processes`/`streams`, fail-closed to
  `OwnerAudience::Ambiguous` on a `process_id`/`stream_id` collision.
- `AudienceIndex`'s cache is keyed on `(IdKind, Uuid)`, so a `view_instance` check and a
  `list_partitions` row filter over the same id share one cache entry.
- `AudienceGuard::authorize`'s uniform, existence-oracle-proof denial text:
  `{fname}: '{id}' not found or not accessible`.

### The view sets `view_instance` can reach

`ViewFactory`'s `view_sets` map (`view_factory.rs:339-373`) — global-only `SqlBatchView`s
(`processes`, `streams`, `blocks`, `log_stats`) are `global_views`, so `view_instance('processes',
…)` already fails with "view set not found".

| view set | instance id | `'global'` accepted? | `jit_update` for `'global'` |
|---|---|---|---|
| `log_entries` | `process_id` | yes (`log_view.rs:84`) | no-op (`:154`) |
| `measures` | `process_id` | yes (`metrics_view.rs:84`) | no-op (`:153`) |
| `images` | `process_id` | no — `ImagesView::new` bails | n/a |
| `otel_spans` | `process_id` | no — bails | n/a |
| `async_events` | `process_id` | no — bails (`:81`) | n/a |
| `net_spans` | `process_id` | no — bails (`:82`) | n/a |
| `thread_spans` | `stream_id` | no — bails (`:84`) | n/a |

**Every view set that accepts `'global'` no-ops its `jit_update` for it** — a `'global'` instance
has no JIT materialization to trigger, so it carries none of this issue's cost residual.

### The `MaterializedView` downcast constraint

`OwnershipRewrite::rewrite_plan` (`ownership_rewrite.rs:400-409`) reaches Prong A's predicate
through `ts.source.downcast_ref::<DefaultTableSource>()` then
`.table_provider.downcast_ref::<MaterializedView>()`, and returns `Transformed::no` (no predicate
at all) on a miss. **Wrapping the `view_instance` provider in a guarding `TableProvider` would
silently disable Prong A for every `view_instance` scan** — a confidentiality regression traded
for a cost fix. This constraint is what decides the design below.

## Design

### 1. `AudienceGuard::authorize_view_instance` — the whole rule, in one place

New method on `AudienceGuard`, next to `global_rows_visible`:

```rust
/// The `view_instance(view_set_name, view_instance_id)` scan-time check (#1486), run by
/// `MaterializedView::scan` before `jit_update` -- so a caller scoped to one audience cannot
/// trigger JIT materialization of an instance it cannot read.
pub async fn authorize_view_instance(
    &self,
    view_set_name: &str,
    view_instance_id: &str,
) -> datafusion::error::Result<()>
```

Rules, in order:

1. `ReadScope::All` → `Ok(())`, no I/O. (Internal, maintenance, `--disable-auth`, and every
   inner session built from an `Authorized::internal_caller()` take this arm.)
2. `view_set_name` on `public_view_sets` → `Ok(())`. Matches Prong A §7: an operator who declared
   a view set public has said every row of it is readable, so denying materialization of one of
   its instances would be incoherent. This is a confidentiality argument only: it leaves the
   cost/availability residual this plan exists to close fully open for any view set an operator
   puts on that list — any authenticated caller can still force `jit_update` for arbitrary
   process/stream ids of a public view set. Deliberate and recorded in §Security's "Not closed by
   this change".
3. `view_instance_id == "global"` → `Ok(())`. See §2 below.
4. `view_instance_id` parses as a `Uuid` → `self.authorize(uuid, IdKind::ProcessOrStream,
   "view_instance")`, discarding the returned `Authorized`. The witness exists to gate
   construction of an inner unscoped session; this call site builds none — `jit_update`'s
   `CallerContext::internal()` is unchanged and stays where it is (#1371 §7's reasoning still
   holds; what changes is only that a denied caller never reaches it).
5. anything else → the same uniform denial as (4). Fail-closed, matching `list_partitions`'
   "anything else is dropped" rule. Unreachable for today's view sets (every `make_view` either
   accepts `'global'` or `Uuid::parse_str`s, and fails at plan time otherwise), so this is a
   safety net against a future view set with a different id vocabulary.

Two small refactors keep this DRY:

- Extract the denial text into a private helper (`fn not_found_err<T>(fname: &str, id: &str) ->
  datafusion::error::Result<T>`) used by both `authorize` and rule (5), so the two denial shapes
  can't drift apart.
- Extract `pub fn is_public_view_set(&self, view_set_name: &str) -> bool` and have
  `global_rows_visible` call it, so rule (2) and the `'global'`-row rule read the same allowlist
  through one accessor.

### 2. `'global'` is row-filtered, not call-guarded — and that is not `list_partitions`' rule

The issue asks what `'global'` instances should do. **Settled: global instances are row-filtered;
only process- and stream-specific instances are guarded at call time.** So rule (3) passes
`'global'` through with no audience check and lets Prong A do the work, one row at a time. Three
reasons:

- **There is nothing to protect.** `jit_update` is a no-op for every `'global'` instance
  (`log_entries`, `measures`; see the Current State table), so a `'global'` scan triggers no
  materialization at all. The residual this issue exists to close does not apply to it.
- **Denying would break a legal query with no security gain.** `view_instance('log_entries',
  'global')` is exactly `SELECT * FROM log_entries` — the same `MaterializedView`, the same Prong
  A `audience IN (...)` filter on the physical `audience` column (#1482 §5). A scoped caller may
  run the named-table form today; making the `view_instance` spelling fail would be an arbitrary
  divergence.
- **`list_partitions`' `global_rows_visible` answers a different question.** There, a `'global'`
  row is *partition metadata* about a multi-audience file (path, size, row count) with no
  row-level filter available, so it is gated on the public allowlist or the lakehouse admin gate.
  Here, the `'global'` rows themselves are filtered one by one by Prong A. Reusing
  `global_rows_visible` would deny non-admin scoped callers the `log_entries`/`measures` global
  instances outright — a regression, not a hardening.

The invariant "`'global'` ⇒ no JIT" is what makes this sound, so it gets stated as such in the
doc comment on rule (3), with a pointer to the `View::jit_update` impls that uphold it. A future
view set whose `'global'` instance *does* materialize JIT would have to revisit this rule; there
is no way to assert it from `AudienceGuard`, which sees only the two strings.

### 3. Where the check runs: a field on `MaterializedView`, not a wrapper

`TableFunctionImpl::call_with_args` is synchronous, so the check cannot run there — audience
resolution is async. It has to happen in `TableProvider::scan`, which is the same shape
`process_spans`/`parse_block`/`perfetto_trace_chunks` already use (guard in `scan`, before the
work). And per Current State, it cannot happen in a *wrapper* provider without breaking Prong A's
downcast.

So `MaterializedView` itself carries the guard, as an `Option`:

```rust
pub struct MaterializedView {
    lakehouse: Arc<LakehouseContext>,
    reader_factory: Arc<ReaderFactory>,
    view: Arc<dyn View>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    /// #1486: the `view_instance(...)` scan-time audience check, run before `jit_update`.
    /// `Some` only when this provider was built by `ViewInstanceTableFunction` -- a
    /// caller-named instance. `None` for every server-constructed `MaterializedView`: the
    /// implicitly-registered global tables (`query.rs::register_table`) and `OwnershipRewrite`'s
    /// own `processes`/`streams` sources, which are Prong A's job to filter row-by-row and must
    /// never be denied wholesale.
    instance_guard: Option<Arc<AudienceGuard>>,
}
```

`scan` becomes:

```rust
async fn scan(&self, ...) -> Result<Arc<dyn ExecutionPlan>> {
    if let Some(guard) = &self.instance_guard {
        guard
            .authorize_view_instance(
                &self.view.get_view_set_name(),
                &self.view.get_view_instance_id(),
            )
            .await?;
    }
    self.view.jit_update(...).await?;   // unchanged, still CallerContext::internal() inside
    ...
}
```

`MaterializedView::new` takes the new field as a **required positional parameter**, not a
defaulted builder step — per `CLAUDE.md`'s "prefer the shape that makes the compiler enumerate
every affected call site". Five sites:

| site | value |
|---|---|
| `view_instance_table_function.rs:76` | `Some(guard)` |
| `query.rs:62` (`register_table`, global tables) | `None` |
| `query.rs:344` (`OwnershipRewrite` `processes_source`) | `None` |
| `query.rs:352` (`OwnershipRewrite` `streams_source`) | `None` |
| `tests/sql_batch_view_merge_ordering_tests.rs:400` | `None` |

### 4. Wiring in `query.rs::register_lakehouse_functions`

`audience_guard` is currently constructed at `query.rs:137`, *after* the `view_instance`
registration at `:121`. Move the `lakehouse_admin` / `AudienceGuard::new` block above the
`view_instance` registration and pass `audience_guard.clone()` into
`ViewInstanceTableFunction::new`. `ViewInstanceTableFunction` gains an
`Arc<AudienceGuard>` field it hands to every `MaterializedView` it builds. That block carries its
own doc comment enumerating the guard's consumers (`process_spans`, `perfetto_trace_chunks`,
`parse_block`, `get_payload`, `list_partitions`'s row filter); update it to add `view_instance` as
a sixth, since the move puts `view_instance`'s registration below the block while the comment
still says "registers below".

No new construction of `AudienceGuard` anywhere: the same per-request instance already shared by
the five other Prong B call sites now covers a sixth.

### 5. What does not change

- `View::jit_update`'s signature, and the `CallerContext::internal()` inside
  `find_stream_from_view` / `find_process_with_latest_timing` (`metadata.rs:181`, `:308`).
  #1371 §7's analysis is unchanged — those reads still produce no caller-visible rows. The guard
  runs one frame earlier, so a denied caller never reaches them.
- `OwnershipRewrite` (Prong A) — untouched. The provider type it downcasts to is the same type.
- Internal, maintenance, and `--disable-auth` sessions: `ReadScope::All` short-circuits with no
  I/O, exactly as for the other five Prong B sites.
- `process_spans`/`perfetto_trace_chunks`' inner sessions, which issue `view_instance(...)` SQL
  under `Authorized::internal_caller()` (`ReadScope::All`) — also a no-op, so no double check and
  no extra Postgres round trip on the flame-graph path.
- The admin-gated `materialize_partitions` / `regenerate_partitions` / `retire_partitions`, which
  can also name a `view_instance_id`. They remain deployment-wide and audience-blind, as
  `mkdocs/docs/admin/authentication.md` already documents.

### 6. Behaviour change for clients

A `ReadScope::Audiences` caller naming an instance outside its audiences previously got a
successful query returning **zero rows** (Prong A filtered them). It now gets a plan error:
`view_instance: '<id>' not found or not accessible` — the same not-found-shaped text every other
Prong B denial produces, and the same text a genuinely nonexistent id produces, so it is not an
existence oracle. Any UI or script that treated "unreadable instance" as "empty result" sees an
error instead. This is the intended shape (it matches `process_spans` and friends), and it needs
a CHANGELOG upgrade note.

Cases that are *not* affected: an id whose process/stream exists in the caller's audience but is
the wrong kind for the view set (e.g. a `process_id` given to `thread_spans`) is authorized by
`IdKind::ProcessOrStream` and then fails inside `jit_update` exactly as it does today.

An id whose Postgres row retention has deleted is a **mixed case**, split by which helper the view
set's `jit_update` calls:

- `log_entries` (`log_view.rs:159`), `measures` (`metrics_view.rs:159`), `images`
  (`images_view.rs:122`), `otel_spans` (`otel/spans_view.rs:125`) call `metadata::find_process`,
  which reads Postgres directly — these already fail today once retention deletes the row, and now
  fail one frame earlier with the uniform message. Not a behaviour change for these four.
- `thread_spans` (`find_stream_from_view` + `find_process_with_latest_timing`,
  `thread_spans_view.rs:358,367`), `net_spans` (`find_process_with_latest_timing`,
  `net_spans_view.rs:330`), and `async_events` (same, `async_events_view.rs:130`) resolve through
  `FROM streams` / `FROM processes` over `LivePartitionProvider` (`metadata.rs:211`, `:335`) —
  daemon-materialized lakehouse views that can outlive the deleted Postgres row. These three
  currently **succeed** for such an id (the process/stream data is still queryable from the
  lakehouse even though the Postgres row is gone); the guard resolves from Postgres only
  (`owner_query_sql`, `audience_guard.rs:141-190`), so such an id becomes `OwnerAudience::Unknown`
  and the query is denied — a real behaviour change, and the one this issue is about. This is the
  same skew `mkdocs/docs/admin/authentication.md:191-197` documents for Prong B generally.

**Client-risk survey (settled: low-risk, no client change needed).** Every `view_instance(...)`
call in `analytics-web-app/` substitutes a `$process_id` the user picked out of a process list
Prong A has *already* audience-filtered: `useMetricsData.ts:11`, `ProcessLogPage.tsx:23`,
`ImageCell.tsx:20`, `perf-analysis/queries.ts:11,18`, `notebook-utils.ts:150`, and
`ProcessMetricsPage.tsx:28,35` (a `view_instance('measures', '$process_id')` pair with the same
shape). None of these can normally reach an unreadable id, so none needs a code change. The
reachable case is a hand-typed id in a notebook cell, which today renders as an empty result and
will render as an error instead — already handled: cells render a query failure as an error state
carrying the server message (e.g. `HgChildPane.tsx:224-231`), so the uniform not-found text
surfaces legibly with no additional client work.

Two published client helpers outside the web app take the same "hand-typed id" shape, without a
Prong-A-filtered picker in front of them: `Client.query_spans(begin, end, limit, stream_id)`
(`python/micromegas/micromegas/flightsql/client.py:1050`, documented as returning a DataFrame, and
demonstrated the same way in `python/micromegas/README.md:63`) and
`fetch_spans_batch(client, stream_id, ...)` (`rust/public/src/client/frame_budget_reporting.rs:114,143,387`),
both of which pass a caller-supplied `stream_id` straight into
`view_instance('thread_spans', ...)`. Neither needs a code change either: both already surface a
server query failure as a returned `Err`/raised exception to their caller, which is exactly what
the uniform not-found text becomes. Named here because they are the only other reachable callers
of this shape; the CHANGELOG upgrade note calls them out explicitly (see §Documentation).

## Implementation Steps

### Phase 1 — the rule

1. `rust/analytics/src/lakehouse/audience_guard.rs`: extract the private `not_found_err` helper
   from `authorize`; add `pub fn is_public_view_set` and rewrite `global_rows_visible` in terms of
   it; add `pub async fn authorize_view_instance` with the five ordered rules and the doc comment
   recording §2's `'global'` rationale; update `IdKind::ProcessOrStream`'s doc comment, which
   today names `list_partitions` as its sole consumer, to add `view_instance`'s scan-time check as
   a second.
2. Update `audience_guard.rs`'s module doc comment: its entry-point list lives in the opening
   paragraph (lines 1-6, "arg-addressed guards ... that Prong A structurally cannot reach"), not
   in the "One cache, one question" / "No existence oracle" sections — leave those alone, they
   don't enumerate entry points. Reframe the opening paragraph rather than appending to it:
   `view_instance` joins Prong B to close the cost/availability residual this plan fixes, not
   because Prong A can't reach it — Prong A already filters `view_instance` scans row-by-row (see
   the "`MaterializedView` downcast constraint" section above). Apply the same reframing to
   `read_scope.rs`'s module doc comment, whose Stage 3 paragraph carries the identical "structurally
   cannot reach" list.

### Phase 2 — the call site

3. `rust/analytics/src/lakehouse/materialized_view.rs`: add the `instance_guard` field with its
   doc comment, extend `MaterializedView::new`, and call the guard at the top of `scan` before
   `jit_update`.
4. `rust/analytics/src/lakehouse/view_instance_table_function.rs`: add the `guard:
   Arc<AudienceGuard>` field, thread it into `MaterializedView::new`, and note in the struct's doc
   comment that this is the only site that supplies one.
5. `rust/analytics/src/lakehouse/query.rs`: move the `lakehouse_admin` / `AudienceGuard::new`
   block above the `view_instance` registration; pass the guard to `ViewInstanceTableFunction::new`;
   pass `None` at `register_table` (:62) and at both `OwnershipRewrite` sources (:344, :352); and
   update the moved block's doc comment enumerating its consumers to add `view_instance` as a
   sixth.
6. `rust/analytics/tests/sql_batch_view_merge_ordering_tests.rs:400`: pass `None`.
7. `rust/analytics/src/metadata.rs`: update the residual note in `find_stream_from_view`
   (`:197`) to say the `view_instance` entry point is now guarded.
   `find_process_with_latest_timing`'s comment (`:317-320`) defers to it ("see
   `find_stream_from_view`'s identical comment above") and needs no separate edit.
8. `rust/analytics/tests/ownership_rewrite_db_test.rs`: this suite `.collect()`s its queries, so the
   guard landing in this phase changes its behaviour — update the three cross-audience
   `view_instance('log_entries'/'async_events'/'thread_spans', ...)` assertions that currently
   expect `0` rows to instead expect the guard's `not found or not accessible` denial, leaving the
   same-audience and `ReadScope::All` assertions untouched.

### Phase 3 — tests

9. Offline unit tests in `rust/analytics/tests/audience_guard_tests.rs` (per §Testing Strategy).
10. DB-backed tests in `rust/analytics/tests/prong_b_guard_db_test.rs`.

### Phase 4 — docs and changelog

11. `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/query-guide/functions-reference.md`,
    `CHANGELOG.md` (per §Documentation).
12. Add a Stage-3 residual-closed note to `tasks/data_isolation/audience_based_access_control_plan.md`'s
    `### Stage 3 — Enforcement Prong B` section (it does not mention this residual today), and mark
    it closed in `tasks/completed/1371_udtf_udf_guards_plan.md` §7.

## Files to Modify

| file | change |
|---|---|
| `rust/analytics/src/lakehouse/audience_guard.rs` | `authorize_view_instance`, `is_public_view_set`, `not_found_err`; reframe opening-paragraph module doc; add `view_instance` as `IdKind::ProcessOrStream`'s second consumer in its doc comment |
| `rust/analytics/src/lakehouse/read_scope.rs` | reframe the Stage 3 paragraph's entry-point list in the module doc |
| `rust/analytics/src/lakehouse/materialized_view.rs` | `instance_guard` field; guard call in `scan` |
| `rust/analytics/src/lakehouse/view_instance_table_function.rs` | carry and pass the guard |
| `rust/analytics/src/lakehouse/query.rs` | reorder guard construction; 4 `MaterializedView::new` / registration sites; update the moved block's consumer-list comment to add `view_instance` |
| `rust/analytics/src/metadata.rs` | residual note on `find_stream_from_view` (`:197`); `find_process_with_latest_timing` defers to it, no separate edit |
| `rust/analytics/tests/audience_guard_tests.rs` | offline rule tests |
| `rust/analytics/tests/prong_b_guard_db_test.rs` | DB-backed enforcement + no-materialization tests |
| `rust/analytics/tests/sql_batch_view_merge_ordering_tests.rs` | `None` at the new parameter |
| `rust/analytics/tests/ownership_rewrite_db_test.rs` | convert the three cross-audience `view_instance(...)` `0`-row assertions (`log_entries`, `async_events`, `thread_spans`) into denial assertions |
| `mkdocs/docs/admin/authentication.md` | Prong B now covers five functions, plus the `'global'` rule |
| `mkdocs/docs/query-guide/functions-reference.md` | `view_instance` denial behaviour |
| `CHANGELOG.md` | Amend the `#1371` Stage 3 entry ("four surfaces" reframe) + new Unreleased → Analytics entry with the upgrade note |
| `tasks/data_isolation/audience_based_access_control_plan.md` | add a Stage-3 residual-closed note (not previously recorded there) |
| `tasks/completed/1371_udtf_udf_guards_plan.md` | mark the residual closed |

## Trade-offs

- **Guard field on `MaterializedView` vs. a wrapping `TableProvider`.** A wrapper is the more
  obvious open/closed shape and keeps authorization out of a provider that also serves the global
  tables. Rejected: `OwnershipRewrite::rewrite_plan` reaches Prong A through
  `downcast_ref::<MaterializedView>()` and silently skips anything else, so every wrapped
  `view_instance` scan would lose its audience predicate — trading a cost residual for a
  confidentiality hole. Teaching Prong A to unwrap is possible but puts a new required step in the
  one code path where a miss fails open; not worth it for one call site.
- **Guard field vs. threading a `CallerContext` through `View::jit_update`.** The parent plan
  (#1371 §7) already weighed and rejected this: ~10 impls changed to gate reads that produce no
  caller-visible rows. Unchanged here — and the guard is the stronger check anyway, since it reads
  Postgres rather than a daemon-materialized snapshot.
- **`Option<Arc<AudienceGuard>>` vs. always passing the guard and deciding inside.** Always
  passing would make `MaterializedView::scan` check the implicitly-registered global tables too,
  where `view_instance_id` is `'global'` and the only available rule (`global_rows_visible`) would
  deny `log_entries`/`measures` to every non-admin scoped caller. The `Option` is what marks
  "caller-named instance" vs. "server-constructed table", and only the former is this issue's
  attack surface.
- **`IdKind::ProcessOrStream` vs. a per-view-set id kind.** A precise kind (stream for
  `thread_spans`, process for the rest) would need a new accessor on the `View` trait for a
  strictly *weaker* result: `ProcessOrStream` is fail-closed on a `process_id`/`stream_id`
  collision (`OwnerAudience::Ambiguous` passes only when every arm passes), whereas a single-arm
  resolution would pick one. Reusing `list_partitions`' kind also shares its `AudienceIndex` cache
  entries.
- **Erroring vs. silently returning zero rows.** Silently returning zero rows would preserve
  today's client-visible behaviour, but the whole point is to refuse *before* `jit_update`, and
  there is no zero-row plan to return at that moment that isn't a lie about a real instance. The
  uniform not-found text matches every other Prong B denial.

## Security

- **What this closes:** compute and object-storage spend, plus materialization error text, that a
  caller scoped to audience A could previously trigger against an instance in audience B by naming
  it in `view_instance(...)`.
- **What it does not change:** confidentiality. Prong A already returned zero rows for a query
  naming an instance outside the caller's audiences, and still does. This is an availability/cost
  hardening — though for `thread_spans`/`net_spans`/`async_events` it also newly *denies* an id
  whose Postgres row retention has deleted but whose lakehouse data still exists (see §6), trading
  that availability for the uniform fail-closed rule.
- **Fail-closed:** every non-`All` scope denies on a resolution error (`AudienceGuard::authorize`
  maps a Postgres failure to a query error, never to a pass), on `OwnerAudience::Unknown`, on
  `Ambiguous` unless every arm is readable, and on an id matching neither `'global'` nor `Uuid`.
- **No existence oracle:** denial and nonexistence produce identical text; the real reason is
  `debug!`-logged for operators only.
- **Not closed by this change:** the admin-gated `materialize_partitions`/`regenerate_partitions`
  can still materialize any instance in any audience. That is the documented, deployment-wide
  admin gate, out of scope here.
- **Not closed by this change:** rule (2)'s `public_view_sets` exemption is a confidentiality
  argument only. A view set an operator has opted into `MICROMEGAS_PUBLIC_VIEW_SETS` is exempt from
  this guard entirely, so any authenticated caller can still force `jit_update` — real object-store
  writes — for arbitrary process/stream ids of that view set. Left open deliberately: denying
  materialization of a view set every row of which is already readable would be incoherent, but
  the cost/availability residual for that view set is not addressed by this plan.

## Testing Strategy

Offline (`rust/analytics/tests/audience_guard_tests.rs`, no DB — reuses the existing
`unroutable_index()` helper, so any test that passes proves no I/O happened):

- `ReadScope::All` + an arbitrary uuid instance → `Ok`, no I/O.
- `ReadScope::Audiences` + a view set on `public_view_sets` → `Ok`, no I/O.
- `ReadScope::Audiences` + `'global'` → `Ok`, no I/O, for both an admin and a non-admin guard and
  a view set *not* on the allowlist (this is the rule that deliberately differs from
  `global_rows_visible`; pin it).
- `ReadScope::Audiences` + a non-public view set + a real `Uuid` over `unroutable_index()` → `Err`,
  never `Ok` — mirrors `authorize_under_restricted_scope_denies_on_resolution_error_not_pass`
  (`audience_guard_tests.rs:189`), pinning that rule (4)'s fall-through to `authorize` really
  attempts resolution and fails closed rather than short-circuiting to a pass. This is the only
  offline coverage of the enforcement rule itself, since the DB-backed tests in
  `prong_b_guard_db_test.rs` are `#[ignore]`d and not run by CI.
- `ReadScope::Audiences` + a non-uuid, non-`'global'` id → denied with the uniform text.
- `global_rows_visible` keeps its existing truth table after the `is_public_view_set` extraction.

DB-backed (`rust/analytics/tests/prong_b_guard_db_test.rs`, `#[ignore]`, live
`MICROMEGAS_SQL_CONNECTION_STRING`/`MICROMEGAS_OBJECT_STORE_URI`) — `setup()` already seeds
process A (`team-a`), B (`team-b`) and C (never stamped → `public`) with a cpu stream and a block
each:

- `view_instance_guard_enforces_audience`: as a `team-a` caller,
  `SELECT * FROM view_instance('thread_spans', '<B's stream_id>')` fails with
  `not found or not accessible`; the same query for A's own stream succeeds. Repeat for a
  process-scoped set (`async_events`, `<B's process_id>`) to cover both resolution arms.
- `view_instance_guard_prevents_jit_materialization` — the test that actually proves the issue is
  fixed. Assert `list_partitions()` (queried under `ReadScope::All`) has **no** row whose
  `view_instance_id` is B's stream id *after* the denied `team-a` query, then run the same query
  as a `team-b` caller and assert a row now exists. Without the fix the first assertion fails.
- `view_instance_global_stays_readable_for_scoped_callers`: as a `team-a` caller,
  `view_instance('log_entries', 'global')` succeeds (no denial, no error) — the §2 rule that
  `'global'` is passed through uncalled and left to Prong A. The fixture seeds no log entries, so
  this does not (and does not claim to) pin the row-level equivalence with
  `SELECT * FROM log_entries`; that equivalence is Prong A's existing row-filter behaviour, already
  covered by the `ownership_rewrite_*` suites this plan also runs.
- `view_instance_unaffected_for_read_scope_all`: the existing
  `list_partitions_row_filter_enforces_audience` test already materializes both processes'
  `thread_spans` instances under `ReadScope::All`; it must keep passing unchanged, which is the
  regression check that internal/maintenance callers are untouched.

Plus: `cargo test -p micromegas-analytics`, `cargo clippy --workspace --all-targets`, and the
existing `ownership_rewrite_*` suites (the Prong A downcast must still find its provider).
`ownership_rewrite_public_view_set_tests.rs` passes unchanged — it only builds optimized logical
plans via `optimized_plan`/`optimized_plan_with_factory` and never `.collect()`s, so
`TableProvider::scan` (and the guard inside it) is never invoked. `ownership_rewrite_db_test.rs`
does `.collect()` and today asserts `0` rows for three cross-audience `view_instance(...)` queries
naming process A's ids under a `team-b` scope (`log_entries` ~ln 388-414, `async_events` ~ln
428-457, `thread_spans` ~ln 474-516); this plan's guard turns each of those into a denial error
before `jit_update` runs, so those three assertions must be updated (Phase 2, alongside the guard
landing) to expect the `not found or not accessible` denial instead of a `0` row count — the
same-audience and `ReadScope::All` assertions in that file are unaffected and stay as-is.

## Documentation

- `mkdocs/docs/admin/authentication.md`, "Audience Filtering Activation": Prong B now covers
  **five** functions — add `view_instance` to the `process_spans`/`perfetto_trace_chunks`/
  `parse_block`/`get_payload` list, but reframe rather than just append: the sentence currently
  reads "Prong B ... covers the four functions Prong A structurally can't reach", which would be
  self-contradictory with `view_instance` appended, since Prong A does reach and filter
  `view_instance` scans (§Design, "The `MaterializedView` downcast constraint"). Reword to "Prong
  B covers five arg-addressed functions; `view_instance` joins them to close a cost/availability
  residual, not because Prong A can't reach it." State that a scoped caller naming an instance
  outside its audiences now gets a not-found-shaped error *instead of an empty result*, and record
  that `'global'` instances are exempt (no JIT to trigger, Prong A filters their rows) — explicitly
  contrasted with `list_partitions`' different `'global'`-row rule, which is described a few
  paragraphs above and would otherwise read as contradictory. Also record that a view set on
  `MICROMEGAS_PUBLIC_VIEW_SETS` is exempt from this guard entirely: any authenticated caller can
  still trigger JIT materialization of any of its instances, since denying materialization of a
  view set every row of which is already readable would be incoherent.
- `mkdocs/docs/query-guide/functions-reference.md`, `view_instance(view_name, identifier)`
  (line 13): a note that on an authenticated deployment the function errors for an identifier
  outside the caller's audiences.
- `CHANGELOG.md`: the `#1371` Stage 3 entry (still `## Unreleased`) opens with "arg-addressed
  audience guards for the four span/metadata surfaces `OwnershipRewrite` (Prong A, above)
  structurally cannot reach" — the same self-contradiction as the mkdocs/module-doc sentences
  above once `view_instance` joins the list. Add an **Amended (#1486, still `## Unreleased`)**
  clause to that existing entry, following the `#1482` clauses already there, reframing it to
  "five arg-addressed functions, `view_instance` joining to close a cost/availability residual"
  rather than appending `view_instance` to the "four ... cannot reach" list.
- `CHANGELOG.md`, Unreleased → **Analytics**: a new entry with the fix, the entry-point-only scope, the `'global'`
  exemption and why it differs from `list_partitions`, the client-visible
  zero-rows→error change as an upgrade note — explicitly calling out
  `micromegas.flightsql.Client.query_spans` (Python) and `fetch_spans_batch` (Rust,
  `rust/public/src/client/frame_budget_reporting.rs`) as the two published client helpers that
  surface it as an exception — and a note that for `thread_spans`/`net_spans`/`async_events` a
  `view_instance_id` whose Postgres row retention has already deleted now fails instead of
  succeeding off the materialized lakehouse view, and a **Minor breaking change** clause for
  `MaterializedView::new` and `ViewInstanceTableFunction::new` gaining a required parameter
  (both published from `micromegas_analytics::lakehouse`).
- Mark the residual closed where it was recorded: `tasks/completed/1371_udtf_udf_guards_plan.md`
  §7. Add a fresh residual-closed note to `tasks/data_isolation/audience_based_access_control_plan.md`'s
  `### Stage 3 — Enforcement Prong B` section, which does not mention this residual today.
