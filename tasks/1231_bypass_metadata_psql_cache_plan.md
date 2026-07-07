# Bypass the Postgres-Backed Partition Metadata Cache (Read-Side Knob) Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1231

## Overview

Partition parquet metadata is currently read via `load_partition_metadata`
(`rust/analytics/src/lakehouse/partition_metadata.rs`): a moka lookaside (`MetadataCache`) in
front of a postgres `SELECT` from the `partition_metadata` table, which stores pre-serialized
footer bytes. That path predates the (now much faster) object-cache service and exists to avoid
re-reading/re-parsing parquet footers from object storage on every miss.

This plan adds a runtime read-side knob — env var `MICROMEGAS_DISABLE_METADATA_PSQL_CACHE` — that,
when enabled, makes `ParquetReader::get_metadata` (the query hot path) **skip the postgres SELECT and
the `MetadataCache` lookaside entirely** and instead read + parse the parquet footer directly from
object storage through the existing object-cache-backed reader (`CachingReader`). It produces
metadata identical to the postgres path (same column-index stripping), so downstream behavior is
unchanged and the two paths can be A/B'd under production traffic to decide, later, whether the
postgres read path (or the `partition_metadata` table itself) can be retired — **that retirement
is explicitly out of scope here** (issue "Relationships": prerequisite for #1205, not this issue).

Writes are untouched: the `partition_metadata` table keeps being populated at write time
(`write_partition.rs`), so flipping the knob back restores exact prior behavior at any time.

## Current State

### Read path

`ReaderFactory` (`rust/analytics/src/lakehouse/reader_factory.rs`) is DataFusion's
`ParquetFileReaderFactory`. For each partition file it builds a `ParquetReader` that holds:
- `inner: CachingReader` — the object-cache-backed byte reader (`caching_reader.rs`), constructed
  with the object store, the file path, and `file_size` (from `partitioned_file.object_meta.size`),
- `pool: PgPool`, `metadata_cache: Arc<MetadataCache>`, `filename`, `file_size`.

`ParquetReader::get_metadata` (`reader_factory.rs:153-167`) ignores its `ArrowReaderOptions` and
calls `load_partition_metadata(&pool, &filename, Some(&metadata_cache))`. That function
(`partition_metadata.rs:76-148`):
1. checks `MetadataCache` (parsed `Arc<ParquetMetaData>`, keyed by file path) — hit returns early;
2. on miss, `SELECT metadata, partition_format_version FROM partition_metadata WHERE file_path=$1`;
3. dispatches by `partition_format_version`: v1 (Arrow 56) → `parse_legacy_and_upgrade` (extra
   `SELECT num_rows FROM lakehouse_partitions`), v2 (Arrow 57) → `parse_parquet_metadata`;
4. `strip_column_index_info` — re-serializes the thrift `FileMetaData`, clears
   `column_index_offset/length` and `offset_index_offset/length` on every column, re-decodes. This
   prevents DataFusion from trying to read legacy `ColumnIndex` structures with incomplete
   `null_pages` fields (required in Arrow 57+);
5. inserts the parsed+stripped `Arc<ParquetMetaData>` into `MetadataCache` (weighted by the
   postgres-serialized size).

`load_partition_metadata` is `#[span_fn]`, so the postgres path already has a latency span.

### `CachingReader` (the object-cache-backed reader)

`caching_reader.rs` exposes `get_bytes(Range<u64>)` and `get_byte_ranges(Vec<Range<u64>>)`. For
files under the file-cache max-size it loads the whole file once (thundering-herd-protected
`FileCache`, shared) and slices; for larger files it issues `object_store.get_range`/`get_ranges`
directly. It is **not** an `AsyncFileReader` — it deliberately provides only byte reads, leaving
metadata to the `ParquetReader` layer.

### Config wiring

`LakehouseContext` (`lakehouse_context.rs`) builds the shared caches and the single `ReaderFactory`.
`ReaderFactory::new` is called from exactly two places — `LakehouseContext::new` (`:105`, reads
`MICROMEGAS_METADATA_CACHE_MB` / `MICROMEGAS_FILE_CACHE_MB` / `MICROMEGAS_FILE_CACHE_MAX_FILE_MB`
env vars) and `LakehouseContext::with_caches` (`:127`, caches supplied by caller). `with_caches` is
currently unused (grep: zero callers anywhere in the repo besides its own definition) — tests and
internal callers (`export_log_view.rs:117`, `sql_batch_view.rs:89`,
`tests/histo_view_test.rs:161`, `tests/sql_view_test.rs:354`) all go through `LakehouseContext::new`.
Nothing else constructs `ReaderFactory`. The `std::env::var(...).parse().unwrap_or_else(warn+default)`
pattern in `LakehouseContext::new` is the house style for these knobs.

### Second caller (out of scope)

`partition_with_metadata` (`partition_cache.rs:33`) also calls `load_partition_metadata(pool, path,
None)`, but it has no object-store handle and **no callers anywhere in the repo** (grep: only its own
definition). The knob does not touch it; it keeps using postgres. Noted so a reviewer doesn't expect
it to change.

### parquet 58 metadata-reader API (verified against `parquet-58.3.0`)

- `ParquetMetaDataReader::new().load_and_finish(fetch, file_size).await -> Result<ParquetMetaData>`
  (`src/file/metadata/reader.rs:427`), gated on `async`+`arrow` features (both enabled via
  datafusion's parquet).
- `MetadataFetch` (`src/arrow/async_reader/metadata.rs:62`) is just
  `fn fetch(&mut self, range: Range<u64>) -> BoxFuture<'_, Result<Bytes>>`, and is blanket-impl'd
  for `&mut T where T: AsyncFileReader` by delegating to `get_bytes`. So an adapter over
  `CachingReader::get_bytes` is a one-liner.
- `ParquetMetaDataReader::new()` defaults both column-index and offset-index policy to `Skip`, so
  `load_and_finish` reads **only the footer** (typically 2 small fetches: the 8-byte
  footer tail, then the footer body) — no page-index reads. Errors are `parquet::errors::Result`,
  matching `get_metadata`'s return type directly.

## Design

### The knob

Env var **`MICROMEGAS_DISABLE_METADATA_PSQL_CACHE`**. Parse as boolean: `true` iff the value is one
of `1` / `true` / `yes` / `on` (case-insensitive); unset or anything else ⇒ `false` (default =
today's postgres path). On/off is a static process-lifetime choice read once at `ReaderFactory`
construction — sufficient for an A/B where you deploy a fleet slice with the env var set. Log the
resolved state once at construction (`info!("partition metadata read path: {object-cache|postgres}")`)
so operators can confirm which path is live.

A small free function in `reader_factory.rs` centralizes the parse so both `LakehouseContext`
constructors share it (DRY):
```rust
/// Reads MICROMEGAS_DISABLE_METADATA_PSQL_CACHE (default false). When true, ParquetReader reads
/// parquet footers directly from object storage instead of the postgres partition_metadata table.
pub fn read_disable_metadata_psql_cache() -> bool {
    match std::env::var("MICROMEGAS_DISABLE_METADATA_PSQL_CACHE") {
        Ok(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}
```

### Threading the flag

Add `bypass_metadata_psql_cache: bool` to `ReaderFactory` and to `ParquetReader`.
- `ReaderFactory::new` gains a `bypass_metadata_psql_cache: bool` param (explicit, so it stays
  testable without env). Both `LakehouseContext::new` and `::with_caches` call
  `read_disable_metadata_psql_cache()` and pass the result, for consistency and to future-proof
  `with_caches` — it currently has no callers (see Current State), so today this only covers the
  env-configured path via `LakehouseContext::new`. `create_reader` copies the bool into each
  `ParquetReader`.

Passing the flag explicitly (rather than reading the env inside `ReaderFactory`) matches how the
sibling cache knobs are resolved at the context layer and keeps `ReaderFactory` env-free/unit-testable.

### The direct-read path

In `ParquetReader::get_metadata`, branch on the flag:
```rust
if self.bypass_metadata_psql_cache {
    load_partition_metadata_from_footer(&mut self.inner, &self.filename, self.file_size).await
} else {
    load_partition_metadata(&pool, &filename, Some(&metadata_cache)).await.map_err(...)
}
```

New helper in `partition_metadata.rs` (co-located with the postgres path and the existing
`strip_column_index_info`, which it reuses):
```rust
/// Read and parse a partition's parquet footer directly from object storage via the
/// object-cache-backed reader, bypassing the postgres partition_metadata table and MetadataCache.
/// Produces metadata equivalent to load_partition_metadata (same column-index stripping) so the
/// two read paths are behaviorally interchangeable and can be A/B compared.
#[span_fn]
pub async fn load_partition_metadata_from_footer(
    reader: &mut CachingReader,
    file_path: &str,
    file_size: u64,
) -> datafusion::parquet::errors::Result<Arc<ParquetMetaData>> {
    let start = std::time::Instant::now();
    let raw = ParquetMetaDataReader::new()
        .load_and_finish(CachingReaderFetch(reader), file_size)
        .await?;
    let stripped = strip_column_index_info(raw)
        .map_err(|e| ParquetError::External(e.into()))?;
    debug!(
        "partition_metadata_footer_read file={file_path} file_size={file_size} \
         duration_ms={}", start.elapsed().as_millis()
    );
    Ok(Arc::new(stripped))
}
```
with the `MetadataFetch` adapter (in `reader_factory.rs` or `caching_reader.rs`, wherever
`CachingReader` visibility is cleanest):
```rust
struct CachingReaderFetch<'a>(&'a mut CachingReader);
impl MetadataFetch for CachingReaderFetch<'_> {
    fn fetch(&mut self, range: Range<u64>) -> BoxFuture<'_, parquet::errors::Result<Bytes>> {
        self.0.get_bytes(range).boxed()
    }
}
```

Why reuse `strip_column_index_info`: it makes the direct path produce the *same* `ParquetMetaData`
the postgres path produces, so switching the knob cannot change query results or trip the
Arrow-57 `ColumnIndex` `null_pages` issue the strip exists to avoid. The only measured difference
between the two paths is the metadata *acquisition* cost (postgres SELECT + parse vs object-store
footer fetch + parse). `load_partition_metadata_from_footer` lives in `partition_metadata.rs`,
the same module that defines `strip_column_index_info`, so it calls it directly as a private
module-local function — no visibility change needed.

### Telemetry / measurability (the point of the knob)

- Both paths are `#[span_fn]` (`load_partition_metadata` already; the new
  `load_partition_metadata_from_footer` gets the attribute), so per-call latency lands in the trace
  stream under distinct span names — directly A/B-comparable, consistent with the object-cache
  per-stage latency telemetry (#1206).
- The direct path emits a `partition_metadata_footer_read` debug log with `file`, `file_size`,
  `duration_ms`, mirroring the `parquet_read` logs already in `get_metadata`'s byte path. The
  `CachingReader`'s own `file_cache_load` / `should_cache` logs continue to show whether the footer
  fetch hit the object/file cache.

### Behavioral notes

- **MetadataCache is bypassed, not populated, under the knob.** Per the issue ("skip the … 
  `MetadataCache` lookaside"), the direct path does not read or write `MetadataCache`; it re-parses
  the footer on every `get_metadata` call. This is intentional — it measures the true cost of the
  object-cache-backed metadata read (the cost #1205's L1 cache would later remove), not a cost
  hidden behind the moka lookaside. `CachingReader`/`FileCache` still cache the footer *bytes*, so
  the object-store round-trip is typically avoided on repeat reads; only the thrift decode + strip
  repeats. This caveat is called out in Trade-offs.
- **No format-version dispatch needed.** The direct path parses the actual on-disk footer with the
  parquet-58 reader. v1 (Arrow 56) partitions still on disk are read by the same reader; the
  postgres path's `parse_legacy_and_upgrade` num_rows injection is a serialization-compat shim for
  the *stored* bytes, not a requirement for reading a real footer. (Validated in testing against
  both a freshly written v2 partition and, if available in the test lake, an older partition.)
- Writes, the `partition_metadata` table, migrations, and the `partition_with_metadata` helper are
  untouched.

### Wiring diagram
```
MICROMEGAS_DISABLE_METADATA_PSQL_CACHE
  -> read_disable_metadata_psql_cache()  (reader_factory.rs)
       -> LakehouseContext::{new, with_caches} -> ReaderFactory { bypass_metadata_psql_cache }
            -> create_reader -> ParquetReader { bypass_metadata_psql_cache }
                 -> get_metadata:
                      bypass? load_partition_metadata_from_footer(&mut inner, file, size)
                              -> ParquetMetaDataReader::load_and_finish(CachingReaderFetch(inner))
                              -> strip_column_index_info   [shared with postgres path]
                      else    load_partition_metadata(pool, file, Some(metadata_cache))  [unchanged]
```

## Implementation Steps

1. **`reader_factory.rs`** — add `read_disable_metadata_psql_cache()` free fn; add
   `bypass_metadata_psql_cache: bool` field to `ReaderFactory` and `ParquetReader`; add the param to
   `ReaderFactory::new` (and copy into `ParquetReader` in `create_reader`); add the
   `CachingReaderFetch` `MetadataFetch` adapter; branch `get_metadata` on the flag. Log the resolved
   path once (in `new`).
2. **`partition_metadata.rs`** — add `load_partition_metadata_from_footer` (`#[span_fn]`), co-located
   with (and calling directly into) the private `strip_column_index_info`, using
   `ParquetMetaDataReader::load_and_finish` + `strip_column_index_info`, with the
   `partition_metadata_footer_read` debug log. Add imports
   (`CachingReader`, `MetadataFetch`, `ParquetError`, `bytes::Bytes`, futures `FutureExt`) as needed.
3. **`lakehouse_context.rs`** — in `new` and `with_caches`, call `read_disable_metadata_psql_cache()`
   and pass the bool to `ReaderFactory::new`.
4. **Tests** (see Testing Strategy) — a test asserting the two read paths yield equal
   `ParquetMetaData` for the same partition, and env-parse unit tests.
5. **Docs** — brief note for the new experimental knob (see Documentation).
6. **Gate** — `cargo fmt`; `cargo clippy --workspace -- -D warnings`; `cargo test` from `rust/`;
   `python3 build/rust_ci.py`.

## Files to Modify

- `rust/analytics/src/lakehouse/reader_factory.rs`
- `rust/analytics/src/lakehouse/partition_metadata.rs`
- `rust/analytics/src/lakehouse/lakehouse_context.rs`
- `rust/analytics/tests/` — new/extended test (metadata parity + env parse); place near existing
  lakehouse tests (identify the right file during implementation, per "unit tests under tests/").
- `mkdocs/docs/admin/object-cache.md` (near the `## Client opt-in` section) — short knob note.

## Trade-offs

- **Static process-lifetime knob vs. per-query toggle.** A/B'ing under production means running a
  fleet slice with the env var set; a per-query flag adds surface with no benefit for this
  measurement. Read once at construction.
- **Bypass `MetadataCache` entirely (per issue) vs. keep the parsed-metadata lookaside and only swap
  the miss source.** Keeping the lookaside would give a cleaner "cost of a miss" comparison, but the
  issue's intent is to validate the *raw* object-cache footer path (the assumption #1205 builds on),
  so the parsed-cache is deliberately out of the loop. The extra per-call thrift decode+strip is the
  cost this exercise means to expose; it's captured by the span so the comparison is explicit.
  Byte-level caching (`FileCache`) still applies, so this is decode cost, not object-store I/O, on
  repeats.
- **Reuse `strip_column_index_info` (re-serialize round-trip) vs. parse footer without page index and
  skip the strip.** Reusing it guarantees identical metadata to the postgres path, so the knob can't
  change results — worth the re-serialize cost for an honest A/B. Skipping the strip is a possible
  future optimization once the postgres path is retired, not now.
- **No `with_prefetch_hint` on the reader.** Default 2-fetch footer load is simplest and correct; the
  footer bytes are usually already in `FileCache`. A prefetch hint (e.g. 64 KiB suffix) could cut it
  to one fetch for uncached large files — deferred, noted as a follow-up if telemetry shows the
  footer fetch dominating.
- **Explicit bool param on `ReaderFactory::new` vs. reading env inside `ReaderFactory`.** Explicit
  keeps `ReaderFactory` env-free and unit-testable and matches how sibling cache knobs resolve at the
  `LakehouseContext` layer; the single env read lives in one shared helper.

## Documentation

`mkdocs/docs/admin/object-cache.md` documents the object-cache read path, already has an
`## Environment variables` table and a `## Client opt-in` section for client-side read-path env
vars — the exact pattern this knob slots into (it's a client read-path toggle in
`reader_factory.rs`). Add a short **experimental** note for
`MICROMEGAS_DISABLE_METADATA_PSQL_CACHE` there (in/near `## Client opt-in`): what it does (read
partition metadata directly from object storage instead of postgres), that it's a read-only A/B
toggle with no write-side or data effect, default off, and how to compare the two `#[span_fn]`
spans. Backfilling docs for the other undocumented sibling knobs (`MICROMEGAS_METADATA_CACHE_MB`,
`MICROMEGAS_FILE_CACHE_MB`, etc.) is out of scope.

## Testing Strategy

- **Metadata parity (core correctness):** a true two-way parity test (`load_partition_metadata`
  vs `load_partition_metadata_from_footer` against a row actually populated by the write path)
  needs a live `DataLakeConnection` (postgres + object store) — `load_partition_metadata(pool,
  path, None)` unconditionally SELECTs against a `PgPool`, and the only way to populate that row is
  the write path (`write_partition.rs`). That makes it inherently an integration test, not a unit
  test. Split it in two:
  - **Footer-side unit test**, in `rust/analytics/tests/file_cache_tests.rs` (`InMemory` object
    store + `CachingReader`, matching that file's existing style): write a parquet file, call
    `load_partition_metadata_from_footer`, and assert the resulting `ParquetMetaData` matches what
    `parse_parquet_metadata` alone produces from the same bytes read directly (schema, num row
    groups, per-row-group row counts / total `num_rows`, column count) — these fields are invariant
    under the column-index strip, so no stripped reference is needed. (`strip_column_index_info` is
    a private `fn` in `partition_metadata.rs` and unreachable from this external integration test
    anyway.) This locks in the footer-read path's correctness without touching postgres.
  - **Full two-way parity test**, `#[ignore]`d alongside the live-infra tests in
    `sql_view_test.rs`/`histo_view_test.rs`: write a partition through the real write path, then
    load its metadata both ways and assert equivalence. Run manually / in an environment with live
    postgres + object storage.
- **Env parse:** unit tests for `read_disable_metadata_psql_cache` covering `1`/`true`/`TRUE`/`on`/
  `yes` ⇒ true and unset/`0`/`false`/garbage ⇒ false.
- **Smoke via existing query tests:** run the lakehouse query tests with the env var set to confirm
  queries still succeed through the direct-read path (no page-index/`null_pages` regression). If a
  serial/env-scoped test harness exists, add one that flips the env var; otherwise document the
  manual check (set `MICROMEGAS_DISABLE_METADATA_PSQL_CACHE=1`, run a query, confirm results match).
- **Full gate:** `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test` from `rust/`,
  then `python3 build/rust_ci.py`.

## Open Questions

_None — all resolved._ (Test home for the parity test is settled in Testing Strategy: a
footer-only unit test in `file_cache_tests.rs` plus an `#[ignore]`d live-infra two-way parity test
alongside `sql_view_test.rs`/`histo_view_test.rs`.)
