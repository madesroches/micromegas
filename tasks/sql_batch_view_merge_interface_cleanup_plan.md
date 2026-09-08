# SqlBatchView Merge Interface Cleanup Plan

## Overview

Issue #1492 surveyed 8 real `SqlBatchView`-backed view definitions in a downstream deployment and
found three cleanup opportunities in the partition-merge interface (`SqlBatchView::new`,
`.with_merge_sort_order()`, `merger_maker`, `BatchPartitionMerger`): a merger implementation with
no caller anywhere, a constructor parameter that's always `None` in practice, and an
easy-to-miss default for whether a view's merges are ordered. This plan addresses the first two —
deleting the dead type and the unused parameter outright — and deliberately leaves the third
(uneven sort-order adoption) out of scope; see Open Questions.

## Current State

- `SqlBatchView::new` (`rust/analytics/src/lakehouse/sql_batch_view.rs:88-151`) takes
  `merger_maker: Option<&MergerMaker>` as its last, positional, mandatory-to-pass argument.
  All 3 real in-repo views pass `None`: `log_stats_view.rs:73`, `processes_view.rs:73`,
  `streams_view.rs:59`. So do 7 of 9 test call sites (`histo_view_test.rs:60`,
  `sql_view_test.rs:89`, `materialize_fail_isolation_tests.rs:46`,
  `sql_partition_spec_sort_order_tests.rs:58`, and the parameterized
  `sql_batch_view_merge_ordering_tests.rs`, which also has one call passing a custom closure).
  Only two call sites pass a real custom maker, both test-only: `sql_view_test.rs:203`
  (`Some(&make_merger)`, a `#[ignore]` live-DB test's `LogSummaryMerger` — a hand-written,
  non-SQL Rust merger that re-runs one query per distinct `process_id` instead of the SQL text in
  `merge_partitions_query`) and `sql_batch_view_merge_ordering_tests.rs`'s
  `custom_merger_maker_coexists_with_a_declared_merge_sort_order` test (an inline `QueryMerger`
  closure standing in for "a custom `merger_maker`"). `with_merge_sort_order`
  (`sql_batch_view.rs:179-207`), by contrast, is a fluent builder called after `new()` —
  `merger_maker` is the only merge-related knob still forced into the constructor, and per the
  survey backing the issue, no downstream call site uses it either.
- `BatchPartitionMerger` (`rust/analytics/src/lakehouse/batch_partition_merger.rs`) implements
  `PartitionMerger` by slicing a merge into event-time batches to bound memory. It was added in
  #395 (2025-06-19), before the streaming k-way merge (`ScanOrdering::PerFile`, landed in #1402 per
  the k-way merge plan) gave `with_merge_sort_order` views a different, non-batching fix for the
  same memory problem. It's referenced only in doc comments — `mod.rs:20-22`,
  `sql_batch_view.rs:173-178` (naming it as the intended fallback merger to keep passed as
  `merger_maker` during a sort-order rollout, per `tasks/completed/1392_kway_merge_sorted_partitions_plan.md`'s
  Rollout section), and a comment in `sql_batch_view_merge_ordering_tests.rs:273`. Nothing
  constructs one — not in this repo's tests, not at any of the 3 real in-repo call sites, and not
  at any of the 8 downstream call sites the issue surveyed.
- Of the 3 real in-repo views, only `log_stats_view.rs:93` calls `.with_merge_sort_order(...)`.
  `processes_view.rs` and `streams_view.rs` take the default `ScanOrdering::Unordered` merge path
  with no call-site signal that this was a deliberate choice rather than an oversight — the same
  gap the issue's downstream survey found (5 of 8 views undeclared).

## Design

### 1. Remove `BatchPartitionMerger`

Delete `rust/analytics/src/lakehouse/batch_partition_merger.rs` and its `pub mod
batch_partition_merger;` declaration (`mod.rs:20-22`). `PartitionMerger`/`QueryMerger` stay — they're
the mechanism `SqlBatchView` itself uses internally (see §2); this removes one specific, unused
implementation, not the extension point. `MergerMaker` does not stay; it's deleted along with
`merger_maker` in §2. Delete the doc-comment paragraph naming `BatchPartitionMerger` as the intended
fallback (`sql_batch_view.rs:173-178`) outright rather than rewording it — its only purpose was
explaining how a custom `merger_maker` coexists with `with_merge_sort_order`, and once §2 removes
`merger_maker` there is nothing left for the paragraph to describe.
`tasks/completed/1392_kway_merge_sorted_partitions_plan.md` is left
untouched — it's a closed plan's historical record of what was true when it was written, not live
documentation.

