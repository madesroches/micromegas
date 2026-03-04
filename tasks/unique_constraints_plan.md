# Unique Constraints for processes, streams, and blocks — Plan

Closes #690, #888, #889.

## Overview

Add database-level UNIQUE constraints on `processes.process_id`, `streams.stream_id`, and `blocks.block_id`, then switch all INSERT statements to use `ON CONFLICT DO NOTHING` instead of the current `WHERE NOT EXISTS` subquery pattern. This closes the race condition window and gives guaranteed data integrity at the database level.

## Current State

### Schema (`rust/ingestion/src/sql_telemetry_db.rs`)
Tables have non-unique indexes:
```sql
CREATE INDEX process_id ON processes(process_id);
CREATE INDEX stream_id ON streams(stream_id);
CREATE INDEX block_id ON blocks(block_id);
```

### Inserts — ingestion (`rust/ingestion/src/web_ingestion_service.rs`)
Uses `INSERT INTO ... SELECT ... WHERE NOT EXISTS`:
```sql
INSERT INTO blocks SELECT $1,... WHERE NOT EXISTS (SELECT 1 FROM blocks WHERE block_id = $1);
```
Works but has a small race window under concurrent inserts.

### Inserts — replication (`rust/analytics/src/replication.rs`)
Uses plain `INSERT INTO ... VALUES(...)` with no duplicate protection. Will hard-fail once unique indexes exist.

### Migration system (`rust/ingestion/src/sql_migration.rs`)
Sequential version-based migrations (currently at v2). Runs inside transactions with advisory lock. `CREATE UNIQUE INDEX CONCURRENTLY` cannot run inside a transaction — needs special handling.

### Cleanup UDFs
`delete_duplicate_blocks_udf.rs`, `delete_duplicate_streams_udf.rs`, `delete_duplicate_processes_udf.rs` exist in `rust/analytics/src/lakehouse/` and are registered in the maintenance daemon.

## Design

### Migration to v3

`CREATE UNIQUE INDEX CONCURRENTLY` cannot run inside a transaction. The v2→v3 migration must:
1. Run the three `CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS` statements outside any transaction
2. Drop old non-unique indexes and bump version to 3 (inside transaction)
3. The `IF NOT EXISTS` clause makes this idempotent — safe if indexes were created manually ahead of time (as documented in CHANGELOG)

For brand-new databases (v0 → v3), the normal migration chain runs: v0→v1 creates tables with non-unique indexes, v1→v2 adds time columns, v2→v3 replaces indexes with unique ones. No special handling needed — creating and dropping indexes on empty tables is instant.

### INSERT changes

Replace `WHERE NOT EXISTS` and plain `VALUES` with `ON CONFLICT DO NOTHING`:

```sql
INSERT INTO blocks VALUES($1,$2,...,$11) ON CONFLICT (block_id) DO NOTHING;
INSERT INTO streams VALUES($1,$2,...,$7) ON CONFLICT (stream_id) DO NOTHING;
INSERT INTO processes VALUES($1,$2,...,$13) ON CONFLICT (process_id) DO NOTHING;
```

This is simpler, atomic, and eliminates the race condition.

### Drop old non-unique indexes

The unique indexes supersede the old non-unique indexes on the same columns. Drop them in the v3 migration:
```sql
DROP INDEX IF EXISTS process_id;
DROP INDEX IF EXISTS stream_id;
DROP INDEX IF EXISTS block_id;
```

## Implementation Steps

### Step 1: Add v3 migration
**File:** `rust/ingestion/src/sql_migration.rs`

- Bump `LATEST_DATA_LAKE_SCHEMA_VERSION` to 3
- Add a pre-transaction step in `execute_migration` (when `current_version == 2`) that runs the three `CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS` statements directly on the pool (not inside a transaction):
  ```sql
  CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS processes_process_id_unique ON processes(process_id);
  CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS streams_stream_id_unique ON streams(stream_id);
  CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS blocks_block_id_unique ON blocks(block_id);
  ```
- Add `upgrade_data_lake_schema_v3` that:
  - Drops old non-unique indexes
  - Bumps version to 3 (inside transaction)
