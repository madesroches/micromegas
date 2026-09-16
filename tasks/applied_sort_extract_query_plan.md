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

It is also a prerequisite for DDL-defined view sets (`tasks/835_ddl_materialized_views_plan.md`),
where `merge_sort_order` is a DDL option and a definition author has no way to see a missing
`ORDER BY` until the daemon's first materialization tick fails. That plan depends on the
`pub` helper this one introduces, so this lands first and on its own.

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
  `prefer_existing_sort`, `repartition_joins`) are merge-specific, not part of this pattern; the
  real reason `execute_sorted_merge` keeps its own full sequence rather than delegating to the
  shared helper is what follows the two assertions: a warn-only check for a surviving `SortExec`
  (`:272-286`) and the `MergeQueryResult.ordering_honored` value it returns, neither of which the
  extract path needs since it builds its context with `make_session_context` (`query.rs:278`),
  not `make_merge_session_context`.
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

### 1. One shared sort-apply-and-plan helper

`sql_partition_spec.rs` grows a `pub` helper that takes the `DataFrame`, the optional declared
`sort_order`, and the context the assertions need (the subject string and the insert range for their
error messages). When a sort order is declared it applies it as a `DataFrame::sort` over the
`ScanSortColumn`s, builds the physical plan, and runs the existing `assert_single_partition` and
`assert_ordering_satisfied` checks; with `sort_order: None` it builds the plan unsorted and skips
both. It returns the plan (and the `TaskContext` the caller needs to execute it).

`execute_extract_query` is reduced to calling the helper and `execute_stream`-ing what it returns.

### 2. The author contract loses an item

`with_merge_sort_order`'s item 3 becomes a statement that neither query needs an author-written
`ORDER BY`: the builder forwards the columns to both sides, `QueryMerger` applies the sort to the
merge query and the extract path applies it to the extract query. Items 1 (declared columns must be
`GROUP BY` keys), 2 (join side placement) and 4 (composable aggregates) are unaffected — they are
properties of the merge query's shape, not of a sort the caller writes.

### 3. `log_stats` drops its `ORDER BY`

With the sort applied unconditionally, `log_stats`'s extract-query `ORDER BY` is redundant. Removing
it is projection-identical — same columns, same types, same nullability, same order — so the view's
inferred Arrow schema and therefore its `file_schema_hash` are unchanged, and every existing
`log_stats` partition stays readable with no rebuild.

## Implementation Steps

1. `rust/analytics/src/lakehouse/sql_partition_spec.rs` — add the `pub` sort-apply-and-plan
   helper described in §1, mirroring `merge.rs:227-270`; reduce `execute_extract_query` to calling
   it; rewrite `SqlPartitionSpec::sort_order`'s field doc (`:40-44`) and `execute_extract_query`'s
   own doc comment (`:72-78`), both of which describe the removed verify semantics, to describe the
   sort-applying path; rewrite the `assert_ordering_satisfied` `reason` string (`:104-111`), which
   still tells the reader to check for a missing top-level `ORDER BY`, to describe the plan-shape
   regression it now guards; the `assert_single_partition` `reason` string (`:80-84`) stays accurate
   and needs no change.
2. `rust/analytics/src/lakehouse/sql_batch_view.rs` — rewrite `with_merge_sort_order`'s doc item 3
   (`:145-163`) per §2.
3. `rust/analytics/src/lakehouse/log_stats_view.rs` — drop `ORDER BY time_bin, process_id, level,
   target` (`:50`) and reword the two comments that explain it (`:29`, `:35`).
