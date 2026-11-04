# View Merge Streaming Check Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1754

## Overview

A view set's `merge_partitions_query` can silently lose the streaming per-file k-way merge: when
a `SortExec` survives in the physical plan built by `QueryMerger::execute_sorted_merge`, the merge
buffers its whole input in memory. Today the only signal is a daemon warning after deploy. This
plan adds a server-side admin table function that plans a view set definition's merge exactly as
the maintenance daemon would (without executing it) and reports whether a `SortExec` remains, and
makes `micromegas-views plan` / `apply` call it, warn per offending view, and exit non-zero under
`--strict`.

## Current State

- `QueryMerger::execute_sorted_merge` (`rust/analytics/src/lakehouse/merge.rs:210`) sets four
  optimizer flags, runs the merge SQL, applies `DataFrame::sort` over the `merge_sort_order`
  columns, builds the physical plan, runs two hard assertions, then computes
  `ordering_honored = !plan_str.contains("SortExec")` and logs a warning. Planning and execution
  are interleaved in one function.
- The session comes from `make_merge_session_context` (`merge.rs:49`, sets
  `repartition_file_scans = false`); the `source` table is a `PartitionedTableProvider` with
  `ScanOrdering::PerFile`.
- `build_sql_batch_view` (`view_definition.rs:118`) turns a `ViewDefinition` into a `SqlBatchView`
  (file schema + `with_merge_sort_order`), and `validate_view_definition` already plans the merge
  query against an empty `MemTable` source, but without the ordered scan or the sort, so it cannot
  see the surviving `SortExec`.
- DDL parsing (`parse_view_ddl`, `view_definition_from_options`, `ViewDdl`) lives in
  `rust/public/src/servers/view_ddl.rs`; `public` depends on `analytics`, so an `analytics` UDTF
  cannot call it today.
- Admin UDTFs are registered in `register_lakehouse_functions`' admin block
  (`rust/analytics/src/lakehouse/query.rs:~235-255`), e.g. `list_view_set_definitions`.
- The CLI (`python/micromegas/micromegas/cli/views.py`) `cmd_plan` (l.343) / `cmd_apply` (l.364)
  work from `compute_plan`'s `creates` / `updates`, with local definitions in `definitions[name].text`.

## Design

### 1. Shared planning step (no drift between check and daemon)

Extract the plan-building half of `execute_sorted_merge` into a reusable function in `merge.rs`:

```rust
pub struct PlannedSortedMerge { plan, task_ctx, plan_text: String, ordering_honored: bool }
pub async fn plan_sorted_merge(ctx: &SessionContext, query: &str, columns: &[ScanSortColumn],
                               insert_range: TimeRange) -> Result<PlannedSortedMerge>
```

It owns the four optimizer settings, `ctx.sql`, `df.sort`, `create_physical_plan`, both hard
assertions and the `plan_str.contains("SortExec")` test. `execute_sorted_merge` becomes
`plan_sorted_merge` + the existing warn + `execute_stream`.

### 2. Check function

New `rust/analytics/src/lakehouse/view_merge_check.rs`:

```rust
pub struct MergeStreamingReport { view_set_name, merge_sort_order: Vec<String>,
                                  streams: bool, sort_exec_line: Option<String>, plan: String }
pub async fn check_merge_streaming(
    def: &ViewDefinition,
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    session_configurator: Arc<dyn SessionConfigurator>,
) -> Result<Option<MergeStreamingReport>>
```

- Returns `None` when `def.options.merge_sort_order` is absent/empty (skipped, per the issue).
- Builds the `SqlBatchView` via `build_sql_batch_view` (gives the file schema), then a session via
  `make_merge_session_context` with `CallerContext::maintenance()` (same as the daemon), registers
  `source` as a `PartitionedTableProvider::with_scan_ordering(.., PerFile)` over two synthetic
  `Partition` entries (planning only; no object-store IO) so the plan has the multi-file k-way
  shape, and calls `plan_sorted_merge`. Both partitions are non-empty (`num_rows > 0`, `file_path`,
  `file_size`, `event_time_range` set) with `sort_order = Some(merge_sort_order)`, matching
  `make_partition` + `certifying_sort_order()` in
  `rust/analytics/tests/sql_batch_view_merge_ordering_tests.rs:106-187`; otherwise
  `make_partitioned_execution_plan` drops them or falls back from `PerFile` to `Unordered` / `EmptyExec`.
  Two non-empty certifying partitions reproduce the daemon's shape for any real merge with k >= 2;
  k = 1 would not.
- The merge query is `def.merge_partitions_query.replace("{source}", "source")` (as `view_definition.rs`
  validation does); ordering columns come from the public `get_scan_output_ordering()`
  (`sql_batch_view.rs` ~401-408) and the file schema from `get_file_schema()`.
