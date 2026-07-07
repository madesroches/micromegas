# Remove the `partition_metadata` Table Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1121

## Overview

`partition_metadata` stores a copy of each partition's Parquet footer in postgres purely as a
persistent cache so query-time metadata reads don't have to hit object storage. #1235 added a
read-side A/B knob (`MICROMEGAS_DISABLE_METADATA_PSQL_CACHE`) that let `ParquetReader` read the
footer directly from object storage instead; production comparison confirmed the two paths are
behaviorally identical and the object-cache-backed footer read is an acceptable replacement. This
plan finishes the retirement: make the object-storage footer read the only read path, delete the
`partition_metadata` table (migration), and remove everything that only existed to serve the
postgres path — the postgres `load_partition_metadata`, the write-path `INSERT`, batch deletes on
partition retirement, the knob itself, and the legacy-format (Arrow 56.0) compatibility shim that
only mattered for bytes stored in that table.

This directly fixes the two problems named in the issue: TOAST overhead on `DELETE` (~1.2s per
1000 rows, see #1108) and write-path overhead on every partition insert.

## Current State

### Read path (`rust/analytics/src/lakehouse/reader_factory.rs`, `partition_metadata.rs`)

`ReaderFactory` is DataFusion's `ParquetFileReaderFactory`. Each `ParquetReader` it creates holds a
`bypass_metadata_psql_cache: bool` (from `read_disable_metadata_psql_cache()`, env var
`MICROMEGAS_DISABLE_METADATA_PSQL_CACHE`) and branches in `get_metadata`:
- `false` (default): `load_partition_metadata(&pool, &filename, Some(&metadata_cache))`
  (`partition_metadata.rs:82`) — checks the in-process `MetadataCache` lookaside, then on miss
  `SELECT metadata, partition_format_version FROM partition_metadata WHERE file_path=$1`,
  dispatches to `metadata_compat::parse_legacy_and_upgrade` (v1/Arrow 56.0, needs an extra
  `SELECT num_rows FROM lakehouse_partitions`) or `parse_parquet_metadata` (v2/Arrow 57.0), then
  `strip_column_index_info`.
- `true`: `load_partition_metadata_from_footer(&mut inner, &filename, file_size,
  Some(&metadata_cache))` (`partition_metadata.rs:188`) — same `MetadataCache` lookaside, but on
  miss reads the footer directly from object storage via `CachingReader::get_bytes` (through
  `ParquetMetaDataReader::load_and_finish` + a `MetadataFetch` adapter) and applies the same
  `strip_column_index_info`.

Both paths insert into the same `MetadataCache` on a miss, so a warm cache behaves identically
either way; the knob only changes the miss-backfill source. This was validated against both v1 and
v2 partitions during #1235 — the direct footer read needs no format-version dispatch because it
parses the real on-disk footer, unlike the postgres path's stored (and once-relossy) serialized
copy.

### Write path (`rust/analytics/src/lakehouse/write_partition.rs:323-340`)

`insert_partition` (called `write_partition_from_rows` → advisory-locked transaction) does, inside
the same transaction as the `lakehouse_partitions` insert:
```rust
if let (Some(file_path), Some(metadata)) = (&partition.file_path, file_metadata) {
    let metadata_bytes = serialize_parquet_metadata(metadata)?;
    sqlx::query("INSERT INTO partition_metadata (file_path, metadata, insert_time, partition_format_version) VALUES ($1, $2, $3, 2)")
        ...
}
```

### Cleanup path (`rust/analytics/src/lakehouse/temp.rs`)

`delete_expired_temporary_files_batch` calls `delete_partition_metadata_batch(&mut tr, &to_delete)`
(`partition_metadata.rs:239`, `DELETE FROM partition_metadata WHERE file_path = ANY($1)`) for every
batch of expired temp files, before the batched blob-storage delete. Partitions are never deleted
directly — a retired partition's file first becomes a `temporary_files` row (`retire_partitions` in
`write_partition.rs`), and its `partition_metadata` row is deleted only once that temp file expires
and this batch job runs. This is one of the two call sites #1108 identified as slow due to TOAST.

