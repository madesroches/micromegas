# list_view_set_definitions() Includes Built-in View Sets Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1720

## Overview
`list_view_set_definitions()` lists only DDL-defined view sets. Make it list every view set,
built-in ones included, each with its `update_group` from `View::get_update_group()`, so a client
can read the materialization order from one query instead of hard-coding the built-in groups.

## Current State
- `rust/analytics/src/lakehouse/list_view_set_definitions_table_function.rs` reads
  `lakehouse_view_set_definitions` from Postgres only. Schema: `view_set_name`, `definition_sql`,
  `update_group`, `updated_at` (all non-null), `updated_by` (nullable).
- It is registered in `register_lakehouse_functions` (`rust/analytics/src/lakehouse/query.rs`,
  admin-gated block) with only the `DataLakeConnection`; the `view_factory` is in scope there.
- Built-in view sets live in the `ViewFactory` (`default_view_factory`, `view_factory.rs`):
  global views (`blocks`, `processes`, `streams`, `log_entries`, `measures`) plus makers with no
  global instance (`images`, `async_events`, ...). `catalog::list_view_sets` already walks both.
- DDL-defined view sets are folded into the same factory by `ViewRegistry`, so the factory
  contains DDL sets too.
- `python/micromegas/micromegas/cli/views.py` `read_server_state` reads this function and treats
  every row as a DDL definition (`plan`, `list`, `show`, `pull`, server-only detection), so
  built-in rows would show up as server-only/removable there.

## Design
Rows are the union of Postgres rows (DDL) and factory view sets not named in Postgres.

- Schema: keep the existing five columns in order and append **`source`** (Utf8, non-null,
  `'ddl'` | `'builtin'`) last. Relax `definition_sql`, `update_group` and `updated_at` to
  nullable (nullability only; types unchanged). `updated_by` is already nullable.
- Built-in row: `definition_sql`/`updated_at`/`updated_by` NULL; `update_group` =
  `get_update_group()` of the view set's global view, or NULL when it has no global instance or
  the view returns `None`.
- A Postgres row wins on name collision (it covers a DDL set that is also in the factory, and one
  that failed to load).
- Extract a pure function, e.g. `fn merge_rows(ddl: Vec<DdlRow>, factory: &ViewFactory) ->
  Vec<DefinitionRow>`, sorted by `view_set_name` like today; `scan` fetches Postgres rows, calls
  it, builds the batch (applying `limit`). The table function takes an `Arc<ViewFactory>` as a
  second constructor argument; `query.rs` passes `view_factory.clone()`.
- Which global views belong to a view set: iterate `factory.get_global_views()` keyed by
  `get_view_set_name()` (first wins, as in `list_view_sets`); then `get_view_sets()` keys without a
  global view.
- Batch building: the builders switch to Option-typed arrays (`StringArray`, `Int32Array`,
  `TimestampNanosecondArray` built from `Option` values) plus an appended `source` Utf8 column.
- Python CLI: `read_server_state` adds `WHERE definition_sql IS NOT NULL` so built-ins never
  enter the plan; this also works against an older server.

## Implementation Steps
1. Update the table function schema, constructor, `merge_rows` and batch building, and the module
   doc comment atop `list_view_set_definitions_table_function.rs`.
2. Pass `view_factory` at the registration site in `query.rs`, and update the comment at
   ~247-249 ("straight from Postgres").
3. Update `read_server_state` in `python/micromegas/micromegas/cli/views.py`.
4. Update docs and CHANGELOG. The CHANGELOG entry carries a `**Minor breaking change:**` clause
   (the pub `ListViewSetDefinitionsTableFunction::new` gains an `Arc<ViewFactory>` parameter) and
   records the new `source` column and the nullable columns.

## Files to Modify
- `rust/analytics/src/lakehouse/list_view_set_definitions_table_function.rs` (incl. module doc
  comment)
- `rust/analytics/src/lakehouse/query.rs` (registration and the "straight from Postgres" comment)
- `python/micromegas/micromegas/cli/views.py`
- `python/micromegas/tests/cli/test_views.py`
- `mkdocs/docs/admin/functions-reference.md`, `views-as-code.md` (remove "never appear in
  `list_view_set_definitions()`"), `materialized-views.md` (Introspection bullet)
- `CHANGELOG.md` (with `**Minor breaking change:**` clause)

## Trade-offs
- Nullable columns plus `source` (the issue's both options) rather than a separate function:
  one query answers the ordering question, and `source` lets clients filter without relying on
  NULL-ness.
- Listing all view sets, not only those with an `update_group`: matches `list_view_sets()` and
  the issue's `WHERE update_group IS NOT NULL` query.

## Decisions
- SQL compatibility: `source` is appended last; existing columns keep name, type and order.
  Nullability relaxation is accepted since built-in rows have no stored definition.
- A DDL view set dropped from Postgres but still in a stale registry snapshot may briefly be
  labelled `builtin`; accepted.

## Documentation
Update the `list_view_set_definitions()` entry in `functions-reference.md` (description, new
`source` column, nullable columns, example ordering query) and the two pages above. Add a
`source` row to the return-columns table in `functions-reference.md` (Utf8, `'ddl'` or
`'builtin'`).

## Testing Strategy
No-DB unit tests on `merge_rows` with a small `ViewFactory`: built-ins get `source='builtin'`,
NULL definition fields and the view's `update_group`; a maker-only set gets NULL `update_group`;
a Postgres row with the same name as a factory view set appears once as `ddl`; output is sorted;
the batch carries NULLs in the Option-typed columns and the appended `source` column.
The existing admin-gate test (`lakehouse_admin_gate_test.rs`) keeps covering registration (update
its construction if needed). In `python/micromegas/tests/cli/test_views.py`, a test using the
existing FakeClient asserts the SELECT recorded in `.statements` by `read_server_state` contains
`WHERE definition_sql IS NOT NULL`.

## Open Questions
None.
