# Apply the Declared Sort to the Extract Query

## Overview

A view declaring `with_merge_sort_order` must today also carry a matching top-level `ORDER BY` in its
extract query: `SqlPartitionSpec::execute_extract_query` builds the physical plan and *asserts* that
its output ordering satisfies the declared columns, erroring with "Check for a missing or mismatched
top-level ORDER BY" when it does not. The merge side of the same option works the other way around —
`QueryMerger::execute_sorted_merge` applies the declared columns as a logical-plan `DataFrame::sort`,
so a missing or mismatched merge sort is not representable at all.

This makes the extract side match the merge side: **apply** the declared sort, don't demand it.
After this change neither query carries an author-written `ORDER BY`, and the ordering guarantee is
enforced by construction on both sides rather than by construction on one and by blame on the other.

It is also a prerequisite for a future DDL-defined-view-sets plan, where `merge_sort_order` would be
a DDL option: a sort order arriving as configuration rather than as code has no author who can read a
"missing or mismatched top-level ORDER BY" error and fix it, so an applied sort is what makes that
option safe. This lands first and on its own.

## Current State

- `rust/analytics/src/lakehouse/sql_partition_spec.rs:72-115` — `execute_extract_query` returns
  `df.execute_stream()` directly when `sort_order` is `None`; when it is declared, it builds the
  physical plan and runs `assert_single_partition` + `assert_ordering_satisfied` against it, then
  executes that exact plan via `execute_stream`.
- `sql_partition_spec.rs:40-44` — `SqlPartitionSpec::sort_order`'s field doc states the verify
  semantics ("When set, `write` verifies the extract query's physical plan actually satisfies it").
- `rust/analytics/src/lakehouse/merge.rs:227-270` — `execute_sorted_merge`'s apply-then-plan
  sequence to mirror: `df.sort(...)` over the `ScanSortColumn`s, `df.task_ctx()`,
  `create_physical_plan()`, then the same two assertions. The four optimizer settings at
  `merge.rs:216-224` (`enable_round_robin_repartition`, `repartition_aggregations`,
  `prefer_existing_sort`, `repartition_joins`) are merge-specific, not part of this pattern.
- `rust/analytics/src/lakehouse/sql_batch_view.rs:145-163` — `with_merge_sort_order`'s doc comment
  states a four-item author contract whose item 3 is the top-level-`ORDER BY` requirement.
- `rust/analytics/src/lakehouse/log_stats_view.rs:50` — the only shipped view declaring a sort order
  carries `ORDER BY time_bin, process_id, level, target`, with comments at `:29` and `:35`
  explaining that the `ORDER BY` is what lets the fresh-write path record the guarantee.
- Three test files pin the current contract: `sql_partition_spec_sort_order_tests.rs` (two tests and
  a module header stating the verify semantics verbatim), `log_stats_ordering_tests.rs`
  (`log_stats_extract_query_satisfies_its_declared_sort_order` and its doc comment), and
  `ordered_aggregation_spike_tests.rs` (two rationale comments citing the verify contract, though
  their assertions are unaffected).

## Design

### 1. One shared sort-apply-and-plan helper: `plan_sorted_extract`

`sql_partition_spec.rs` grows a `pub fn plan_sorted_extract` that takes the `DataFrame`, the
declared sort order as `sort_order: Option<&[ScanSortColumn]>`, and the context the assertions need
(the subject string and the insert range for their error messages). When a sort order is declared it
applies it as a `DataFrame::sort` over the `ScanSortColumn`s, builds the physical plan, and runs the
existing `assert_single_partition` and `assert_ordering_satisfied` checks; with `sort_order: None`
it builds the plan unsorted and skips both. It returns the plan (and the `TaskContext` the caller
needs to execute it).

`execute_extract_query` keeps today's `Vec<String>` → `ScanSortColumn` conversion (`:82-88`) at its
own call site, then calls `plan_sorted_extract` with the converted slice and `execute_stream`-s
what it returns.

### 2. The author contract loses an item

