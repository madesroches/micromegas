# Validate Concurrent Indexes in v2→v3 Migration

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/911

## Overview

`CREATE UNIQUE INDEX CONCURRENTLY` can silently produce an `INVALID` index (e.g., due to duplicate rows or transient errors). The v2→v3 migration in `execute_migration` does not check for this, so it proceeds to bump the schema version to 3 and drop the old non-unique indexes. The `ON CONFLICT (block_id) DO NOTHING` queries then fail at runtime because there is no usable unique constraint.

## Current State

In `rust/ingestion/src/sql_migration.rs:95-117`, the v2→v3 migration:
1. Creates three unique indexes concurrently (outside a transaction, as required by PostgreSQL)
2. Immediately opens a transaction and calls `upgrade_data_lake_schema_v3` which drops the old non-unique indexes and bumps the version to 3

There is no validation between steps 1 and 2. If any `CREATE UNIQUE INDEX CONCURRENTLY` produces an INVALID index, the migration still proceeds.

Multiple call sites rely on these unique constraints via `ON CONFLICT`:
- `rust/ingestion/src/web_ingestion_service.rs:75,128,159`
- `rust/analytics/src/replication.rs:54,109,189`

## Design

After the three `CREATE UNIQUE INDEX CONCURRENTLY` statements and before opening the transaction for `upgrade_data_lake_schema_v3`, query `pg_index.indisvalid` for each index. If any index is invalid, drop it and return an error so the migration does not bump the version. The next startup will retry.

### Validation query

```sql
SELECT i.indisvalid
FROM pg_class c
JOIN pg_index i ON i.indexrelid = c.oid
WHERE c.relname = $1;
```

### On invalid index

1. Drop the invalid index: `DROP INDEX IF EXISTS <index_name>`
2. Return `Err(...)` with a descriptive message

The `IF NOT EXISTS` on the `CREATE` statements already makes the migration idempotent — on the next startup it will re-create the dropped index and re-validate.

## Implementation Steps

1. Add a helper function `check_index_is_valid(pool, index_name) -> Result<bool>` in `sql_migration.rs` that:
   - Queries `pg_class`/`pg_index` to check `indisvalid`
   - If the index doesn't exist, returns an error (unexpected state)
   - If `indisvalid` is false, drops the index and returns `Ok(false)`
   - If valid, returns `Ok(true)`

2. Add a function `validate_unique_indexes(pool) -> Result<()>` that:
   - Calls `check_index_is_valid` for all three indexes (`processes_process_id_unique`, `streams_stream_id_unique`, `blocks_block_id_unique`), collecting results
   - If any index was invalid (returned `false`), returns a single error listing all dropped indexes
   - This ensures all invalid indexes are dropped in one pass, so a single restart can recover them all

3. Call `validate_unique_indexes` after the `CREATE` statements and before the `upgrade_data_lake_schema_v3` transaction.

## Files to Modify

- `rust/ingestion/src/sql_migration.rs` — add validation helper and call it in `execute_migration`

## Trade-offs

**Alternative: wrap validation inside `upgrade_data_lake_schema_v3`**
Rejected because the validation needs to query the pool directly (not through the transaction) and potentially drop indexes outside a transaction. Keeping it in `execute_migration` alongside the `CREATE` statements is cleaner.

**Alternative: automatically retry index creation in a loop**
Rejected as over-engineering. The root cause (duplicate rows or transient errors) should be investigated by the operator. Returning an error and retrying on the next startup is sufficient.

## Testing Strategy

- Manual testing against a local PostgreSQL:
  1. Create a duplicate row in one of the tables
  2. Run the migration — verify it creates an INVALID index, detects it, drops it, and returns an error
  3. Remove the duplicate row and restart — verify the migration succeeds
- Unit testing is impractical here since this requires a real PostgreSQL with concurrent index support
