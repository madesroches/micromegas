# Lakehouse v12 Migration Fix Plan

Issue: #1783

## Overview

The v11 → v12 lakehouse migration step added by #1780 can never run. It sits in
`migrate_lakehouse` after the call to `execute_lakehouse_migration`, but that function ends with
`assert_eq!(current_version, LATEST_LAKEHOUSE_SCHEMA_VERSION)`. Its step chain stops at v11, so
the assert compares 11 with 12 and panics first. Every service that builds a lakehouse context
against a database below v12 (that is, every database) crashes at startup. The fix moves the step
into the chain.

## Current State

`rust/analytics/src/lakehouse/migration.rs`:

- `LATEST_LAKEHOUSE_SCHEMA_VERSION = 12` (line 9).
- `migrate_lakehouse` (lines 31–58) takes advisory lock `1` inside the `if current_version !=
  LATEST` block, calls `execute_lakehouse_migration`, and then, outside that block and so after
  the lock transaction is dropped, runs `if 11 == current_version { upgrade_v11_to_v12 … }`
  (lines 49–55).
- `execute_lakehouse_migration` (lines 61–142) is the `if N == current_version` chain for
  v0 → v11, ending with the assert at line 140, which is the panic site.
- `upgrade_v11_to_v12` (lines 670–681) is correct as written:
  `ALTER COLUMN update_group DROP NOT NULL` plus the version bump.

The ingestion-side `migrate_db` / `execute_migration`
(`rust/ingestion/src/remote_data_lake.rs:23`, `rust/ingestion/src/sql_migration.rs`) has the
correct shape: every step is inside `execute_migration`, and `migrate_db` has nothing after the
lock block except its own assert. `migrate_lakehouse` had the same shape before #1780.

No database can be at v12 today. The v0 → v11 steps each commit, then the assert panics before
v12 runs, so every affected database is stuck at exactly v11. No data repair is needed, and the
fixed binary upgrades those databases normally on its next start.

Existing coverage: `rust/analytics/tests/query_deny_list_db_test.rs` calls `migrate_lakehouse`,
but it is `#[ignore]`d, runs against the shared dev schema, and was not run for #1780. There is
no test for migrating the lakehouse to the latest version.

## Design

### Fix

Move the `if 11 == current_version { … }` block from `migrate_lakehouse` to
`execute_lakehouse_migration`, after the `10 == current_version` step and before the final
assert. `migrate_lakehouse` then goes back to its pre-#1780 shape, the same as `migrate_db`.
As a result, the v12 step also runs under advisory lock `1` like every other step. Today it runs
after the lock transaction has been dropped, so two services starting at once could race on it.

No change to `upgrade_v11_to_v12`, the version constant, or any SQL surface.

### Regression test

New `rust/analytics/tests/lakehouse_migration_db_test.rs`. It is `#[ignore]`d and requires
`MICROMEGAS_SQL_CONNECTION_STRING`. It uses the throwaway, `search_path`-scoped schema pattern
from `rust/ingestion/tests/sql_migration_test.rs`: drop and create a dedicated schema, connect a
pool with `.max_connections(2)` (`migrate_lakehouse` holds lock 1 on one transaction while it
calls `pool.begin()` again) and `options([("search_path", SCHEMA)])`, run the body, then drop the
schema on Ok/Err. The schema is also dropped before it is created, so a panic, which skips the
final drop, is cleaned up by the next run.

The `search_path` must contain **only** the throwaway schema, not `public`. In the v0 case
`read_lakehouse_schema_version` would otherwise find `public.lakehouse_migration`, read the real
dev database's version, and the test would migrate (or skip) the wrong tables.

One test, **`migrates_fresh_lakehouse_to_latest`**: on an empty schema (v0), call
`migrate_lakehouse`. Assert that it returns `Ok`, that the version equals
`LATEST_LAKEHOUSE_SCHEMA_VERSION`, and that `lakehouse_view_set_definitions.update_group` is
nullable (`information_schema.columns.is_nullable = 'YES'`, filtered by `table_schema` equal to
the throwaway schema, since `search_path` does not scope `information_schema`). Then call `migrate_lakehouse` again
and assert it is still `Ok` (the idempotent restart path).

The test assumes the `btree_gist` extension is already installed, for example in `public` by a
prior lakehouse migration.

The doc comment names the bug the test pins: a step left outside `execute_lakehouse_migration`
panics at startup. It also says how to run it.

## Implementation Steps

1. Add `rust/analytics/tests/lakehouse_migration_db_test.rs` as designed above. Reuse
   `common::db_fixtures::ensure_telemetry_guard` as the other analytics DB tests do.
2. With local services' Postgres up, run `cargo test -p micromegas-analytics --test
   lakehouse_migration_db_test -- --ignored`. It must fail with the line-140 panic.
3. In `rust/analytics/src/lakehouse/migration.rs`, move lines 49–55 into
   `execute_lakehouse_migration` after the v10 → v11 block, then rerun the test. It must pass.
4. `cargo fmt`, then `cargo clippy --workspace --all-targets -- -D warnings`, then
   `cargo test -p micromegas-analytics`.

## Files to Modify

- `rust/analytics/src/lakehouse/migration.rs`
- `rust/analytics/tests/lakehouse_migration_db_test.rs` (new)

## Trade-offs

- **Table-driven step list with a no-DB unit test that it covers `0..LATEST`.** Rejected. It
  would make this class of bug unrepresentable, but it is a rewrite of a pattern shared with the
  ingestion migration, for a mistake the regression test already catches.

## Decisions

- No `CHANGELOG.md` entry: #1780 is unreleased, and the existing Unreleased line already
  describes the v12 behavior as it will ship.
- No repair path for v12 databases: none can exist, since the step was unreachable.

## Testing Strategy

- The `#[ignore]`d live-DB test above. The bug was seen in the wild (daemon crash at boot),
  and migrations are a silent-failure class the tier rule sends to automated coverage. Only a
  real Postgres runs the DDL chain and the version reads that the panic depends on. There is no
  Rust-side seam that reproduces it without a database.
- The existing `cargo test -p micromegas-analytics` suite as a no-regression check.

## Manual Verification

None beyond steps 2 and 3 of the implementation. Those steps are where the regression test proves it
catches the bug, by failing before the fix and passing after it.