`with_merge_sort_order`'s item 3 (the top-level-`ORDER BY` requirement) is dropped from the numbered
contract, and items 1 (declared columns must be `GROUP BY` keys), 2 (join side placement) and 4
(composable aggregates) are renumbered 1–3 — they are unaffected, being properties of the merge
query's shape, not of a sort the caller writes. The `:148` lead-in changes from "four-item" to
"three-item". A prose sentence after the list replaces item 3: neither query needs an author-written
`ORDER BY` any more, since the builder forwards the columns to both sides, `QueryMerger` applies the
sort to the merge query and the extract path applies it to the extract query.

### 3. `log_stats` drops its `ORDER BY`

With the sort applied unconditionally, `log_stats`'s extract-query `ORDER BY` is redundant. Removing
it is projection-identical — same columns, same types, same nullability, same order — so the view's
inferred Arrow schema and therefore its `file_schema_hash` are unchanged, and every existing
`log_stats` partition stays readable with no rebuild.

## Implementation Steps

1. `rust/analytics/src/lakehouse/sql_partition_spec.rs` — add the `pub` sort-apply-and-plan helper
   `plan_sorted_extract` described in §1, mirroring `merge.rs:227-270`; reduce
   `execute_extract_query` to calling it; rewrite `SqlPartitionSpec::sort_order`'s field doc
   (`:40-44`) and `execute_extract_query`'s own doc comment (`:73-77`), both of which describe the
   removed verify semantics, to describe the sort-applying path; rewrite the
   `assert_ordering_satisfied` `reason` string (`:109-112`), which still tells the reader to check
   for a missing top-level `ORDER BY`, to describe the plan-shape regression it now guards; the
   `assert_single_partition` `reason` string (`:96-101`) stays accurate and needs no change.
2. `rust/analytics/src/lakehouse/sql_batch_view.rs` — rewrite `with_merge_sort_order`'s doc comment
   (`:145-163`) per §2: drop item 3, renumber items 1/2/4 as 1–3, change the `:148` lead-in to
   "three-item", and add the "neither query needs an author-written `ORDER BY`" statement as prose
   after the list.
3. `rust/analytics/src/lakehouse/log_stats_view.rs` — drop `ORDER BY time_bin, process_id, level,
   target` (`:50`) and reword the two comments that explain it (`:29`, `:35`).