- The check builds one synthetic `insert_range` covering the synthetic partitions and passes
  `Arc::new(PartitionCache::empty(insert_range))` as `part_provider` (pattern in
  `sql_batch_view_merge_ordering_tests.rs:124`), `Some(insert_range)` as `query_range` (the daemon
  passes `Some(insert_range)`, `merge.rs:300`; this installs `TableScanRewrite`), and the same
  `insert_range` to `plan_sorted_merge`. Other views referenced by the merge query plan as
  `EmptyExec`, so the verdict is exact only for merge queries over `source`.
- `sort_exec_line` is the first plan line containing `SortExec`, the same test `plan_sorted_merge` uses.
- Hard assertion failures (`assert_single_partition` etc.) make `check_view_merge` return an error
  (the query fails), not a row with `streams = false`.

### 3. Admin UDTF

`check_view_merge(ddl)` — admin-gated, registered next to `list_view_set_definitions`. Takes the
verbatim `CREATE [OR REPLACE] MATERIALIZED VIEW ...` text, parses it, runs `check_merge_streaming`.
Output columns: `view_set_name`, `merge_sort_order` (Utf8, comma-joined), `streams` (Boolean),
`sort_exec_line` (Utf8, nullable), `plan` (Utf8). Zero rows for a view without `merge_sort_order`.
Non-DDL or `DROP` input is an error.

To let the UDTF in `analytics` parse DDL, move `ViewDdl`, `parse_view_ddl`, `require_eof` and
`view_definition_from_options` from `public/src/servers/view_ddl.rs` into
`analytics/src/lakehouse/view_ddl.rs`, along with the `value_to_string` and `value_to_i32` helpers;
`public` keeps `authorize_view_ddl` (it needs `tonic::Status`) and re-exports only `ViewDdl` and
`parse_view_ddl` via `pub use` (the four helpers `require_eof`, `value_to_string`, `value_to_i32`,
`view_definition_from_options` stay private in `analytics`) so `flight_sql_service_impl.rs` keeps compiling. The parser module doc (`rust/public/src/servers/view_ddl.rs:1-9`) moves to the top of
`analytics/src/lakehouse/view_ddl.rs`; the remaining `public` `view_ddl.rs` gets a short header
covering `authorize_view_ddl` and the re-exports. `view_ddl_parse_tests.rs` is split: the
parse tests move to `rust/analytics/tests/`; the `authorize_view_ddl` tests (and their caller
helpers) stay in `rust/public/tests/`, keeping their `[[test]]` entry in `rust/public/Cargo.toml`; its
header narrows to the `authorize_view_ddl` tests.
This is a Rust API move only (see CHANGELOG).

The UDTF needs the session configurator; `make_session_context` already takes one, so it is threaded
through `register_functions` -> `register_lakehouse_functions` to the new UDTF (all callers
updated). Planning happens in an async `scan`, following
`list_view_set_definitions_table_function.rs`.

### 4. Client

In `views.py`:
- `check_merge_streaming(client, ddl)` runs `SELECT * FROM check_view_merge('<escaped ddl>')`
  (single quotes doubled) and returns the report row or `None`.
- `cmd_plan`: after printing the plan, for each view in `creates` + `updates` call the check and
  print a `warning:` line per view with `streams = false`, including `sort_exec_line` and the
  `merge_sort_order`. Unchanged views are not checked (they are already deployed).
- `--strict` (on `plan` and `apply`): any warning makes the exit code non-zero.
- `cmd_apply`: run the same check right after printing the plan and before `confirm_apply`; warn,
  and under `--strict` exit non-zero before the prompt. Without `--strict`, apply proceeds after the
  warnings. The "No changes" early return and drop-only runs skip the check.
- Any failure of the check query for a view (e.g. an extract query that does not plan yet because a
  dependency is created in the same apply) is reported as "check skipped: <reason>" for that view,
  not as a streaming warning; under `--strict` it also fails.

## Implementation Steps

1. `merge.rs`: extract `plan_sorted_merge`; refactor `execute_sorted_merge`.
2. Move DDL parsing (and its module doc) to `analytics/src/lakehouse/view_ddl.rs`; give the
   remaining `public` `view_ddl.rs` a short header; update `public` imports; split
   `view_ddl_parse_tests.rs` (parse tests to `analytics/tests/`, `authorize_view_ddl` tests stay, header narrowed).
3. Add `view_merge_check.rs` and `CheckViewMergeTableFunction`; register in the admin block of
   `query.rs`, threading `configurator` through `register_functions` / `register_lakehouse_functions`
   and updating all callers; add module lines to `lakehouse/mod.rs`, including `pub mod view_ddl;`.
4. `views.py`: check helper, `plan`/`apply` integration, `--strict`.
5. Docs and CHANGELOG.

