# list_view_sets() Sort Order Reporting Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1823

## Overview

`list_view_sets()` tells callers (people, the MCP server, the query skill) which columns a view
is already sorted by, so they know which filter or join is cheap. Today it reports `NULL` for
`blocks`, whose new partitions are written sorted by `insert_time`, and for every view set that has only a
`ViewMaker` (`thread_spans` among them, which declares a `Concatenated` ordering on `begin`).
This plan makes the catalog report each view set's real sort order without changing how any
query is planned.

## Current State

- `rust/analytics/src/lakehouse/catalog.rs:57-62` derives `sort_order` only from
  `View::get_scan_output_ordering()`. `catalog.rs:96` hard-codes `None` for maker-only view sets.
- `BlocksView` (`blocks_view.rs`) leaves `get_scan_output_ordering()` at its `Unordered`
  default. Its extract query orders by `blocks.insert_time, blocks.block_id`, its ordered merger
  declares `Concatenated` on `insert_time` / `OrderingBounds::InsertTime` (`blocks_view.rs:108`),
  and `get_merged_partition_sort_order` records `["insert_time"]` (`blocks_view.rs:258`), but
  only when every input partition already carries that recorded `sort_order`
  (`all_inputs_ordered_or_empty`).
- `ThreadSpansView::get_scan_output_ordering()` (`thread_spans_view.rs:443`) returns
  `Concatenated` on `begin` (`MIN_TIME_COLUMN`) with `OrderingBounds::EventTime`.
  `ThreadSpansViewMaker` (`thread_spans_view.rs:56`) exposes nothing about it.
- Besides `ThreadSpansView`, only `SqlBatchView` declares an ordering (`PerFile` when configured with `merge_sort_order`); no maker does. The `ViewMaker` trait (`view_factory.rs:243`) has
  `make_view`, `get_schema_hash`, `get_schema`, and is implemented by seven makers: `log_view`,
  `metrics_view`, `net_spans_view`, `images_view`, `async_events_view`, `thread_spans_view`,
  `otel/spans_view`.

## Design

### Reported sort order is separate from the scan declaration

`get_scan_output_ordering()` is a trusted planner input. The fix stays on the
reporting side:

- Add a helper on `ScanOrdering` (`partitioned_execution_plan.rs`):

  ```rust
  pub fn sort_columns(&self) -> Option<Vec<String>>
  ```

  `Unordered` → `None`; `Concatenated { columns, .. }` / `PerFile { columns }` → the column names.
  This replaces the `match` in `catalog.rs`.

- Add a `View` trait method (`view.rs`) that reports the order the view's data is written in:

  ```rust
  /// Ascending columns this view's partitions are written sorted by, as reported by
  /// `list_view_sets()`. Informational only -- planning reads `get_scan_output_ordering()`.
  fn get_reported_sort_order(&self) -> Option<Vec<String>> {
      self.get_scan_output_ordering().sort_columns()
  }
  ```

  The default is correct for every view that declares a scan ordering (`SqlBatchView`,
  `ThreadSpansView`) and for the unordered ones, so only `BlocksView` overrides it, returning
  `Some(insert_time_sort_order())`.

### Maker-only view sets

Add a **required** method to `ViewMaker` (`view_factory.rs`):

```rust
/// Ordering every instance this maker builds declares; see `View::get_scan_output_ordering`.
fn get_scan_output_ordering(&self) -> ScanOrdering;
```

It is required, with no default, so the compiler lists all seven makers. Six return
`ScanOrdering::Unordered`. `ThreadSpansViewMaker` returns the same value as
`ThreadSpansView::get_scan_output_ordering()`. Both call one private
`fn thread_spans_scan_ordering() -> ScanOrdering` in `thread_spans_view.rs`, so the two cannot
drift apart.

The catalog's maker-only loop sets
`sort_order: view_maker.get_scan_output_ordering().sort_columns()`. We don't build an instance
with `make_view` here, because that needs a valid instance id (a stream id for `thread_spans`).

### SQL surface

No change to the shape. The `sort_order` column keeps its name, type, and position; only its
values get more accurate. Update the `ViewSetInfo::sort_order` doc comment to match.

## Implementation Steps