- Update `execute_migration` to call v3 upgrade when `current_version == 2`

### Step 2: Switch ingestion to ON CONFLICT
**File:** `rust/ingestion/src/web_ingestion_service.rs`

Replace the three `INSERT ... SELECT ... WHERE NOT EXISTS` queries with `INSERT INTO ... VALUES(...) ON CONFLICT (block_id/stream_id/process_id) DO NOTHING`. Keep the `rows_affected() == 0` duplicate logging.

### Step 3: Switch replication to ON CONFLICT
**File:** `rust/analytics/src/replication.rs`

Replace the three plain `INSERT INTO ... VALUES(...)` queries with `INSERT INTO ... VALUES(...) ON CONFLICT (block_id/stream_id/process_id) DO NOTHING`.

### Step 4: Remove delete_duplicate UDFs
The constraint prevents duplicates at the database level, making these UDFs unnecessary. Remove:
- `rust/analytics/src/lakehouse/delete_duplicate_blocks_udf.rs`
- `rust/analytics/src/lakehouse/delete_duplicate_streams_udf.rs`
- `rust/analytics/src/lakehouse/delete_duplicate_processes_udf.rs`
- Their registrations in `rust/analytics/src/lakehouse/mod.rs` and `rust/analytics/src/lakehouse/query.rs`
- Their invocations in `rust/public/src/servers/maintenance.rs`

No references to these UDFs exist in the Python API — nothing to clean up there.

## Files to Modify

| File | Change |
|------|--------|
| `rust/ingestion/src/sql_migration.rs` | v3 migration, bump version constant, CONCURRENTLY index creation |
| `rust/ingestion/src/web_ingestion_service.rs` | `ON CONFLICT DO NOTHING` |
| `rust/analytics/src/replication.rs` | `ON CONFLICT DO NOTHING` |
| `rust/analytics/src/lakehouse/delete_duplicate_blocks_udf.rs` | Delete |
| `rust/analytics/src/lakehouse/delete_duplicate_streams_udf.rs` | Delete |
| `rust/analytics/src/lakehouse/delete_duplicate_processes_udf.rs` | Delete |
| `rust/analytics/src/lakehouse/mod.rs` | Remove UDF registrations |
| `rust/analytics/src/lakehouse/query.rs` | Remove UDF registrations |
| `rust/public/src/servers/maintenance.rs` | Remove UDF invocations |

## Upgrade Path for Existing Deployments

Already documented in `CHANGELOG.md` under Unreleased:

1. Run `delete_duplicate_*()` UDFs to clean existing duplicates
2. Verify no duplicates remain with `SELECT id, COUNT(*) ... HAVING COUNT(*) > 1`
3. Optionally run the `CREATE UNIQUE INDEX CONCURRENTLY` SQL manually ahead of time
4. Deploy new code — migration handles everything, `IF NOT EXISTS` makes pre-applied indexes a no-op

## Trade-offs

**ON CONFLICT vs WHERE NOT EXISTS**: `ON CONFLICT` is atomic (no race window), slightly simpler SQL, and standard PostgreSQL idiom for upsert/skip. Requires a unique constraint to exist, which is the whole point.

**Dropping old indexes**: The unique indexes serve the same purpose as the old non-unique ones. Keeping both wastes write I/O. Dropping them in migration is safe because the unique index covers the same queries.

**CONCURRENTLY outside transaction**: Slightly unusual migration pattern but necessary — PostgreSQL does not allow `CREATE INDEX CONCURRENTLY` inside a transaction. The `IF NOT EXISTS` makes it idempotent and safe for retries.

## Testing Strategy

- `cargo test` — existing tests should pass since ON CONFLICT is a behavioral superset of WHERE NOT EXISTS
- Manual test: insert a block twice via HTTP, verify second insert returns success (200) but doesn't create a duplicate row
- Manual test: run replication with a duplicate row, verify it succeeds silently
- Verify migration on fresh database (v0 → v3) and on existing database (v2 → v3)
- Verify `\di` in psql shows unique indexes after migration

## Open Questions

None.
