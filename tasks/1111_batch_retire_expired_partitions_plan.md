# Batch retire_expired_partitions Plan

Issue: [#1111](https://github.com/madesroches/micromegas/issues/1111)

## Overview

`retire_expired_partitions` fetches all expired `lakehouse_partitions` rows in a
single unbounded `fetch_all`, then inserts them all into `temporary_files` and
deletes them from `lakehouse_partitions` inside one long-lived transaction. On a
large backlog this can load unbounded memory, hold an open transaction for a long
time, and dump a large burst of rows into `temporary_files` that the subsequent
batched cleanup has to drain. The fix is to apply the same bounded-batch + loop
pattern already used by `delete_expired_blocks_batch` and
`delete_expired_temporary_files_batch` (#1110).

## Current State

- **`rust/analytics/src/lakehouse/write_partition.rs:70-108`** —
  `retire_expired_partitions(lake, expiration)`:
  - One `fetch_all` with no `LIMIT` — loads every expired partition into memory.
  - Loops over results calling `add_file_for_cleanup` (INSERT into `temporary_files`)
    inside the same transaction.
  - One unbounded `DELETE FROM lakehouse_partitions WHERE end_insert_time < $1`
    at the end of the same transaction.
  - Commit.

- **`rust/analytics/src/delete.rs:173-175`** — only call site:
  ```rust
  retire_expired_partitions(lake, expiration)
      .await
      .with_context(|| "retire_expired_partitions")?;
  ```
  Called from `delete_old_data`, which is invoked from `EveryHourTask` in
  `rust/public/src/servers/maintenance.rs:99`.

- **`rust/analytics/src/lakehouse/write_partition.rs:34-51`** —
  `add_file_for_cleanup(transaction, file_path, file_size)`: inserts one row into
  `temporary_files`; takes `&mut sqlx::Transaction`. Used inside the batch loop.

- **Schema** (`rust/analytics/src/lakehouse/migration.rs:103-120`) —
  `lakehouse_partitions` has no single-column primary key; the logical key is
  `(view_set_name, view_instance_id, begin_insert_time, end_insert_time)`.
  `file_path` is nullable (NULL for empty partitions that wrote no Parquet file).

- **Pattern to match** — `rust/analytics/src/delete.rs:12-53` and
  `rust/analytics/src/lakehouse/temp.rs:10-59`:
  - `_batch` function returns `Result<bool>` (true = more to process).
  - Internal `batch_size: i32 = 1000`.
  - Outer function drives a `while … {}` loop.

## Design

Split into the two-function form matching the established pattern:

```
#[span_fn]
async fn retire_expired_partitions_batch(lake, expiration) -> Result<bool>
pub async fn retire_expired_partitions(lake, expiration)   -> Result<()>   // loop
```

### Batch function

Use a single `DELETE … WHERE (…) IN (SELECT … LIMIT $2) RETURNING file_path,
file_size` inside a transaction. PostgreSQL supports row-tuple comparison in `IN`,
which lets us express the scoped delete in one round-trip:

```sql
DELETE FROM lakehouse_partitions
WHERE (view_set_name, view_instance_id, begin_insert_time, end_insert_time) IN (
    SELECT view_set_name, view_instance_id, begin_insert_time, end_insert_time
    FROM lakehouse_partitions
    WHERE end_insert_time < $1
    LIMIT $2
)
RETURNING file_path, file_size;
```

After collecting the returned rows:
- For each row where `file_path IS NOT NULL`, call `add_file_for_cleanup`.
- Commit the transaction.
- Return `rows.len() == batch_size as usize`.

Unlike `delete_expired_temporary_files_batch`, there are no S3 operations here —
the actual file deletion is deferred to `delete_expired_temporary_files` via the
`temporary_files` table. The transaction is therefore short and inexpensive.

Batch size: **1 000** — consistent with every other bounded-batch function in
this codebase. The batch is naturally more expensive than blocks (it writes to
`temporary_files` for each file-path row), so conservative sizing is correct.

Log a single `info!` line per batch: `"retired {count} expired partitions"`.

### Wrapper function

```rust
pub async fn retire_expired_partitions(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<()> {
    while retire_expired_partitions_batch(lake, expiration).await? {}
    Ok(())
}
```

The signature is unchanged — `delete.rs` needs no edits.

## Implementation Steps

1. Edit `rust/analytics/src/lakehouse/write_partition.rs`:
   - Add `retire_expired_partitions_batch(lake: &DataLakeConnection, expiration: DateTime<Utc>) -> Result<bool>` using the `DELETE … RETURNING` pattern above.
   - Annotate the new function with `#[span_fn]`.
   - Replace the body of `retire_expired_partitions` with a `while` loop that calls `retire_expired_partitions_batch`.
   - Remove the original `fetch_all` + loop + unbounded `DELETE` from `retire_expired_partitions`.

2. Run `cargo fmt` from `rust/`.

3. Run `cargo clippy --workspace -- -D warnings` from `rust/`.

4. Run `cargo test` from `rust/`.

## Files to Modify

- `rust/analytics/src/lakehouse/write_partition.rs` — only file to change.

## Trade-offs

- **DELETE+RETURNING with row-tuple subquery vs. SELECT then DELETE.**
  The subquery form selects and deletes atomically in one round-trip, preventing
  a concurrent maintenance run from processing the same rows twice. The
  non-transactional SELECT-then-DELETE pattern (used for blocks) is simpler but
  allows double-processing. The subquery form is consistent with the temp.rs
  fix and strictly better.

- **Keeping the function in `write_partition.rs` vs. moving it to `delete.rs`.**
  Moving it to `delete.rs` would co-locate all maintenance cleanup functions, but
  would require re-exporting or importing `add_file_for_cleanup`. Keeping it in
  `write_partition.rs` leaves the call graph unchanged and requires no edits to
  `delete.rs`.

- **Batch size 1 000.** Matches all other bounded-batch operations. Each batch
  issues at most a handful of `INSERT INTO temporary_files` statements — well
  within any transaction budget. Can be tuned later.

## Testing Strategy

- `cargo test` from `rust/` to confirm compilation and no regressions.
- Manual verification: start services via
  `local_test_env/ai_scripts/start_services.py`, insert rows into
  `lakehouse_partitions` with `end_insert_time` in the past, call
  `retire_expired_partitions`, confirm rows are removed from
  `lakehouse_partitions` and corresponding entries appear in `temporary_files`.
- No new unit test file needed: logic delegates entirely to `sqlx` and
  `add_file_for_cleanup`, which are independently tested.

## Open Questions

None — the fix is well-specified and mirrors the established pattern.