4. `rust/analytics/tests/sql_partition_spec_sort_order_tests.rs` — the file's two tests,
   `extract_query_missing_order_by_fails_the_write` (`:83-115`) and
   `extract_query_matching_order_by_passes_the_ordering_check` (`:120-178`), assert the same
   ordering property once the sort is applied unconditionally, and the latter already builds its own
   plan (`ctx.sql(extract_query).create_physical_plan()`) rather than going through the production
   path while still carrying the fixture's now-redundant `ORDER BY name, time_bin`. Fold them into
   one helper-driven test (named for the sort-applying path, e.g.
   `extract_query_without_an_order_by_satisfies_the_declared_sort_order`). Inline the out-of-order
   `VALUES` fixture (today's `:90-92`) directly into `make_test_view` instead of keeping
   `extract_query` as its parameter, since only one query remains; reword `make_test_view`'s doc
   comment (`:50`), which calls `extract_query` "the only thing that varies between the two tests
   below," to match a fixture with no parameter left to vary. Call the new helper rather than
   re-applying the sort itself or reaching for `write()`'s `expect_err` (which the fixture's
   `connect_lazy` pool cannot reach once the ordering assertion stops firing). Because the helper
   itself already returns `Err` when the plan isn't single-partition and ordering-satisfying,
   re-checking `ordering_satisfy` on the plan it returns would only prove the helper returned `Ok`;
   instead, execute that plan (`execute_stream` with the helper's returned `TaskContext`), collect
   the resulting batches, and assert the emitted rows actually come out in `(name, time_bin)` order
   — a check the helper's internal assertion does not make — replacing today's plan-shape assertion
   and its failure message (`:165`). Keep the trailing "Sanity-check the view itself still declares
   that sort_order" block (`:168-178`) unchanged: it is the only remaining coverage of
   `SqlBatchView::with_merge_sort_order` itself, since the folded test now passes declared columns
   to the helper directly rather than through the view. Rewrite the module header (`:1-12`), which
   states the removed "refuses to record a false sort_order guarantee ... e.g. a missing top-level
   `ORDER BY`" contract verbatim, to describe the sort-applying path — without citing this plan
   document.
5. `rust/analytics/tests/log_stats_ordering_tests.rs` — update
   `log_stats_extract_query_satisfies_its_declared_sort_order` (`:173`) to plan through the new
   helper rather than the raw SQL text. Since the helper itself already returns `Err` when the
   plan isn't single-partition and ordering-satisfying, replace the `partition_count` and
   `ordering_satisfied` assertions (`:219-242`) with a plain `expect()` on the helper's `Ok` —
   re-checking those same properties here would only prove the helper returned `Ok`. Reword the
   doc comment (`:165-169`, pinning "`ORDER BY time_bin, process_id, level, target`") and the
   module header's `ORDER BY` half (`:4`, `:10`) accordingly; the dropped assertion message
   (`:241`, "check for a missing or reordered top-level ORDER BY") goes with the assertion it
   belonged to.
6. `rust/analytics/tests/ordered_aggregation_spike_tests.rs` — reword the rationale comments in
   `cte_internal_order_by_is_discarded_by_a_later_join` and
   `top_level_order_by_satisfies_the_declared_columns` that cite the removed contract
   ("SqlPartitionSpec::write's declared-path plan verification relies on" / "plan verification relies
   on to accept a fresh extract query"). Both assertions are about DataFusion's own plan behavior and
   keep passing unchanged.
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

## Files to Modify

Modified:
- `rust/analytics/src/lakehouse/sql_partition_spec.rs`, `sql_batch_view.rs`, `log_stats_view.rs`
- `rust/analytics/tests/sql_partition_spec_sort_order_tests.rs`, `log_stats_ordering_tests.rs`,
  `ordered_aggregation_spike_tests.rs`
- `CHANGELOG.md`
- `mkdocs/docs/admin/functions-reference.md`, `mkdocs/docs/query-guide/python-api.md`

No new files, no migration, no SQL-surface change.

## Trade-offs

**Applying the sort vs. keeping the assertion-only contract.** The current contract is cheaper — no
logical-plan node is added — and it is honest for a hand-written view, whose author can read the
error and add the `ORDER BY`. It stops being honest the moment the sort order arrives as
configuration rather than as code, which is what the DDL work needs. Applying the sort also makes the
two sides of one option behave the same way, which is worth the extra node on its own.

**Changing `execute_extract_query` for every declared-sort view rather than only for new ones.** A
DDL-only sort-applying path would leave the existing behavior untouched, at the cost of two extract
paths differing in whether they trust the author. One path is the point of the change.

## Decisions

- Applying the declared sort and building the physical plan lives in one `pub` helper rather
  than being re-implemented by each caller, so a validated plan and the daemon's plan cannot diverge.
- `assert_single_partition` and `assert_ordering_satisfied` are kept on the extract path even though
  no author mistake can trip them any more; they become the regression guard against a plan shape
  that silently invalidates a recorded sort guarantee.
- `log_stats`'s `ORDER BY` is removed in this change rather than left as dead SQL, since the shipped
  view is the in-repo proof that the sort-applying path works.

## Testing Strategy

All no-DB unit tests, in the offline harness the three touched test files already use (lazy pool,
in-memory object store, `NullPartitionProvider`).

**`sql_partition_spec_sort_order_tests.rs`** — the folded test from step 4: an extract query with no
`ORDER BY`, planned through the new helper against a view declaring `(name, time_bin)`, then executed
and its rows collected. The test asserts the rows actually come out in `(name, time_bin)` order,
which is stronger than re-checking the plan's output ordering (the helper has already asserted that
before returning `Ok`), and it exercises the production path rather than a test-local
re-implementation of it.

**`log_stats_ordering_tests.rs`** — the shipped `log_stats` extract query, with its `ORDER BY` now
gone, still plans successfully through the helper, which itself asserts the plan is single-partition
and satisfies `(time_bin, process_id, level, target)`; the test only `expect()`s that `Ok`, since
re-asserting the same properties here would be tautological. Together with the existing
`log_stats_merge_query_stays_a_streaming_kway_merge` this keeps both halves of the streaming contract
pinned for the one shipped declared-sort view.

**`ordered_aggregation_spike_tests.rs`** — assertions unchanged; the file is touched for comment
accuracy only, and its continued passing is the check that this change does not depend on the
DataFusion behaviors it characterizes.

No new `#[ignore]` live-DB test: per `CONTRIBUTING.md` those are reserved for pinning a bug witnessed
in the wild.

## Manual Verification

Materialize a `log_stats` range on a local stack (`python3 local_test_env/ai_scripts/start_services.py`,
then `materialize_partitions`) and confirm the written partitions still record their sort guarantee
and that a `log_stats` query over the range returns rows — the end-to-end check that the applied sort
produces the same materialization as the author-written `ORDER BY` did, which no planning-only test
covers.
