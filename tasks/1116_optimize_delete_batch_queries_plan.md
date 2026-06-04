# Optimize delete_expired_blocks_batch and Related Cleanup Queries Plan

Issue: [#1116](https://github.com/madesroches/micromegas/issues/1116)

## Overview

`delete_expired_blocks_batch` uses two un-transacted round-trips to delete a batch
of expired blocks: a `SELECT` followed by `DELETE … WHERE block_id = ANY($1)` with
1 000 UUIDs. The array-form DELETE can fall back to a sequential scan as the table
grows, hitting the slow-query threshold. The fix replaces both statements with a
single `DELETE FROM blocks WHERE block_id IN (SELECT block_id … LIMIT $2) RETURNING
process_id, stream_id, block_id` inside a transaction — matching the established
pattern in `temp.rs` (`delete_expired_temporary_files_batch`). The same commit also converts
`delete_empty_streams_batch` and `delete_empty_processes_batch` from SELECT+DELETE
pairs to single-statement `DELETE … RETURNING` operations (already annotated with
`// delete returning would be more efficient` comments).

## Current State

All three functions live in `rust/analytics/src/delete.rs`.

### `delete_expired_blocks_batch` (lines 13–45)

```
1. SELECT process_id, stream_id, block_id … WHERE insert_time <= $1 LIMIT 1000
2. lake.blob_storage.delete_batch(&paths)   -- S3 delete (outside any transaction)
3. DELETE FROM blocks WHERE block_id = ANY($1)  -- 1 000-element UUID array
```

Problems:
- No transaction: if the process crashes after S3 but before DB delete, S3 objects
  are gone but the DB rows remain, causing orphaned rows on the next run.
- `ANY(large_array)` can cause the planner to choose a sequential scan.
- Two round trips to the DB.

### `delete_empty_streams_batch` (lines 60–97)

```
1. SELECT streams.stream_id WHERE NOT EXISTS (blocks …) LIMIT 1000
2. DELETE FROM streams WHERE stream_id = ANY($1)
```

Comment at line 65: `// delete returning would be more efficient`

### `delete_empty_processes_batch` (lines 110–145)

```
1. SELECT processes.process_id LEFT JOIN streams … GROUP BY … HAVING count = 0 LIMIT 1000
2. DELETE FROM processes WHERE process_id = ANY($1)
```

Comments at lines 117–118: `// delete returning would be more efficient` and a note
to replace the GROUP BY with a NOT EXISTS pattern matching `delete_empty_streams_batch`.

## Design

### `delete_expired_blocks_batch`

Wrap the entire operation in a transaction. Use a single `DELETE … RETURNING`
statement to atomically remove the batch and retrieve the rows needed to build
S3 blob paths — the same transaction + S3-cleanup structure used by
`delete_expired_temporary_files_batch` in `temp.rs`. (Note: `temp.rs` returns
`Ok(true)` unconditionally when non-empty; this function instead returns
`Ok(paths.len() == batch_size as usize)`, matching `retire_expired_partitions_batch`
in `write_partition.rs` and the original blocks code.)

```sql
-- inside a sqlx transaction
DELETE FROM blocks
WHERE block_id IN (
    SELECT block_id FROM blocks WHERE insert_time <= $1 LIMIT $2
)
RETURNING process_id, stream_id, block_id;

-- Rust: build paths from RETURNING rows, call lake.blob_storage.delete_batch(...)

-- transaction.commit()
```

Failure modes:
- S3 delete fails → don't commit → DB unchanged → retry next run.
- Crash after S3 but before COMMIT → transaction rolls back → next run re-selects
  same rows → S3 returns "not found" (idempotent) → DB delete succeeds cleanly.

The subquery form lets the planner use the index on `insert_time` for the inner
scan and the primary key index for the outer delete, avoiding the sequential-scan
risk of `ANY(large_array)`.

### `delete_empty_streams_batch`

Replace the SELECT+DELETE pair with a single `DELETE … RETURNING` inside a
transaction, eliminating the double-round-trip and the double-processing risk:

```sql
WITH batch AS (
    SELECT stream_id FROM streams
    WHERE  insert_time <= $1
    AND    NOT EXISTS (SELECT 1 FROM blocks WHERE blocks.stream_id = streams.stream_id LIMIT 1)
    LIMIT  $2
)
DELETE FROM streams
WHERE stream_id IN (SELECT stream_id FROM batch)
RETURNING stream_id;
```

The `stream_ids` variable becomes unnecessary (rows come back from RETURNING).
Log a single `info!` line with the count.

### `delete_empty_processes_batch`

Replace the inefficient `LEFT JOIN … GROUP BY … HAVING count = 0` pattern with
`NOT EXISTS` (matching `delete_empty_streams_batch`), and convert to
`DELETE … RETURNING`:

```sql
WITH batch AS (
    SELECT process_id FROM processes
    WHERE  insert_time <= $1
    AND    NOT EXISTS (
        SELECT 1 FROM streams WHERE streams.process_id = processes.process_id LIMIT 1
    )
    LIMIT  $2
)
DELETE FROM processes
WHERE process_id IN (SELECT process_id FROM batch)
RETURNING process_id;
```

## Implementation Steps

1. **Edit `rust/analytics/src/delete.rs`**:

   a. `delete_expired_blocks_batch`:
      - Add `let mut transaction = lake.db_pool.begin().await?;`
      - Replace the SELECT + DELETE pair with a single
        `DELETE FROM blocks WHERE block_id IN (SELECT block_id FROM blocks WHERE
        insert_time <= $1 LIMIT $2) RETURNING process_id, stream_id, block_id`
        executed against `&mut *transaction`.
      - Build blob paths from the RETURNING rows.
      - Early-return `Ok(false)` (let the transaction drop — no commit needed) when
        no rows are returned.
      - Call `lake.blob_storage.delete_batch(&paths).await?;` (S3 is not
        transactional; the transaction guards the DB side).
      - Add `transaction.commit().await.with_context(|| "commit")?;`
      - Return `Ok(paths.len() == batch_size as usize)` to preserve the
        loop-continuation signal, matching steps 1b/1c and the original semantics.

   b. `delete_empty_streams_batch`:
      - Replace the SELECT + DELETE pair with a single CTE-based
        `DELETE … RETURNING` statement executed against `&lake.db_pool`. No
        transaction object is needed: it is one statement (inherently atomic in
        Postgres) and there is no S3 cleanup to coordinate.
      - Collect returned `stream_id` values from RETURNING to compute the count.
      - Return `Ok(count == batch_size as usize)` to preserve the loop-continuation
        signal, matching the existing pattern.
      - Replace the two-line log with `info!("deleted {count} empty streams")`.

   c. `delete_empty_processes_batch`:
      - Replace the LEFT JOIN + GROUP BY query with a `NOT EXISTS` subquery.
      - Apply the same CTE + `DELETE … RETURNING` pattern as streams.
      - Return `Ok(count == batch_size as usize)` to preserve the loop-continuation
        signal, matching the existing pattern.
      - Replace the two-line log with `info!("deleted {count} empty processes")`.

2. **Run `cargo fmt`** from `rust/`.

3. **Run `cargo clippy --workspace -- -D warnings`** from `rust/`.

4. **Run `cargo test`** from `rust/`.

## Files to Modify

- `rust/analytics/src/delete.rs` — only file to change.

## Trade-offs

- **DELETE … RETURNING vs. SELECT + DELETE (no transaction).**
  The `DELETE … RETURNING` approach matches the established pattern in `temp.rs`
  (`delete_expired_temporary_files_batch`). It is atomic (one round-trip), makes crash recovery
  clean, and avoids the sequential-scan risk of `ANY(large_array)`. The
  transaction holds the DB rows deleted until `commit()`, but S3 deletes happen
  before commit so a failure there leaves the DB unchanged for the next retry.
  No meaningful downside.

- **NOT EXISTS vs. LEFT JOIN + GROUP BY for processes.**
  `NOT EXISTS` short-circuits on the first matching row, which is more efficient
  than aggregating all stream rows per process. The existing streams function
  already uses this pattern; aligning processes is a consistency fix.

- **Batch size stays at 1 000.** No reason to change it; all existing
  bounded-batch operations in this codebase use 1 000.


## Testing Strategy

- `cargo test` from `rust/` to confirm compilation and no regressions.
- Manual verification: start services via
  `local_test_env/ai_scripts/start_services.py`, ingest some data, run the admin
  CLI or maintenance task with an expiration in the future (so all data qualifies),
  confirm blocks/streams/processes are removed and S3 paths are cleaned up.
- No new unit test file needed: the logic delegates entirely to `sqlx` and
  `blob_storage.delete_batch`, which are independently tested.

## Open Questions

None — the fix is well-specified by the issue and mirrors established patterns
in `write_partition.rs` (`retire_expired_partitions_batch`) and `temp.rs`
(`delete_expired_temporary_files_batch`).