### Dead code coupled to the table

- `partition_with_metadata` / `PartitionWithMetadata` (`partition_cache.rs:17-40`) call
  `load_partition_metadata(pool, file_path, None)` but have **zero callers anywhere in the repo**
  (confirmed by grep) — leftover from before the reader-factory cache existed.
- `metadata_compat::parse_legacy_and_upgrade` (`metadata_compat.rs`) exists only to fix up
  `num_rows` in the postgres-stored serialized bytes for v1 (Arrow 56.0) partitions; it is not
  needed to read an actual on-disk v1 footer (confirmed during #1235 testing). Once the postgres
  path is gone, it has no callers.
- `arrow_utils::serialize_parquet_metadata` is called only from the write-path `INSERT` above; once
  that's removed it has no production callers (still referenced by tests — see Testing Strategy).
- `read_disable_metadata_psql_cache()` and `bypass_metadata_psql_cache` fields become dead once
  there's only one read path.

### Schema (`rust/analytics/src/lakehouse/migration.rs`)

`LATEST_LAKEHOUSE_SCHEMA_VERSION = 5`. `partition_metadata` was created in `upgrade_v3_to_v4`
(`file_path VARCHAR(2047) PRIMARY KEY, metadata bytea NOT NULL, insert_time TIMESTAMPTZ NOT NULL`)
and gained `partition_format_version INTEGER NOT NULL DEFAULT 1` in `upgrade_v4_to_v5`.
`lakehouse_partitions.partition_format_version` (added in the same v4→v5 migration) is a separate
column, exposed read-only via `list_partitions()` (`list_partitions_table_function.rs`) — it is
**not** touched by this plan; it stays as informational metadata about how a partition was written,
independent of where its Parquet metadata is read from.

### Not affected

- `list_partitions()` / `ListPartitionsTableProvider` queries `lakehouse_partitions` only.
- `parse_parquet_metadata` (`arrow_utils.rs`) is also called from
  `migration.rs::populate_num_rows_column` (the v2→v3 migration, reading the old
  `lakehouse_partitions.file_metadata` column) — that call site is historical migration code and
  stays regardless of this change.
- `MICROMEGAS_METADATA_CACHE_MB` and the `MetadataCache` moka lookaside are unrelated to this issue
  (they cache *parsed* metadata in-process) and are untouched.

## Design

Make the object-storage footer read (`load_partition_metadata_from_footer`) the only partition
metadata read path, and delete everything that exists solely to serve the postgres path.

### Read path

- Rename `load_partition_metadata_from_footer` → `load_partition_metadata` in
  `partition_metadata.rs` (it's now the only loader; the old name loses its meaning). New
  signature stays `(reader: &mut CachingReader, file_path: &str, file_size: u64, cache: Option<&MetadataCache>) -> parquet::errors::Result<Arc<ParquetMetaData>>`.
- Delete the old postgres-backed `load_partition_metadata` (the `SELECT ... FROM partition_metadata`
  version) and `metadata_compat` module entirely.
- `ReaderFactory` / `ParquetReader`: drop `bypass_metadata_psql_cache` and `pool: PgPool` fields
  (nothing else in either struct needs `pool`). `get_metadata` calls the renamed
  `load_partition_metadata` unconditionally — no branch.
- Delete `read_disable_metadata_psql_cache()`.
- `ReaderFactory::new` drops the `pool: PgPool` and `bypass_metadata_psql_cache: bool` parameters.
  Update both call sites in `lakehouse_context.rs` (`LakehouseContext::new`, `::with_caches`) to
  match, and drop their `read_disable_metadata_psql_cache()` calls.

### Write path

Delete the `INSERT INTO partition_metadata` block in `write_partition.rs` (lines 323-340) — the
`lakehouse_partitions` insert (with `file_path`, `file_size`, `partition_format_version`) already
carries everything needed to locate and read the file later.

That block is the *only* consumer of the `file_metadata` plumbing in this file, so remove it too
or clippy `-D warnings` fails (`unused_variables` on the parameter, `dead_code` on the field):
- `insert_partition`'s `file_metadata: Option<&Arc<ParquetMetaData>>` parameter (line 254) and the
  `result.file_metadata.as_ref()` argument at its call site in `write_partition_from_rows`
  (line 662).
- `PartitionWriteResult.file_metadata: Option<Arc<ParquetMetaData>>` (line 405) and its
  initializations (`Some(Arc::new(parquet_metadata))` in the non-empty case, `None` in the two
  empty/error cases). `num_rows` is extracted from `parquet_metadata` before this, so the
  `arrow_writer.close()` result is still needed — only the `Arc::new` + field threading goes.
- The now-unused `parquet::file::metadata::ParquetMetaData` import (those two spots are its only
  uses in the file).

### Cleanup path

Delete `delete_partition_metadata_batch` (`partition_metadata.rs`) and its call in
`temp.rs::delete_expired_temporary_files_batch`. This is the direct fix for the TOAST-driven slow
`DELETE` in #1108/#1121: one fewer 1000-row batch delete against a `bytea`-column table per cleanup
pass.

### Dead code removal

- Delete `partition_with_metadata` / `PartitionWithMetadata` from `partition_cache.rs` (zero
  callers; its only reason to exist was calling the postgres loader with no object-store handle).
- Delete `metadata_compat.rs` and its module declaration in `mod.rs`.
- Delete `arrow_utils::serialize_parquet_metadata` (no remaining production caller).

### Schema migration

Add `upgrade_v5_to_v6` in `migration.rs`:
```rust
async fn upgrade_v5_to_v6(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    tr.execute("DROP TABLE partition_metadata;")
        .await
        .with_context(|| "dropping partition_metadata table")?;
    tr.execute("UPDATE lakehouse_migration SET version=6;")
        .await
        .with_context(|| "Updating lakehouse schema version to 6")?;
    Ok(())
}
```
Bump `LATEST_LAKEHOUSE_SCHEMA_VERSION` to `6` and add the `if 5 == current_version { ... }` step in
`execute_lakehouse_migration`, following the exact pattern of `upgrade_v4_to_v5`. No data migration
needed — every partition's real footer already lives in its own Parquet file in object storage;
that's the entire premise of the issue.

### Rollout ordering (why this is safe to do in one step now)

#1235 already ran the A/B in production and confirmed the footer-read path is equivalent. Unlike
that PR (which kept both paths side by side for comparison), this plan removes the postgres path
outright — there is no longer a reason to keep it once the comparison is done. Within a single
service instance the ordering is automatic: `migrate_lakehouse` runs at startup before queries are
served, so new code never sees the old schema.

Across instances, there *is* a transient window: several services share the lakehouse database
(flight-sql-srv replicas, telemetry-admin, monolith), and the first new instance to start drops the
table. Old-binary instances still running at that point will error on partition-metadata reads
(on a `MetadataCache` miss), on partition inserts, and on the temp-file cleanup's metadata delete,
until the rollout replaces them. This is the same exposure every prior schema migration here had
(v3→v4 dropped `lakehouse_partitions.file_metadata`, which old readers selected) and the accepted
operational answer is the same: deploy all services of a deployment together and let the brief
overlap errors resolve as old instances terminate. Failed materializations retry on the next pass;
failed queries can be re-run.

## Implementation Steps

1. **`partition_metadata.rs`** — delete the postgres `load_partition_metadata` and
   `delete_partition_metadata_batch`; rename `load_partition_metadata_from_footer` to
   `load_partition_metadata`; drop now-unused imports (`sqlx::{PgPool, Row}`,
   `crate::lakehouse::metadata_compat`, `crate::arrow_utils::parse_parquet_metadata`, `anyhow`
   pieces only used by the deleted function — keep `strip_column_index_info`, it's still used by
   the renamed function).
2. **`metadata_compat.rs`** — delete the file; remove `pub mod metadata_compat;` from `mod.rs`.
3. **`arrow_utils.rs`** — delete `serialize_parquet_metadata`.
4. **`reader_factory.rs`** — delete `read_disable_metadata_psql_cache`; drop
   `bypass_metadata_psql_cache` and `pool` fields from `ReaderFactory` and `ParquetReader`; update
   `ReaderFactory::new` signature and the `Debug` impl; simplify `get_metadata` to call
   `load_partition_metadata` directly (no branch); drop the now-unused `sqlx::PgPool` import and
   the `use super::partition_metadata::{load_partition_metadata, load_partition_metadata_from_footer}`
   line (import just the one renamed function).
5. **`lakehouse_context.rs`** — update both `ReaderFactory::new` call sites (drop `lake.db_pool.clone()`
   and `read_disable_metadata_psql_cache()` args); drop the now-unused import of
   `read_disable_metadata_psql_cache`.
6. **`write_partition.rs`** — delete the `INSERT INTO partition_metadata` block; remove the
   now-dead `file_metadata` plumbing (the `insert_partition` parameter and its call-site argument,
   the `PartitionWriteResult.file_metadata` field and its initializations — see Design → Write
   path); drop the `arrow_utils::serialize_parquet_metadata` and `ParquetMetaData` imports.
7. **`temp.rs`** — delete the `delete_partition_metadata_batch` call and its `with_context`; drop
   the `use super::partition_metadata::delete_partition_metadata_batch` import.
8. **`partition_cache.rs`** — delete `partition_with_metadata` and `PartitionWithMetadata`; drop the
   now-unused `load_partition_metadata` import and any now-unused `ParquetMetaData`/`PgPool`
   imports it leaves behind.
9. **`migration.rs`** — add `upgrade_v5_to_v6`, bump `LATEST_LAKEHOUSE_SCHEMA_VERSION` to 6, add the
   dispatch step in `execute_lakehouse_migration`.
10. **Tests** — see Testing Strategy: delete `reader_factory_tests.rs`, `test_metadata_compat.rs`,
    `test_parquet_metadata_format.rs`; update `file_cache_tests.rs`'s call sites to the renamed
    `load_partition_metadata`; replace `sql_view_test.rs`'s
    `partition_metadata_footer_parity_test` with a plain end-to-end smoke test (materialize a
    partition, query it back) since there's no longer a second path to compare against.
11. **Docs** — remove the "Experimental: bypassing the postgres partition-metadata cache" section
    from `mkdocs/docs/admin/object-cache.md` (see Documentation).
12. **Gate** — `cargo fmt`; `cargo clippy --workspace -- -D warnings`; `cargo test` from `rust/`;
    `python3 build/rust_ci.py`.

## Files to Modify

- `rust/analytics/src/lakehouse/partition_metadata.rs`
- `rust/analytics/src/lakehouse/metadata_compat.rs` (delete)
- `rust/analytics/src/lakehouse/mod.rs`
- `rust/analytics/src/arrow_utils.rs`
- `rust/analytics/src/lakehouse/reader_factory.rs`
- `rust/analytics/src/lakehouse/lakehouse_context.rs`
- `rust/analytics/src/lakehouse/write_partition.rs`
- `rust/analytics/src/lakehouse/temp.rs`
- `rust/analytics/src/lakehouse/partition_cache.rs`
- `rust/analytics/src/lakehouse/migration.rs`
- `rust/analytics/tests/reader_factory_tests.rs` (delete)
- `rust/analytics/tests/test_metadata_compat.rs` (delete)
- `rust/analytics/tests/test_parquet_metadata_format.rs` (delete)
- `rust/analytics/tests/file_cache_tests.rs`
- `rust/analytics/tests/sql_view_test.rs`
- `mkdocs/docs/admin/object-cache.md`

## Trade-offs

- **Go straight to removal vs. another A/B/soak period.** #1235 already produced production
  comparison data justifying this; running a second observation window on the same comparison adds
  delay without new information. If telemetry from #1235's rollout is inconclusive by the time this
  is implemented, pause and gather more before proceeding (see Open Questions).
- **Drop the table vs. keep it as a disabled/inert safety net.** Keeping the table around unused
  still pays the write-path insert cost and doesn't fix the TOAST-on-delete problem (the actual
  motivation in #1108/#1121) unless the cleanup-path delete is also removed — at which point the
  table is just accumulating rows nobody reads. A clean drop is simpler and matches the issue as
  filed. The persistent-cache alternatives in the issue (S3 Express, Redis/Valkey, a range-aware
  proxy) remain available as later, separate work if cold-miss latency becomes a measured problem —
  nothing here forecloses them, since they'd sit *underneath* the footer read (in `CachingReader`)
  rather than beside it.
- **Rename `load_partition_metadata_from_footer` → `load_partition_metadata` vs. keep the longer
  name.** With only one loader left, the `_from_footer` qualifier no longer disambiguates anything;
  renaming matches how the function reads at call sites (`ReaderFactory::get_metadata`) and avoids
  carrying naming baggage from the now-deleted sibling.
- **Delete `metadata_compat`/`serialize_parquet_metadata` vs. leave them unused.** Both are
  dead once their only caller is gone; per project convention, delete rather than leave inert code
  and tests around a format-compatibility problem that no longer applies to any live read path.

## Documentation

`mkdocs/docs/admin/object-cache.md` — delete the "### Experimental: bypassing the postgres
partition-metadata cache" subsection (lines ~182-199): the knob it documents no longer exists, and
its behavior (footer read via the object-cache-backed reader) is now simply how partition metadata
is always read, not worth a dedicated callout in the object-cache doc beyond what the surrounding
sections already say about what gets cached.

A `CHANGELOG.md` "Unreleased" entry should be added at PR time (per repo convention, one line
summarizing the removal and linking #1121), not as part of this plan document.

## Testing Strategy

- **Unit tests (`file_cache_tests.rs`)** — the two existing footer-read tests
  (`test_load_partition_metadata_from_footer_matches_direct_parse`,
  `test_load_partition_metadata_from_footer_with_metadata_cache`) already cover the sole remaining
  read path against an `InMemory` object store; update their call sites to the renamed
  `load_partition_metadata` (consider renaming the tests too, dropping `_from_footer`, for
  consistency).
- **Delete now-meaningless tests**: `reader_factory_tests.rs` (tested the env-var parser being
  removed), `test_metadata_compat.rs` (tests `parse_legacy_and_upgrade`, being removed),
  `test_parquet_metadata_format.rs` (tests `serialize_parquet_metadata`, being removed).
- **Replace the parity test** — `sql_view_test.rs::partition_metadata_footer_parity_test` currently
  compares the postgres and footer paths; once the postgres path is gone there's nothing to compare.
  Replace it with a smoke test in the same `#[ignore]`d live-infra style: materialize a partition
  through the real write path (as today), then run a query against it end-to-end and assert it
  succeeds and returns the expected rows — this exercises `get_metadata` through the only remaining
  path without needing a second implementation to diff against.
- **Migration test** — there are no existing automated tests covering `migrate_lakehouse` or any
  `upgrade_v*` step (confirmed by grep), so don't look for a pattern to follow. Verify the
  migration the way the repo actually exercises it: start the local test env
  (`local_test_env/ai_scripts/start_services.py`) against a database at v5 — `migrate_lakehouse`
  runs at service startup — then confirm the version row reads 6, `partition_metadata` is gone
  (`SELECT ... FROM information_schema.tables`), and an end-to-end query (the replacement smoke
  test, or `micromegas-query`) succeeds against pre-existing partitions.
- **Full gate**: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test` from `rust/`,
  then `python3 build/rust_ci.py`.

## Open Questions

- Has #1235's production A/B run long enough / against enough traffic to be confident there's no
  latency regression from cold misses before deleting the postgres path outright? (This plan
  assumes yes — confirm before implementing if the soak period was short.)
- None of the "persistent cache layer for cold misses" alternatives from the issue (S3 Express,
  Redis/Valkey, range-aware proxy) are in scope here — confirm that's still the intent, i.e. ship
  the removal first and only revisit a persistent cache if cold-miss latency shows up as a problem
  in practice.
