# Optimize delete_expired_blocks_batch and Related Cleanup Queries Plan

Issue: [#1116](https://github.com/madesroches/micromegas/issues/1116)

## Overview

`delete_expired_blocks_batch` uses two un-transacted round-trips to delete a batch
of expired blocks: a `SELECT` followed by `DELETE … WHERE block_id = ANY($1)` with
1 000 UUIDs. The array-form DELETE can fall back to a sequential scan as the table
grows, hitting the slow-query threshold. The fix wraps the operation in a
transaction using `SELECT … FOR UPDATE SKIP LOCKED` to lock the batch, deletes S3
blobs, then deletes the locked rows — a pattern that is also safe for concurrent
workers. The same commit also converts `delete_empty_streams_batch` and
`delete_empty_processes_batch` from SELECT+DELETE pairs to single-statement
`DELETE … RETURNING` operations (already annotated with `// delete returning would
be more efficient` comments).

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
- Not safe for concurrent maintenance workers (double-processing possible).

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

Comments at lines 65–66: `// delete returning would be more efficient` and a note
to replace the GROUP BY with a NOT EXISTS pattern matching `delete_empty_streams_batch`.

## Design

### `delete_expired_blocks_batch`

Wrap the entire operation in a transaction. Use `SELECT … FOR UPDATE SKIP LOCKED`
to atomically reserve the batch, preventing concurrent workers from selecting the
same rows:

```sql
-- inside a sqlx transaction
SELECT block_id, process_id, stream_id
FROM   blocks
WHERE  insert_time <= $1
LIMIT  $2
FOR UPDATE SKIP LOCKED;

-- Rust: build paths from returned rows, call lake.blob_storage.delete_batch(...)

DELETE FROM blocks WHERE block_id = ANY($1);  -- $1 = locked_ids Vec<Uuid>

-- transaction.commit()
```

Failure modes (unchanged from issue spec):
- S3 delete fails → don't commit → DB unchanged → retry next run.
- Crash after S3 but before COMMIT → transaction rolls back → next run re-selects
  same rows → S3 returns "not found" (idempotent) → DB delete succeeds cleanly.

`FOR UPDATE SKIP LOCKED` ensures a second concurrent worker will skip locked rows
rather than block, making the design safe for concurrent execution.

The `DELETE … WHERE block_id = ANY($1)` with a 1 000-element array still relies
on the planner choosing the index on `block_id`. If that remains problematic after
this change, a follow-up could use a CTE join:
```sql
WITH locked AS (…)
DELETE FROM blocks USING locked WHERE blocks.block_id = locked.block_id
```
That is out of scope here; the transaction + SKIP LOCKED change is the primary fix.

### `delete_empty_streams_batch`

Replace the SELECT+DELETE pair with a single `DELETE … RETURNING` inside a
transaction, eliminating the double-round-trip and the double-processing risk:

```sql
DELETE FROM streams
WHERE stream_id IN (
    SELECT stream_id
    FROM   streams
    WHERE  insert_time <= $1
    AND    NOT EXISTS (
        SELECT 1 FROM blocks WHERE blocks.stream_id = streams.stream_id LIMIT 1
    )
    LIMIT  $2
    FOR UPDATE SKIP LOCKED
)
RETURNING stream_id;
```

Note: PostgreSQL does not allow `FOR UPDATE` in a subquery of `DELETE` directly,
but it is allowed in a nested sub-select (the inner `SELECT … FOR UPDATE SKIP
LOCKED` within the `IN (…)` expression is valid). If the planner doesn't push
the lock down, an alternative using a CTE is:

```sql
WITH locked AS (
    SELECT stream_id FROM streams
    WHERE  insert_time <= $1
    AND    NOT EXISTS (SELECT 1 FROM blocks WHERE blocks.stream_id = streams.stream_id LIMIT 1)
    LIMIT  $2
    FOR UPDATE SKIP LOCKED
)
DELETE FROM streams
WHERE stream_id IN (SELECT stream_id FROM locked)
RETURNING stream_id;
```

Use whichever form sqlx accepts cleanly. The CTE form is preferred as it makes
the intent explicit and is unambiguously valid.

The `stream_ids` variable becomes unnecessary (rows come back from RETURNING).
Log a single `info!` line with the count.

### `delete_empty_processes_batch`

Replace the inefficient `LEFT JOIN … GROUP BY … HAVING count = 0` pattern with
`NOT EXISTS` (matching `delete_empty_streams_batch`), and convert to
`DELETE … RETURNING`:

```sql
WITH locked AS (
    SELECT process_id FROM processes
    WHERE  insert_time <= $1
    AND    NOT EXISTS (
        SELECT 1 FROM streams WHERE streams.process_id = processes.process_id LIMIT 1
    )
    LIMIT  $2
    FOR UPDATE SKIP LOCKED
)
DELETE FROM processes
WHERE process_id IN (SELECT process_id FROM locked)
RETURNING process_id;
```

## Implementation Steps

1. **Edit `rust/analytics/src/delete.rs`**:

   a. `delete_expired_blocks_batch`:
      - Add `let mut transaction = lake.db_pool.begin().await?;`
      - Change the SELECT query to append `FOR UPDATE SKIP LOCKED` and execute
        against `&mut *transaction`.
      - Keep the path/block_id extraction loop unchanged.
      - Keep `lake.blob_storage.delete_batch(&paths).await?;` (no transaction
        needed for S3 — it is not transactional).
      - Change the DELETE to execute against `&mut *transaction`.
      - Add `transaction.commit().await.with_context(|| "commit")?;`
      - Early-return `Ok(false)` when `paths.is_empty()` (before starting the
        transaction would also work; either is fine since the SELECT would find
        nothing anyway).

   b. `delete_empty_streams_batch`:
      - Replace the SELECT + DELETE pair with a single CTE-based
        `DELETE … RETURNING` statement executed against `&lake.db_pool` (no
        transaction object needed since it is one statement) or wrapped in a
        short transaction for consistency.
      - Collect returned `stream_id` values from RETURNING to compute the count.
      - Replace the two-line log with `info!("deleted {count} empty streams")`.

   c. `delete_empty_processes_batch`:
      - Replace the LEFT JOIN + GROUP BY query with a `NOT EXISTS` subquery.
      - Apply the same CTE + `DELETE … RETURNING` pattern as streams.
      - Replace the two-line log with `info!("deleted {count} empty processes")`.

2. **Run `cargo fmt`** from `rust/`.

3. **Run `cargo clippy --workspace -- -D warnings`** from `rust/`.

4. **Run `cargo test`** from `rust/`.

## Files to Modify

- `rust/analytics/src/delete.rs` — only file to change.

## Trade-offs

- **SELECT FOR UPDATE SKIP LOCKED vs. SELECT + DELETE (no transaction).**
  The transaction approach is strictly better: it is atomic, prevents
  double-processing by concurrent workers, and makes crash recovery clean. The
  only downside is that the transaction holds locks while S3 deletes run (~100 ms
  for 1 000 blobs), which is acceptable.

- **DELETE RETURNING vs. SELECT + DELETE for streams/processes.**
  Single-statement `DELETE … RETURNING` is one round-trip instead of two,
  eliminates the concurrent double-processing window, and removes the intermediate
  UUID array. No downside.

- **NOT EXISTS vs. LEFT JOIN + GROUP BY for processes.**
  `NOT EXISTS` short-circuits on the first matching row, which is more efficient
  than aggregating all stream rows per process. The existing streams function
  already uses this pattern; aligning processes is a consistency fix.

- **Batch size stays at 1 000.** No reason to change it; all existing
  bounded-batch operations in this codebase use 1 000.

- **CTE form vs. nested subquery for SKIP LOCKED.**
  PostgreSQL technically allows `FOR UPDATE SKIP LOCKED` in a subquery used by
  `DELETE … WHERE x IN (SELECT … FOR UPDATE SKIP LOCKED)`, but the CTE form
  (`WITH locked AS (SELECT … FOR UPDATE SKIP LOCKED) DELETE … WHERE x IN (SELECT
  x FROM locked)`) is unambiguously valid per the PostgreSQL docs and makes the
  intent clearer.

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