1. `partitioned_execution_plan.rs`: add `ScanOrdering::sort_columns()`.
2. `view.rs`: add `View::get_reported_sort_order()` with the default above.
3. `blocks_view.rs`: override `get_reported_sort_order()` to return
   `Some(insert_time_sort_order())`.
4. `view_factory.rs`: add the required `ViewMaker::get_scan_output_ordering()`.
5. Implement it in the seven makers; in `thread_spans_view.rs`, extract
   `thread_spans_scan_ordering()` and use it from both the view and the maker.
6. `catalog.rs`: use `global_view.get_reported_sort_order()` in the first loop and
   `view_maker.get_scan_output_ordering().sort_columns()` in the second; update the field doc; remove the now-unused `ScanOrdering` import (`catalog.rs:8`).
7. Tests (see Testing Strategy).
8. Docs and `CHANGELOG.md` (see Documentation).
9. Run `build/sync_claude_plugin_skills.py` to regenerate the claude-plugin copy of the query skill.

## Files to Modify

- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs`
- `rust/analytics/src/lakehouse/view.rs`
- `rust/analytics/src/lakehouse/view_factory.rs`
- `rust/analytics/src/lakehouse/catalog.rs`
- `rust/analytics/src/lakehouse/blocks_view.rs`
- `rust/analytics/src/lakehouse/thread_spans_view.rs`
- `rust/analytics/src/lakehouse/{log_view,metrics_view,net_spans_view,images_view,async_events_view}.rs`
- `rust/analytics/src/lakehouse/otel/spans_view.rs`
- `rust/analytics/tests/catalog_tests.rs`
- `mkdocs/docs/admin/functions-reference.md`, `mkdocs/docs/query-guide/functions-reference.md`
- `rust/public/src/servers/mcp/skills/micromegas-query/SKILL.md`
- `claude-plugin/skills/micromegas-query/SKILL.md` (regenerated by `build/sync_claude_plugin_skills.py`)
- `CHANGELOG.md`

## Trade-offs

- **Declare `Concatenated` scan ordering on `BlocksView`** (the issue's first option): rejected.
  It would let the planner skip sorts, but `Concatenated` trusts every partition, and legacy
  `blocks` partitions without a recorded `sort_order` are not guaranteed sorted. Turning on sort
  elision for `blocks` is a separate correctness change that would need its own certification
  design.
- **Declare `PerFile { insert_time }` on `BlocksView`**: certification-gated so safe, but it changes the user-query scan shape (per-partition file groups + `SortPreservingMergeExec`) whenever all partitions certify, which this plan excludes.
- **Catalog falls back to the merger's `merge_scan_ordering`**: would need a new accessor on
  `PartitionMerger` and would couple the catalog to merge internals. An explicit
  `get_reported_sort_order()` override is simpler and local to the view.

## Documentation

- `mkdocs/docs/admin/functions-reference.md`: describe the `sort_order` row as the order the
  view set's partitions are normally written in (informational, not guaranteed for every
  partition), reported for maker-only view sets as well (drop "or when the view set has no
  global instance").
- `mkdocs/docs/query-guide/functions-reference.md`: reword "the `sort_order` of its scan" to
  match.
- `rust/public/src/servers/mcp/skills/micromegas-query/SKILL.md` (line 232): reword "`sort_order` lists the
  columns a scan is already ordered by" to the order the view set's partitions are normally
  written in (informational, not guaranteed for every partition). Then run `build/sync_claude_plugin_skills.py` to update the claude-plugin copy.
- `CHANGELOG.md`: an Unreleased entry for the fix, with a **Minor breaking change** clause:
  `ViewMaker` gains a required `get_scan_output_ordering()`.

## Testing Strategy

No-DB unit tests in `rust/analytics/tests/catalog_tests.rs`:

- Extend `test_list_view_sets_sort_order_of_global_views`: the `blocks` global view (already in
  `base_factory`) reports `Some(["insert_time"])`, and `log_entries` still reports `None`.
- New test: a factory with `ThreadSpansViewMaker` reports `thread_spans` as `Some(["begin"])`.
- `test_list_view_sets_description_and_columns` keeps asserting that `measures` (an unordered
  maker) reports `None`; update its message, since maker-only sets can now carry an order.
