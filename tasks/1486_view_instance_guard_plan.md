# `view_instance` Scan-Time Audience Guard Plan (#1486, AbAC Stage 3 residual)

## Overview

`MaterializedView::scan` runs `View::jit_update` before anything filters the caller's read
(`rust/analytics/src/lakehouse/materialized_view.rs:70`), so a `ReadScope::Audiences` caller who
names a view instance belonging to another audience can make the server materialize partitions
for data it will then return zero rows of. This is the availability/cost residual accepted in
AbAC Stage 3 (#1371 §7) and recorded in `tasks/completed/1371_udtf_udf_guards_plan.md` and
`tasks/data_isolation/audience_based_access_control_plan.md`.

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
   its instances would be incoherent.
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

### 2. `'global'` is allowed, deliberately — and it is not `list_partitions`' rule

The issue asks what `'global'` instances should do. **Allow them, with no audience check.** Three
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
`Arc<AudienceGuard>` field it hands to every `MaterializedView` it builds.

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
`IdKind::ProcessOrStream` and then fails inside `jit_update` exactly as it does today. An id whose
Postgres row retention has deleted already fails today inside `find_process`; it now fails one
frame earlier with the uniform message.

## Implementation Steps

### Phase 1 — the rule

1. `rust/analytics/src/lakehouse/audience_guard.rs`: extract the private `not_found_err` helper
   from `authorize`; add `pub fn is_public_view_set` and rewrite `global_rows_visible` in terms of
   it; add `pub async fn authorize_view_instance` with the five ordered rules and the doc comment
   recording §2's `'global'` rationale.
2. Update the module doc comment's "One cache, one question" / "No existence oracle" sections to
   list `view_instance` alongside the other guarded entry points.

### Phase 2 — the call site

3. `rust/analytics/src/lakehouse/materialized_view.rs`: add the `instance_guard` field with its
   doc comment, extend `MaterializedView::new`, and call the guard at the top of `scan` before
   `jit_update`.
4. `rust/analytics/src/lakehouse/view_instance_table_function.rs`: add the `guard:
   Arc<AudienceGuard>` field, thread it into `MaterializedView::new`, and note in the struct's doc
   comment that this is the only site that supplies one.
5. `rust/analytics/src/lakehouse/query.rs`: move the `lakehouse_admin` / `AudienceGuard::new`
   block above the `view_instance` registration; pass the guard to `ViewInstanceTableFunction::new`;
   pass `None` at `register_table` (:62) and at both `OwnershipRewrite` sources (:344, :352).
6. `rust/analytics/tests/sql_batch_view_merge_ordering_tests.rs:400`: pass `None`.
7. `rust/analytics/src/lakehouse/materialized_view.rs` + `metadata.rs`: update the doc comments
   that describe the residual as open (`metadata.rs`'s notes on the two `CallerContext::internal()`
   lookups) to say the entry point is now guarded.

### Phase 3 — tests

8. Offline unit tests in `rust/analytics/tests/audience_guard_tests.rs` (per §Testing Strategy).
9. DB-backed tests in `rust/analytics/tests/prong_b_guard_db_test.rs`.

### Phase 4 — docs and changelog

10. `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/query-guide/functions-reference.md`,
    `CHANGELOG.md` (per §Documentation).
11. Mark the residual closed in `tasks/data_isolation/audience_based_access_control_plan.md`
    (the `#1486` residual note) and in
    `tasks/completed/1371_udtf_udf_guards_plan.md` §7.

## Files to Modify

| file | change |
|---|---|
| `rust/analytics/src/lakehouse/audience_guard.rs` | `authorize_view_instance`, `is_public_view_set`, `not_found_err`; module doc |
| `rust/analytics/src/lakehouse/materialized_view.rs` | `instance_guard` field; guard call in `scan` |
| `rust/analytics/src/lakehouse/view_instance_table_function.rs` | carry and pass the guard |
| `rust/analytics/src/lakehouse/query.rs` | reorder guard construction; 4 `MaterializedView::new` / registration sites |
| `rust/analytics/src/metadata.rs` | doc comments on the two internal lookups |
| `rust/analytics/tests/audience_guard_tests.rs` | offline rule tests |
| `rust/analytics/tests/prong_b_guard_db_test.rs` | DB-backed enforcement + no-materialization tests |
| `rust/analytics/tests/sql_batch_view_merge_ordering_tests.rs` | `None` at the new parameter |
| `mkdocs/docs/admin/authentication.md` | Prong B now covers five functions, plus the `'global'` rule |
| `mkdocs/docs/query-guide/functions-reference.md` | `view_instance` denial behaviour |
| `CHANGELOG.md` | Unreleased → Analytics entry with the upgrade note |
| `tasks/data_isolation/audience_based_access_control_plan.md`, `tasks/completed/1371_udtf_udf_guards_plan.md` | mark the residual closed |

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
- **What it does not change:** confidentiality. Prong A already returned zero rows for such a
  query, and still does. This is an availability/cost hardening.
- **Fail-closed:** every non-`All` scope denies on a resolution error (`AudienceGuard::authorize`
  maps a Postgres failure to a query error, never to a pass), on `OwnerAudience::Unknown`, on
  `Ambiguous` unless every arm is readable, and on an id matching neither `'global'` nor `Uuid`.
- **No existence oracle:** denial and nonexistence produce identical text; the real reason is
  `debug!`-logged for operators only.
- **Not closed by this change:** the admin-gated `materialize_partitions`/`regenerate_partitions`
  can still materialize any instance in any audience. That is the documented, deployment-wide
  admin gate, out of scope here.

## Testing Strategy

Offline (`rust/analytics/tests/audience_guard_tests.rs`, no DB — reuses the existing
`unroutable_index()` helper, so any test that passes proves no I/O happened):

- `ReadScope::All` + an arbitrary uuid instance → `Ok`, no I/O.
- `ReadScope::Audiences` + a view set on `public_view_sets` → `Ok`, no I/O.
- `ReadScope::Audiences` + `'global'` → `Ok`, no I/O, for both an admin and a non-admin guard and
  a view set *not* on the allowlist (this is the rule that deliberately differs from
  `global_rows_visible`; pin it).
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
  `view_instance('log_entries', 'global')` succeeds and returns exactly the rows
  `SELECT * FROM log_entries` returns for that caller (A's, not B's) — the §2 rule, pinned
  end-to-end against the named-table equivalence it is justified by.
- `view_instance_unaffected_for_read_scope_all`: the existing
  `list_partitions_row_filter_enforces_audience` test already materializes both processes'
  `thread_spans` instances under `ReadScope::All`; it must keep passing unchanged, which is the
  regression check that internal/maintenance callers are untouched.

Plus: `cargo test -p micromegas-analytics`, `cargo clippy --workspace --all-targets`, and the
existing `ownership_rewrite_*` suites (the Prong A downcast must still find its provider —
`ownership_rewrite_public_view_set_tests.rs` and `ownership_rewrite_db_test.rs` are the ones that
would catch a regression there).

## Documentation

- `mkdocs/docs/admin/authentication.md`, "Audience Filtering Activation": Prong B now covers
  **five** functions — add `view_instance` to the `process_spans`/`perfetto_trace_chunks`/
  `parse_block`/`get_payload` list, state that a scoped caller naming an instance outside its
  audiences now gets a not-found-shaped error *instead of an empty result*, and record that
  `'global'` instances are exempt (no JIT to trigger, Prong A filters their rows) — explicitly
  contrasted with `list_partitions`' different `'global'`-row rule, which is described a few
  paragraphs above and would otherwise read as contradictory.
- `mkdocs/docs/query-guide/functions-reference.md`, `view_instance(view_name, identifier)`
  (line 13): a note that on an authenticated deployment the function errors for an identifier
  outside the caller's audiences.
- `CHANGELOG.md`, Unreleased → **Analytics**: the fix, the entry-point-only scope, the `'global'`
  exemption and why it differs from `list_partitions`, the client-visible
  zero-rows→error change as an upgrade note, and a **Minor breaking change** clause for
  `MaterializedView::new` and `ViewInstanceTableFunction::new` gaining a required parameter
  (both published from `micromegas_analytics::lakehouse`).
- Mark the residual closed where it was recorded: `tasks/completed/1371_udtf_udf_guards_plan.md`
  §7 and `tasks/data_isolation/audience_based_access_control_plan.md`.

## Open Questions

1. **`'global'` exemption** — the plan settles this as "allow, unconditionally" (§2). It is the
   one place where a reviewer might prefer `global_rows_visible` for symmetry with
   `list_partitions`; the counter-argument is that it would break `view_instance('log_entries',
   'global')` for every non-admin scoped caller while protecting nothing. Worth an explicit
   confirmation before implementing, since it is the issue's own stated open point.
2. **Erroring vs. empty result** — §6's behaviour change is client-visible, but the survey of
   current consumers says it is low-risk: every `view_instance(...)` call in `analytics-web-app/`
   (`useMetricsData.ts:11`, `ProcessLogPage.tsx:23`, `ImageCell.tsx:20`,
   `perf-analysis/queries.ts:11,18`, `notebook-utils.ts:150`) substitutes a `$process_id` the user
   picked out of a process list that Prong A has *already* audience-filtered, so a scoped caller
   cannot normally reach an unreadable id. The reachable case is a hand-typed id in a notebook
   cell, which today renders as an empty result and would render as an error. No code change is
   needed on the client; confirm the error surfaces legibly in a notebook cell during
   implementation.
