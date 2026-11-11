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

## Design

### Fix

Move the `if 11 == current_version { … }` block from `migrate_lakehouse` to
`execute_lakehouse_migration`, after the `10 == current_version` step and before the final
assert. `migrate_lakehouse` then goes back to its pre-#1780 shape, the same as `migrate_db`.
As a result, the v12 step also runs under advisory lock `1` like every other step.

No change to `upgrade_v11_to_v12`, the version constant, or any SQL surface.

## Implementation Steps

1. In `rust/analytics/src/lakehouse/migration.rs`, move lines 49–55 into
   `execute_lakehouse_migration` after the v10 → v11 block.
2. From `rust/`: `cargo fmt`, then `cargo clippy --workspace --all-targets -- -D warnings`, then
   `cargo test -p micromegas-analytics`.
3. Verify manually on the local stack, as described under Manual Verification.

## Files to Modify

- `rust/analytics/src/lakehouse/migration.rs`

## Trade-offs

- **Table-driven step list with a no-DB unit test that it covers `0..LATEST`.** Rejected. It
  would make this class of bug unrepresentable, but it is a rewrite of a pattern shared with the
  ingestion migration, for a mistake a single local start would catch.

## Decisions

- No `CHANGELOG.md` entry: #1780 is unreleased, and the existing Unreleased line already
  describes the v12 behavior as it will ship.
- No live-DB regression test: the bug was caught on a dev deployment, not in the wild; the fix is
  verified manually on the local stack.

## Manual Verification

`migrate_lakehouse` runs when a lakehouse context is built: in `flight-sql-srv` (log
`/tmp/analytics.log`) and in the maintenance daemon (`/tmp/daemon.log`). The version is read from
Postgres with `(cd local_test_env/db && python3 connect.py)` (a `psql` session), running
`SELECT version FROM lakehouse_migration;`.

1. Upgrade from v11: bring up a local PostgreSQL whose lakehouse is at v11 by starting the
   services from `main` (before the fix) with `python3 local_test_env/ai_scripts/start_services.py`.
   They panic at the assert, which leaves the database at v11 (or reuse an existing local
   database already at v11). Stop them with `python3 local_test_env/ai_scripts/stop_services.py`
   without wiping the database.
2. On the fix branch, run `start_services.py` again. This runs the v11 → v12 step. Confirm there
   is no panic in `/tmp/analytics.log` / `/tmp/daemon.log`, that the `lakehouse_migration` query
   returns 12, and that the constraint is gone:
   `SELECT is_nullable FROM information_schema.columns WHERE table_name = 'lakehouse_view_set_definitions' AND column_name = 'update_group';`
   returns `YES` (it was `NO` at v11).
3. Restart the services again. The database is already at v12, so no step should run: no panic,
   version still 12.
4. Fresh database: run `stop_services.py`, then `docker stop teledb`, then
   `(cd local_test_env/db && python3 delete.py)`, then `start_services.py`, which creates a new empty
   `teledb`. Confirm it reaches version 12 without a panic and `update_group` is nullable. This destroys the local database;
   `local_test_env/db/dump.py <file>` can back it up first.
