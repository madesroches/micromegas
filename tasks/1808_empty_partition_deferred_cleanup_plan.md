# Remove Synchronous S3 DELETE from the Empty-Partition Write Path Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1808

## Overview

`finalize_partition_write` issues an inline `object_store.delete` (`delete_empty_file` span)
whenever a partition turns out empty. That DELETE sits on the materialization critical path, and
with frequent empty partitions it shows up as one of the largest async self-time spans in the
maintenance daemon (~118 ms per call). This plan removes it: skip the cleanup entirely when no
byte was ever handed to the object-store writer, and otherwise register the path in
`temporary_files` so the existing batched cleanup deletes it later.

## Current State

`rust/analytics/src/lakehouse/write_partition.rs`:

- `write_partition_file` (~line 1136) builds an `AsyncParquetWriter` around a
  `BufWriter`, with a shared `byte_counter: Arc<AtomicI64>`.
  `AsyncParquetWriter::write` (`async_parquet_writer.rs`) increments the counter **before** handing
  the bytes to `BufWriter::put`, so the counter is an upper bound of what reached the store: if it
  is 0, nothing for this path was ever handed to the object store.
- `finalize_partition_write` (~line 971) has three cleanup sites:
  1. `event_time_range == None` (no row set received — the common case): drops the writer and
     calls `object_store.delete` inside `instrument_named!(…, "delete_empty_file")`, ignoring the
     result.
  2. `close()` succeeded but `num_rows == 0`: the file was uploaded; it is deleted inline
     (`delete_empty_file`), warning on failure.
  3. `close()` failed: deletes the partial file inline, then returns the error.
- `write_partition_file` also deletes inline when `write_rows_and_track_times` fails.
- `add_file_for_cleanup(transaction, path, size)` (~line 30) inserts into `temporary_files` with a
  1-hour expiration. Callers: `retire_expired_partitions_batch` and `retire_partitions` (same file),
  `retire_partition_by_file_udf.rs:86`, `retire_partition_by_metadata_udf.rs:146` — all inside an
  existing transaction.
- `temp.rs::delete_expired_temporary_files` drains expired rows in batches of 1000 and deletes them
  with `blob_storage.delete_batch`; the maintenance daemon runs it
  (`public/src/servers/maintenance.rs:319`).
- `write_partition_file` has two consumers: `write_partition_from_rows` → `insert_partition`, and
  `SqlPartitionSpec::write_file` → `insert_partitions_replacing`.

## Design

### 1. Skip cleanup when no byte was handed to the store

In `finalize_partition_write`, for cases 1 and 2, read
`byte_counter.load(Ordering::Relaxed)`. If it is 0, do nothing: no backend can have created the
object. This checks our own writer, not `AsyncArrowWriter`/`BufWriter` buffering internals, so a
future eager-writing parquet version just makes the counter non-zero and falls through to step 2.

In practice case 1 always has a 0 counter today, and case 2 always has a non-zero one (`close()`
writes the header and footer).

### 2. Otherwise, defer the delete through `temporary_files`

When the counter is non-zero, register the path with `add_file_for_cleanup(path, bytes)` instead
of deleting inline. The INSERT runs on the pool as its own autocommit statement — not inside the
partition-insert transaction (see Trade-offs).

To allow that without a hand-rolled transaction, generalize `add_file_for_cleanup` to take any
Postgres executor:

```rust
pub async fn add_file_for_cleanup<'c, E>(executor: E, file_path: &str, file_size: i64) -> Result<()>
where
    E: sqlx::PgExecutor<'c>,
```

Existing callers change from `add_file_for_cleanup(transaction, …)` /
`add_file_for_cleanup(&mut transaction, …)` to `add_file_for_cleanup(&mut **transaction, …)` /
`add_file_for_cleanup(&mut *transaction, …)`; the new call site passes `&lake.db_pool`.

