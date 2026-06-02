# Temporary File Delete Batching Plan

Issue: [#1108](https://github.com/madesroches/micromegas/issues/1108)

## Overview

`delete_expired_temporary_files` processes all expired files in a single
unbounded operation. Under production load the `temporary_files` table can hold
hundreds of thousands of entries, causing a slow SQL DELETE (~162 s observed),
S3 `SlowDown` 503 errors, and a self-worsening retry loop because the rolled-back
transaction leaves all expired rows for the next maintenance cycle. Apply the
same bounded-batch + loop pattern already used by `delete_expired_blocks_batch`.

## Current State

- **`rust/analytics/src/lakehouse/temp.rs:11-40`** — one transaction, one
  unbounded `DELETE … RETURNING` over the entire `temporary_files` table.
  - `delete_partition_metadata_batch` issues `DELETE … WHERE file_path = ANY($1)` with
    the full (potentially 500 k+) array (see `partition_metadata.rs:153-168`).
  - `BlobStorage::delete_batch` uses `.try_chunks(1_000).buffered(20)` — 500 k
    files → 500 concurrent `DeleteObjects` requests → S3 `SlowDown`.
  - S3 error propagates before `tr.commit()`, transaction rolls back, rows
    remain, next cycle is even larger.
  - Per-file `info!("deleting expired file {file_path}")` produces hundreds of
    thousands of log lines per cycle — itself a performance problem.

- **Pattern to match** — `rust/analytics/src/delete.rs:12-53`:
  - `delete_expired_blocks_batch` returns `Result<bool>` (true = more to do).
  - Bounded by `batch_size: i32 = 1000`.
  - Outer `delete_expired_blocks` drives a `while … {}` loop.

- **Callers** (`delete_expired_temporary_files`):
  - `rust/public/src/servers/maintenance.rs:100` (hourly maintenance task).
  - `rust/telemetry-admin-cli/src/telemetry_admin.rs:74` (admin CLI).

## Design

Replace the single-pass implementation with the two-function split established by
`delete_expired_blocks`:

```
pub async fn delete_expired_temporary_files_batch(lake, now) -> Result<bool>
pub async fn delete_expired_temporary_files(lake)             -> Result<()>
```

### Batch function

Use a single `DELETE … WHERE file_path IN (SELECT … LIMIT $2) RETURNING file_path`
within a transaction. This atomically selects and removes the batch from the DB,
preventing a concurrent cycle from picking up the same rows.

The partition metadata and S3 deletes happen before `tr.commit()`, preserving
the existing ordering guarantee: if S3 fails the transaction rolls back and the
rows stay for the next attempt. The required order within the transaction is:
(1) `DELETE … RETURNING` to collect file paths, (2) `delete_partition_metadata_batch`,
(3) `blob_storage.delete_batch`, (4) `tr.commit()` — same ordering as the current
code. S3 is not transactional; `tr.commit()` must not be called until after
`blob_storage.delete_batch` succeeds, so that a failed S3 delete leaves the DB
rows intact for the next retry.

Batch size: **1 000** — consistent with `delete_expired_blocks_batch` and all
other bounded-batch operations in `delete.rs`. At 1 000 files,
`delete_batch` produces at most 1 S3 `DeleteObjects` request, well within
S3's rate limits.

Log a single summary line per batch (`info!("deleted {count} expired temporary files")`).
Remove the per-file `info!` log.

### Wrapper function

```rust
pub async fn delete_expired_temporary_files(lake: Arc<DataLakeConnection>) -> Result<()> {
    let now = Utc::now();
    while delete_expired_temporary_files_batch(&lake, now).await? {}
    Ok(())
}
```

Capturing `now` once outside the loop ensures that rows added during a large
backfill cleanup are not swept up in the same maintenance run (they'll expire in
a future cycle).

## Implementation Steps

1. Edit `rust/analytics/src/lakehouse/temp.rs`:
   - Add a `delete_expired_temporary_files_batch(lake: &DataLakeConnection, now: DateTime<Utc>) -> Result<bool>` function implementing the bounded loop body.
   - Replace the body of `delete_expired_temporary_files` with a `while` loop that calls `delete_expired_temporary_files_batch`.
   - Remove the per-file `info!` log; replace with a single `info!("deleted {count} expired temporary files")` per batch.
   - Add necessary imports (`DateTime`).

2. Run `cargo fmt` and `cargo clippy --workspace -- -D warnings` from `rust/`.

3. Run `cargo test` to confirm no regressions.

## Files to Modify

- `rust/analytics/src/lakehouse/temp.rs` — only file to change.

## Trade-offs

- **DELETE…RETURNING with subquery vs. SELECT then DELETE (blocks pattern).**
  The subquery form atomically selects and deletes in one round-trip, preventing
  a second concurrent worker from picking up the same rows. The blocks pattern
  uses a non-transactional SELECT then a separate DELETE — simpler but subject to
  double-processing. The subquery form is strictly better and consistent with what
  the issue proposes.

- **Batch size 1 000 vs. 10 000 (issue suggestion).** The issue suggests 10 000
  to match 10 concurrent S3 DeleteObjects requests. However, 1 000 is already
  safe (1 request per batch), matches every other bounded-batch function in this
  codebase, and keeps individual SQL transactions short. A larger batch can be
  tuned later if needed; starting conservative avoids re-introducing the original
  problem in a milder form.

- **Capture `now` outside vs. inside the loop.** Outside: rows inserted after
  the maintenance cycle starts are deferred to the next run. Inside: the window
  grows, potentially running forever during a large backfill. Outside is safer.

- **Two-function split vs. internal loop (issue suggestion).** The issue shows a
  single function with an internal loop. The two-function split matches the existing
  blocks/streams/processes pattern, is slightly more testable, and is consistent.

## Testing Strategy

- `cargo test` from `rust/` — confirms compilation and any existing tests pass.
- Manual verification: start services via `local_test_env/ai_scripts/start_services.py`,
  insert rows into `temporary_files` with expired `expiration` timestamps, run
  `delete_expired_temporary_files`, confirm rows are removed and no errors are
  logged.
- No new unit test file needed: the function delegates entirely to
  well-tested primitives (`sqlx`, `delete_partition_metadata_batch`,
  `blob_storage.delete_batch`).

## Open Questions

None — the fix is well-specified and matches an established pattern.
