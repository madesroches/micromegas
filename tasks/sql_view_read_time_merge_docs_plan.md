# SQL-Defined View Read-Time Merge Docs Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1752

## Overview
`mkdocs/docs/admin/materialized-views.md` never states outright that every query on a SQL-defined view runs `merge_partitions_query` at read time. Readers who only look at `MaterializedView::scan` assume an aggregating view returns several rows per grain key until the daemon merges its partitions. Add one bullet to the authoring-rules list to say otherwise.

## Current State
- `SqlBatchView::register_table` (`rust/analytics/src/lakehouse/sql_batch_view.rs`) registers the raw partitions as `__<name>__partitions`, then registers `<name>` as a DataFusion view over `merge_partitions_query` with `{source}` replaced by that table name.
- The doc's "Author obligations" section (`materialized-views.md`, around line 136) has a bullet that the merge query's aggregates must be composable. It says nothing about read-time execution.

## Design
Docs-only change. Add this bullet immediately after the "merge query's aggregates must be composable" bullet in `mkdocs/docs/admin/materialized-views.md`:

> - Querying `<name>` runs `merge_partitions_query` over the partitions the query range selects; the unmerged partitions are readable as `__<name>__partitions`. A view whose merge query re-aggregates by its grain returns one row per grain key even before the daemon merges partitions.

## Implementation Steps
1. Edit `mkdocs/docs/admin/materialized-views.md` to insert the bullet.
2. Add a `CHANGELOG.md` entry if the project's convention covers doc changes.

## Files to Modify
- `mkdocs/docs/admin/materialized-views.md`

## Trade-offs
Alternatives are a new section or rewording lines 12 and 62. A single bullet next to the related composability rule is the smallest change that makes the statement explicit.

## Documentation
The change is the documentation itself.

## Testing Strategy
No code changes, so there is no automated coverage. Check that the page builds, if the mkdocs tooling is available.