Time-slicing a merge into event-time batches (`BatchPartitionMerger`'s approach) was always a
workaround for bounding merge memory: it re-runs the merge query once per batch to keep any one
query's working set small, but still buffers a full sort per batch and gives no ordering guarantee
to callers. The problem it worked around is now fixed for **every** merger, not just ones that
declare an ordering: a merge session sets `repartition_file_scans = false`
(`merge.rs:66-71`, from `tasks/1491_merge_scan_memory_plan.md`), so every `QueryMerger` merge
scans its source partitions with a single sequential reader regardless of `ScanOrdering` — a view
with no natural sort contract gets a bounded-memory merge for free, with no ordering guarantee to
maintain (see the note on `View::get_scan_output_ordering`, `view.rs:153-160`). `ScanOrdering`'s
job is a different one: eliding sorts. `PerFile` lets a merge query that must preserve order
across an aggregation stream through a k-way merge instead of a blocking `SortExec`
(`with_merge_sort_order`, already on `SqlBatchView`), and lets user queries that `ORDER BY` those
columns skip their own sort. Neither the memory bound nor the sort elision needs
`BatchPartitionMerger`'s batch-and-requery workaround, which is why removing it here is a cleanup
rather than a capability loss.

### 2. `merger_maker` is deleted outright

Per user direction, this isn't demoted to a builder method — the whole customization hook is
removed. `SqlBatchView::new` drops the `merger_maker` parameter, and `new()` always builds the
default `QueryMerger`-based merger from `merge_partitions_query` (today's `unwrap_or` branch,
unconditionally). The `MergerMaker` type alias (`sql_batch_view.rs:34-35`) is deleted along with
it. `SqlBatchView` no longer offers any way to override its merger with custom Rust logic — every
merge goes through SQL text plus, optionally, a declared `with_merge_sort_order`.

`PartitionMerger`/`QueryMerger` (`merge.rs`) are untouched: they're the mechanism `SqlBatchView`
itself uses internally, and `BlocksView` (a hand-written `View`, not a `SqlBatchView`) constructs a
`QueryMerger` directly for its own `Concatenated`-ordering merge. Only `SqlBatchView`'s
constructor-level hook onto that trait is removed.

### 3. Uneven sort-order adoption — out of scope

Not addressed by this plan. See Open Questions.

## Implementation Steps

1. Delete `batch_partition_merger.rs` and its `mod.rs` declaration; delete the
   `sql_batch_view.rs:173-178` doc-comment paragraph outright (its only purpose was explaining how a
   custom `merger_maker` coexists with `with_merge_sort_order`, and step 2 removes `merger_maker`).
2. `sql_batch_view.rs`: drop `merger_maker` from `new()`'s parameter list and delete the
   `MergerMaker` type alias.
3. Update call sites:
   - `log_stats_view.rs`, `processes_view.rs`, `streams_view.rs`: drop the trailing `None,`
     argument from `SqlBatchView::new(...)`.
   - `histo_view_test.rs`, `materialize_fail_isolation_tests.rs`,
     `sql_partition_spec_sort_order_tests.rs`: drop the trailing `None,` argument.
   - `sql_view_test.rs`: drop the trailing `None,` argument from the remaining plain-merge view
     constructor (`make_log_entries_levels_per_process_minute_view`); delete the entire
     custom-merger demonstration this file exists partly to exercise —
     `LogSummaryMerger`, `make_merger`, and
     `make_log_entries_levels_per_process_minute_view_with_custom_merge`
     (lines ~108-273) — along with the `sql_view_test()` block that builds that view and calls
     `test_log_summary_view` on it; `test_log_summary_view` itself and its call for the plain view
     stay. Drop the now-unused `PartitionMerger`/`MergeQueryResult`/`RecordBatchReceiverStreamBuilder`
     /`query_partitions`/`DictionaryArray`/`StringArray`/`Int32Type`/`typed_column_by_name` imports
     this deletion leaves behind (`cargo clippy` will flag any missed).
   - `sql_batch_view_merge_ordering_tests.rs`: delete
     `custom_merger_maker_coexists_with_a_declared_merge_sort_order` (its
     uncertified-input-falls-back-to-the-plain-merger case is already covered by
     `one_uncertified_input_falls_back_to_the_plain_merger`); fold
     `make_test_view_with_merge_query_and_merger` into `make_test_view_with_merge_query` (drop the
     `merger_maker` parameter — with it gone the two functions are identical) and update its one
     remaining reference; delete the now-unused `MergerMaker`/`RuntimeEnv`/`Schema`/`QueryMerger`/
     `PartitionMerger` imports this leaves behind.
4. Add a `CHANGELOG.md` Unreleased entry (Analytics) covering both changes and the breaking
   signature change.
5. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test` from `rust/`.

## Files to Modify

- `rust/analytics/src/lakehouse/sql_batch_view.rs`
- `rust/analytics/src/lakehouse/mod.rs`
- `rust/analytics/src/lakehouse/batch_partition_merger.rs` (deleted)
- `rust/analytics/src/lakehouse/processes_view.rs`
- `rust/analytics/src/lakehouse/streams_view.rs`
- `rust/analytics/src/lakehouse/log_stats_view.rs`
- `rust/analytics/tests/sql_batch_view_merge_ordering_tests.rs`
- `rust/analytics/tests/sql_view_test.rs`
- `rust/analytics/tests/histo_view_test.rs`
- `rust/analytics/tests/sql_partition_spec_sort_order_tests.rs`
- `rust/public/tests/materialize_fail_isolation_tests.rs`
- `CHANGELOG.md`

## Decisions

- `merger_maker` is deleted outright rather than demoted to a builder method — user call,
  overriding this plan's initial `with_merger_maker` proposal.
- Uneven sort-order adoption (issue item 3) is left out of scope for this plan — user call,
  overriding this plan's initial `accept_unordered_merge()` proposal (see Open Questions).
- `BatchPartitionMerger` is removed, not kept as dormant API — user call: its batching approach
  was always a workaround for bounding merge memory, and the single-sequential-reader merge scan
  already bounds that for every merger (see Design §1).
- **No `with_merge_concatenated_order` builder for `SqlBatchView`.** `ScanOrdering::Concatenated`
  stays available only to hand-written `View` impls (`BlocksView`, `ThreadSpansView`).
  `SqlBatchView` cuts partitions on **insert time** while its leading sort column is **event
  time**, so late-arriving telemetry puts the same sort-key value in two insert windows and the
  `Concatenated` non-overlap contract (`view.rs:164-166`) cannot hold; a violated declaration errors
  **user queries** (via `get_scan_output_ordering`), not just merges, so this isn't a case where
  approximating the contract is safe.

## Documentation

No `mkdocs/` page references `BatchPartitionMerger`, `merger_maker`, or `with_merge_sort_order`
today; nothing there needs updating. `CHANGELOG.md` gets the Unreleased entry described above.

## Testing Strategy

- Existing offline (no-DB) tests already cover `with_merge_sort_order`
  (`sql_batch_view_merge_ordering_tests.rs`, `sql_partition_spec_sort_order_tests.rs`) and keep
  doing so after the one deleted test and the fixture simplification — the merger-selection matrix
  those tests exercise (all certified / one uncertified / all empty / mixed) is orthogonal to
  `merger_maker` and is untouched.
- No new test is added for the deletions themselves: removing a parameter and its only two
  exercising tests has nothing left to assert beyond "it still compiles and the remaining tests
  still pass," which the compiler and `cargo test` already cover.
- `cargo clippy --workspace -- -D warnings` catches any dead-code/unused-import fallout from
  deleting `batch_partition_merger.rs`, `MergerMaker`, and the two custom-merger test fixtures.

## Manual Verification

None needed — every change here is covered by the automated tests above and by the compiler (the
removed positional parameter fails every uncorrected call site at compile time).

## Open Questions

1. Issue item 3 (uneven sort-order adoption — `processes`/`streams` silently take the default
   unordered merge path with no call-site signal it was deliberate) is left unaddressed by this
   plan. Worth a follow-up (a lint, a doc example, or an explicit opt-out builder), or is the
   status quo acceptable?

The `Concatenated`-parity question an earlier draft raised here is now settled — see Decisions.