4. `rust/analytics/tests/sql_partition_spec_sort_order_tests.rs` — the file's two tests,
   `extract_query_missing_order_by_fails_the_write` (`:83-115`) and
   `extract_query_matching_order_by_passes_the_ordering_check` (`:120-178`), assert the same
   ordering property once the sort is applied unconditionally, and the latter already builds its own
   plan (`ctx.sql(extract_query).create_physical_plan()`) rather than going through the production
   path while still carrying the fixture's now-redundant `ORDER BY name, time_bin`. Fold them into
   one helper-driven test (named for the sort-applying path, e.g.
   `extract_query_without_an_order_by_satisfies_the_declared_sort_order`). Drop `make_test_view` and
   the `SqlBatchView` fixture it builds entirely: neither surviving assertion needs a view, only a
   session context and the declared columns. Build the `DataFrame` the same way the passing test
   already does — `make_session_context(...)` then `ctx.sql(...)` — over the out-of-order `VALUES`
   fixture (today's `:90-92`), and construct `declared_columns` as a literal `[ScanSortColumn; 2]`
   the way `:151-154` already does, rather than reading it off a view. Call `plan_sorted_extract`
   rather than re-applying the sort itself or reaching for `write()`'s `expect_err` (which the
   fixture's `connect_lazy` pool cannot reach once the ordering assertion stops firing). Because the
   helper itself already returns `Err` when the plan isn't single-partition and ordering-satisfying,
   re-checking `ordering_satisfy` on the plan it returns would only prove the helper returned `Ok`;
   instead, execute that plan (`execute_stream` with the helper's returned `TaskContext`), collect
   the resulting batches, and assert the emitted rows actually come out in `(name, time_bin)` order
   — a check the helper's internal assertion does not make — replacing today's plan-shape assertion
   and its failure message (`:165`). Delete the trailing "Sanity-check the view itself still
   declares that sort_order" block (`:168-178`): it never exercised the extract-query behavior under
   test (`SqlBatchView::make_batch_partition_spec` only runs the count query; `SqlPartitionSpec`
   exposes no accessor for `sort_order` to check against), and once the view fixture is gone there
   is nothing left for it to sanity-check. `with_merge_sort_order` itself stays covered by
   `sql_batch_view_merge_ordering_tests.rs`. Rewrite the module header (`:1-12`), which states the
   removed "refuses to record a false sort_order guarantee ... e.g. a missing top-level `ORDER BY`"
   contract verbatim, to describe the sort-applying path — without citing this plan document. Update
   the import list: `SqlBatchView`, `PartitionCache`, `TracingLogger`, `make_lex_ordering`, and the
   `View` trait become unused once the view fixture and the old plan-shape assertion are gone, so
   drop them; add `datafusion::physical_plan::execute_stream` (to run the plan the helper returns) —
   new for this call site, since `sql_batch_view_merge_ordering_tests.rs` gets its stream from
   `MergeQueryResult` rather than executing a plan itself — plus `futures::TryStreamExt` (for
   `try_collect`) and `datafusion::arrow::array::{Array, RecordBatch, StringArray}` (to read the
   `name` column back and check row order), the same two imports
   `sql_batch_view_merge_ordering_tests.rs` uses for its equivalent check.
5. `rust/analytics/tests/log_stats_ordering_tests.rs` — rename
   `log_stats_extract_query_satisfies_its_declared_sort_order` (`:173`) to
   `log_stats_extract_query_still_plans_through_the_sort_applying_helper` and update it to plan
   through `plan_sorted_extract` rather than the raw SQL text, passing it the `declared_columns`
   read off the shipped view via `view.get_scan_output_ordering()` (`match`ing out the
   `ScanOrdering::PerFile` columns, `_ => panic!`) instead of re-declaring `(time_bin, process_id,
   level, target)` as a literal. Since the helper itself already returns `Err` when the plan isn't
   single-partition and ordering-satisfying, replace the `partition_count` and `ordering_satisfied`
   assertions (`:219-242`) with a plain `expect()` on the helper's `Ok` — re-checking those same
   properties here would only prove the helper returned `Ok`. Reword the doc comment (`:164-171`,
   pinning "`ORDER BY time_bin, process_id, level, target`") and the module header's `ORDER BY` half
   (`:4`, `:10`) to state what the renamed test still pins — that the shipped `log_stats` extract
   query still plans and sorts cleanly through the helper — rather than that the query satisfies the
   declared order; the dropped assertion message (`:241`, "check for a missing or reordered
   top-level ORDER BY") goes with the assertion it belonged to. Drop `make_lex_ordering` and
   `ScanSortColumn` from the import list once the `ordering_satisfied` assertion and the literal
   `declared_columns` they supported are gone; add `partitioned_execution_plan::ScanOrdering` for
   the new `match` on `view.get_scan_output_ordering()`.
6. `rust/analytics/tests/ordered_aggregation_spike_tests.rs` — delete
   `cte_internal_order_by_is_discarded_by_a_later_join` (`:411-439`): it existed solely to justify the
   extract query's now-removed top-level-`ORDER BY` requirement, and the fact it pinned (a join
   discards its input's ordering) is already covered by
   `enrichment_join_with_the_ordered_side_on_the_build_side_reinstates_a_blocking_sort` (`:479`).
   Reword the rationale comment in `top_level_order_by_satisfies_the_declared_columns` (`:442`) that
   cites the removed contract ("plan verification relies on to accept a fresh extract query"); the
   assertion itself is about DataFusion's own plan behavior and keeps passing unchanged.
7. `CHANGELOG.md` — one entry describing that `with_merge_sort_order` no longer requires a top-level
   `ORDER BY` in the extract query (the sort is now applied for every declared-sort view); existing
   views that still carry an `ORDER BY` are unaffected.
8. `mkdocs/docs/admin/functions-reference.md:148` — the `regenerate_partitions` "Alignment
   invariant" warning says the blocking sort over an already-merged bucket comes from "the extract
   query's `ORDER BY` (required by `with_merge_sort_order`)"; reword it to attribute the sort to
   the sort order that `with_merge_sort_order` now applies to the extract query, since no
   author-written `ORDER BY` is required any more. `mkdocs/docs/query-guide/python-api.md:640`
   carries the same note ("its extract query's **required** `ORDER BY` then sorts that whole
   bucket's aggregated output in a single blocking pass"); reword it the same way.
9. `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` — once step 1 removes
   `SqlPartitionSpec::execute_extract_query`'s own call site for `assert_single_partition` and
   `assert_ordering_satisfied`, in favor of `plan_sorted_extract`, update both functions' rustdoc
   caller lists, which still name it: `assert_single_partition`'s (`:209-216`, "Shared by the
   query-execution paths that must verify this before executing: ... and
   `SqlPartitionSpec::execute_extract_query`") and `assert_ordering_satisfied`'s (`:235-244`,
   "Shared by the two paths that record a `sort_order` guarantee:
   `QueryMerger::execute_sorted_merge` and `SqlPartitionSpec::execute_extract_query`"). In both,
   replace `SqlPartitionSpec::execute_extract_query` with
   `sql_partition_spec::plan_sorted_extract` as the caller.

## Files to Modify

Modified:
- `rust/analytics/src/lakehouse/sql_partition_spec.rs`, `sql_batch_view.rs`, `log_stats_view.rs`,
  `partitioned_execution_plan.rs`
- `rust/analytics/tests/sql_partition_spec_sort_order_tests.rs`, `log_stats_ordering_tests.rs`,
  `ordered_aggregation_spike_tests.rs`
- `CHANGELOG.md`
- `mkdocs/docs/admin/functions-reference.md`, `mkdocs/docs/query-guide/python-api.md`

No new files, no migration, no SQL-surface change.

## Decisions

- Applying the declared sort and building the physical plan lives in one `pub` helper
  (`plan_sorted_extract`) rather than being re-implemented by each caller, so a validated plan and
  the daemon's plan cannot diverge.
- Accepted cost: the applied sort adds a logical-plan `Sort` node the assertion-only contract did
  not.
- `assert_single_partition` and `assert_ordering_satisfied` are kept on the extract path even though
  no author mistake can trip them any more; they become the regression guard against a plan shape
  that silently invalidates a recorded sort guarantee.
- `log_stats`'s `ORDER BY` is removed in this change rather than left as dead SQL, since the shipped
  view is the in-repo proof that the sort-applying path works.
- `execute_sorted_merge` keeps its own full apply-then-plan sequence rather than delegating to the
  shared helper: its warn-only check for a surviving `SortExec` (`merge.rs:272-286`) and the
  `MergeQueryResult.ordering_honored` value it returns have no extract-path counterpart, since the
  extract path builds its context with `make_session_context` (`query.rs:278`), not
  `make_merge_session_context`.

## Testing Strategy

All no-DB unit tests. `sql_partition_spec_sort_order_tests.rs` and `log_stats_ordering_tests.rs` use
the offline lakehouse harness (lazy pool, in-memory object store, `NullPartitionProvider`);
`ordered_aggregation_spike_tests.rs` is a planning-only DataFusion harness with no database or
object store access, and step 6 only deletes a test from it.

**`log_stats_ordering_tests.rs`** — together with the existing
`log_stats_merge_query_stays_a_streaming_kway_merge`,
`log_stats_extract_query_still_plans_through_the_sort_applying_helper` keeps both halves of the
streaming contract pinned for the one shipped declared-sort view.

No new `#[ignore]` live-DB test: per `CONTRIBUTING.md` those are reserved for pinning a bug witnessed
in the wild.

## Manual Verification

Materialize a `log_stats` range on a local stack (`python3 local_test_env/ai_scripts/start_services.py`,
then `materialize_partitions`) and confirm the written partitions still record their sort guarantee
and that a `log_stats` query over the range returns rows — the end-to-end check that the applied sort
produces the same materialization as the author-written `ORDER BY` did, which no planning-only test
covers.