## Files to Modify

- `rust/analytics/src/lakehouse/merge.rs`
- `rust/analytics/src/lakehouse/view_ddl.rs` (new, moved), `view_merge_check.rs` (new),
  `check_view_merge_table_function.rs` (new), `mod.rs`, `query.rs`
- `rust/public/src/servers/view_ddl.rs`, `flight_sql_service_impl.rs` (imports), `rust/public/Cargo.toml`
  (`[[test]]` entry for `view_ddl_parse_tests.rs`)
- `rust/public/src/servers/view_ddl.rs` (admin-gated function list in the doc comment, l.43),
  `rust/analytics/tests/lakehouse_admin_gate_test.rs` (header list)
- `mkdocs/docs/admin/authorization.md` (~l.227-245), `mkdocs/docs/admin/authentication.md` (~l.586):
  "nine admin-gated functions" becomes "ten", add `check_view_merge`
- `rust/analytics/tests/` (moved DDL parse tests, new tests below); `rust/public/tests/view_ddl_parse_tests.rs`
  (keeps the `authorize_view_ddl` tests)
- `python/micromegas/micromegas/cli/views.py`, `python/micromegas/tests/cli/test_views.py`
- `mkdocs/docs/admin/views-as-code.md`, `mkdocs/docs/admin/functions-reference.md`, `materialized-views.md`
- `CHANGELOG.md`

## Trade-offs

- **UDTF vs. a validation step inside `CREATE MATERIALIZED VIEW`**: a validation failure at CREATE
  would be a behavior change for existing views and forbid shipping a deliberately buffering view.
  A separate read-only function lets the client choose warn vs. gate.
- **Moving the DDL parser vs. passing pre-parsed arguments** (`check_view_merge(extract, merge, ...)`):
  the DDL text is what the CLI already holds, and a multi-argument signature would duplicate the
  option parsing in Python.
- **Sharing `plan_sorted_merge` vs. re-implementing the plan in the check**: re-implementing would
  drift from the daemon's session settings, which is exactly the failure being detected.

## Documentation

- `views-as-code.md`: `plan` / `apply` warning output and `--strict`; update the `plan`/`apply`
  synopsis (add `[--strict]`), the "plan always exits 0 for pending changes" sentence, and the
  exit-code list; the "Validation happens at `apply`, not at `plan`" lead-in ("plan only compares
  text" is stale: `plan` now also runs the advisory merge streaming check server-side, while
  CREATE-time validation still only surfaces at `apply`); the "CI example" (show `plan --strict` and
  its extra exit condition); the admin-gated function list (l.15-17), add `check_view_merge`.
- `mkdocs/docs/admin/functions-reference.md`: `check_view_merge(ddl)` entry (admin, output columns).
- `authorization.md`, `authentication.md`: admin-gated function count "nine" becomes "ten"; list
  `check_view_merge`.
- `materialized-views.md`: next to the `merge_sort_order` option, state the authoring rule (the merge
  query must not need an input order beyond `merge_sort_order`) and point to the check.
- `CHANGELOG.md`: new function and CLI flag; **Minor breaking change**: DDL parser types moved from
  `micromegas::servers::view_ddl` to `micromegas_analytics::lakehouse::view_ddl`; widened signatures
  of `register_functions` and `register_lakehouse_functions` (new `configurator` parameter).

## Testing Strategy

No-DB unit tests (lazy pool / `MemTable`-free planning; the check plans over synthetic partitions
and never reads objects):
- `check_merge_streaming` reports `streams = true` for a merge query whose ordered `GROUP BY`
  covers `merge_sort_order`, and `streams = false` with a `SortExec` line for
  `flatten(array_agg(items ORDER BY first_time))` under `merge_sort_order = 'k1, k2'` (the
  scenario from the issue).
- A definition without `merge_sort_order` yields no report.
- UDTF output schema/rows via a canned DDL; non-admin rejection joins `lakehouse_admin_gate_test.rs`.
- Existing `sql_batch_view_merge_ordering_tests.rs` must keep passing unchanged (guards the refactor).
- Python (`test_views.py`): warning rendering, `--strict` exit codes for warn / skipped-check, and
  SQL literal escaping, with a stubbed client. `FakeClient` routes SQL containing `check_view_merge(`
  to a per-view `check_results` / `check_errors` mapping ahead of the generic SELECT branch (default:
  empty report-shaped frame); `make_args` defaults gain `"strict": False`. Existing tests need these
  updates.

## Manual Verification

1. Start services (`python3 local_test_env/ai_scripts/start_services.py`), put the issue's example
   view in a views directory, run `micromegas-views plan --strict`; expect a warning naming the
   `SortExec` line and a non-zero exit. The end-to-end wiring (client → Flight SQL → UDTF) is not
   covered by the unit tests above.