A failure to register propagates as an error (with the existing `adding file … to temporary
files for cleanup` context), failing the write. A PG failure here means the following
`insert_partition` would fail too, and propagating keeps the cleanup crash-safe rather than
silently leaking the object.

### 3. Signature change

`finalize_partition_write` takes `db_pool: &sqlx::PgPool` (or `&DataLakeConnection`) in place of
`object_store: Arc<dyn ObjectStore>` for cases 1–2; case 3 still needs the object store, so pass
`&DataLakeConnection` and derive both from it (`lake.db_pool`, `lake.blob_storage.inner()`).

### 4. Unchanged

- Case 3 (`close()` failed) and the `write_rows_and_track_times` error path keep their inline
  delete: rare error paths, and no transaction exists there.
- The `delete_empty_file` span disappears; the deferred path is visible through the existing
  `sql_insert_temporary_file` span.

## Implementation Steps

1. `write_partition.rs`: make `add_file_for_cleanup` generic over `sqlx::PgExecutor<'c>`; update
   its two in-file callers.
2. `retire_partition_by_file_udf.rs`, `retire_partition_by_metadata_udf.rs`: update the call to
   pass `&mut **transaction`.
3. `write_partition.rs`: change `finalize_partition_write` to take `lake: &DataLakeConnection`;
   replace the case-1 and case-2 inline deletes with a shared private helper, e.g.
   `release_empty_file(lake, &file_path, &byte_counter)`, that returns early when the counter is 0
   and otherwise calls `add_file_for_cleanup(&lake.db_pool, …)`. Keep case 3 as-is (using
   `lake.blob_storage.inner()`).
4. `write_partition_file`: pass `lake` to `finalize_partition_write`.
5. Add tests (below), run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test -p
   micromegas-analytics`.
6. `CHANGELOG.md`: note the change (unreleased section).

## Files to Modify

- `rust/analytics/src/lakehouse/write_partition.rs`
- `rust/analytics/src/lakehouse/retire_partition_by_file_udf.rs`
- `rust/analytics/src/lakehouse/retire_partition_by_metadata_udf.rs`
- `rust/analytics/tests/write_partition_tests.rs`
- `CHANGELOG.md`

## Trade-offs

- **Standalone INSERT vs. joining the partition-insert transaction** (the issue's suggestion).
  Joining would need the cleanup path carried on `Partition` or alongside it through both
  consumers (`insert_partition` and `insert_partitions_replacing`), touching every `Partition`
  constructor. After step 1, the deferred path only fires for case 2 (row sets arrived but were
  all empty), which is uncommon; one extra autocommit INSERT there is far cheaper than the S3
  round trip it replaces, and still crash-safe. Not worth the plumbing.
- **Keep the inline delete for case 3** rather than a standalone transaction: rare error path,
  matching the issue's own allowance.
- **Propagate vs. warn on registration failure**: propagate — see Design §2.

## Decisions

## Documentation

No user-facing docs cover this path; only `CHANGELOG.md`.

## Testing Strategy

No-DB unit tests in `rust/analytics/tests/write_partition_tests.rs`, calling `write_partition_file`
with a `DataLakeConnection` built from `PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")`
(precedent: `merge_scan_partitioning_tests.rs:50`) and an `InMemory` blob store:

1. **No row set (case 1)**: drop the sender without sending anything. Expect `Ok(partition)` with
   `file_path == None`, `num_rows == 0`, and an empty `InMemory` listing. Because any PG query
   against the unreachable pool would error and registration errors propagate, success proves the
   common path issues neither a DELETE-worth of cleanup nor a PG round trip.
2. **Only empty row sets (case 2)**: send a `PartitionRowSet` with a 0-row batch. Expect `Err`
   whose chain contains `temporary files for cleanup` (the deferred path was taken), and the
   `InMemory` store still holds the uploaded object under `views/…` (no inline delete).

Existing `#[ignore]` DB tests are not extended (no live-DB test for a feature, per `CLAUDE.md`).

## Open Questions

None.
