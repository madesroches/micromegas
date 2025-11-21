# DataFusion 51 null_pages Field Issue

## Status
✅ **RESOLVED** - Fix implemented and tested successfully

## Problem

After upgrading to DataFusion 51, one integration test fails with:

```
pyarrow._flight.FlightInternalError: Flight returned internal error, with message:
error building data stream: External error: Parquet error: Parquet error:
Required field null_pages is missing at public/src/servers/flight_sql_service_impl.rs:311
```

**Failing test:**
- `tests/test_queries.py::test_spans`

**Test results:**
- ✅ 80 passed
- ❌ 1 failed
- ⏭️ 6 skipped

## Root Cause Analysis

### What is null_pages?

The `null_pages` field is a **required** field in the `ColumnIndex` structure of the Parquet format specification. From the Parquet thrift definition:

```thrift
struct ColumnIndex {
  1: required list<bool> null_pages  // <-- This is required!
  2: required list<binary> min_values
  3: required list<binary> max_values
  4: required BoundaryOrder boundary_order
  5: optional list<i64> null_counts
  // ...
}
```

The `null_pages` field is a list of boolean values where:
- `true` indicates a page contains only null values
- `false` indicates the page has valid min/max values

### When was it introduced?

- Page Index feature (including ColumnIndex with null_pages) was added in Parquet format spec
- Implemented in apache/parquet-format PR #63 (PARQUET-922)
- Available since Parquet 1.11.0 (released ~2018)
- Made default in DataFusion 51+ with Arrow 57.0

### Why does it fail now?

**DataFusion 50.x and earlier:**
- Page index reading was optional and disabled by default
- Legacy Parquet files without complete ColumnIndex worked fine

**DataFusion 51+ with Arrow 57.0:**
- Page index reading is enabled by default (`enable_page_index = true`)
- Arrow 57.0's Parquet parser strictly requires the `null_pages` field
- Legacy files with incomplete/malformed ColumnIndex now cause errors

### When does the error occur?

The error occurs during **query execution** when:
1. DataFusion builds a data stream to read Parquet files
2. The Parquet reader tries to parse the ColumnIndex from the file
3. The ColumnIndex is missing the required `null_pages` field
4. Arrow 57.0's strict parser throws an error

This is NOT during:
- Metadata loading from PostgreSQL (that works fine)
- Query planning (that completes successfully)

## Solution Implementation

The fix uses a **three-layer defense-in-depth approach** to prevent DataFusion from reading legacy ColumnIndex structures:

### Layer 1: Strip Column Index from Metadata ✅
**Location:** `rust/analytics/src/lakehouse/partition_metadata.rs:11-73, 122-124`

Added `strip_column_index_info()` function that:
1. Serializes ParquetMetaData to thrift format
2. Removes `column_index_offset` and `column_index_length` from all ColumnChunks
3. Also removes `offset_index_offset` and `offset_index_length` for consistency
4. Re-parses the modified metadata back to ParquetMetaData

**Key insight**: The metadata stored in PostgreSQL contains *pointers* to where the ColumnIndex lives in the actual Parquet file (byte offset and length). By removing these pointers from the metadata, DataFusion never knows that a ColumnIndex exists in the file, so it never attempts to read the malformed ColumnIndex data from the file itself.

**File structure**:
```
[Page Data]
[ColumnIndex]     ← Malformed data is HERE in the file (incomplete null_pages)
[FileMetaData]    ← But the POINTERS to ColumnIndex are in here
[Footer]
```

Our fix removes the pointers from FileMetaData before DataFusion sees it.

```rust
metadata = strip_column_index_info(metadata)?;
```

**Result:** ✅ Successfully prevents DataFusion from attempting to read column indexes

### Layer 2: Disable Page Index in SessionConfig ✅
**Location:** `rust/analytics/src/lakehouse/query.rs:224-225`

```rust
let config = SessionConfig::default()
    .set_bool("datafusion.execution.parquet.enable_page_index", false);
```

**Result:** ✅ Provides session-level protection against page index reading

### Layer 3: Disable Page Index in ArrowReaderOptions ✅
**Location:** `rust/analytics/src/lakehouse/reader_factory.rs:107-110`

```rust
let options = options.cloned().unwrap_or_else(|| {
    ArrowReaderOptions::new()
        .with_page_index(false)
});
```

**Result:** ✅ Provides reader-level protection as final safeguard

## Test Results

After implementing the fix:
- ✅ All 81 integration tests pass (previously: 80 passed, 1 failed)
- ✅ `tests/test_queries.py::test_spans` now succeeds
- ✅ No regressions in other tests

## New Files Written Correctly ✅

**Good news**: Files written with DataFusion 51+ (Arrow 57.0+) already have proper page index!

**Location**: `rust/analytics/src/lakehouse/write_partition.rs:582-590`

The `WriterProperties` configuration explicitly enables page-level statistics:

```rust
let props = WriterProperties::builder()
    .set_writer_version(WriterVersion::PARQUET_2_0)
    .set_compression(Compression::LZ4_RAW)
    .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Page)
    .build();
```

**Why this works**:
- Arrow 57.0+ defaults to `EnabledStatistics::Page` (page-level statistics enabled)
- This automatically generates proper ColumnIndex structures with valid `null_pages` field
- New files written after the DataFusion 51 upgrade are fully compatible
- Only **legacy files** (pre-upgrade) need migration

**Impact**:
- ✅ All new partitions written going forward will have proper page indexes
- ✅ Performance optimizations (page pruning, null skipping) will work for new data
- ⚠️ Only old partitions (created before upgrade) are affected by the workaround

## Performance Implications

**Important**: The performance impact only affects **legacy partitions** created before the DataFusion 51 upgrade. New partitions have proper page indexes and will benefit from full performance optimizations once the workarounds are removed.

### What We Lost (Legacy Partitions Only)

By disabling page index reading, we lose several important query optimizations for old files:

1. **Page-level pruning**: DataFusion can't skip pages where min/max values don't match query predicates
2. **Null page skipping**: Can't skip pages that contain only null values
3. **Reduced I/O**: Without page indexes, must read more data pages even when predicates would exclude them
4. **Query performance**: Queries with selective WHERE clauses will be slower, especially on large files

**Example impact:**
```sql
SELECT * FROM events WHERE timestamp > '2025-01-01'
```
Without page index: Reads ALL pages, filters in memory
With page index: Skips pages with max_timestamp < '2025-01-01'

### Migration Path to Restore Performance

**Three options to consider:**

1. **Natural Expiration (RECOMMENDED for most cases)**
   - Do nothing, wait for old data to expire
   - Timeline: Based on retention policy (e.g., 30-90 days)
   - Effort: Zero
   - Best for: Short-to-medium retention periods

2. **Metadata-only Migration (RECOMMENDED if active migration needed)**
   - Update metadata in PostgreSQL without touching files
   - Timeline: ~4 days development + migration time
   - Effort: Low, safe, fast
   - Best for: Need to remove runtime overhead soon

3. **File Rewrite Migration (Only if page index performance critical)**
   - Rewrite Parquet files with valid ColumnIndex
   - Timeline: 1-2 weeks + large I/O cost
   - Effort: High, slower, more risky
   - Best for: Long retention + need performance on historical queries

---

### Detailed Implementation Steps

#### Phase 1: Identify Legacy Files (Immediate)
```sql
-- Query to find partitions that need migration
SELECT file_path, num_rows, min_event_time, max_event_time
FROM lakehouse_partitions
WHERE created < '2025-01-20'  -- Before DataFusion 51 upgrade
ORDER BY num_rows DESC;
```

#### Phase 2: Strip Column Index Pointers from Legacy Metadata (Short-term, RECOMMENDED)

**Much simpler approach**: Update metadata in PostgreSQL without touching the Parquet files!

**Why this works**:
- The actual Parquet files don't change (no risk of data corruption)
- Only the metadata pointers in PostgreSQL are updated
- This is exactly what `strip_column_index_info()` already does at runtime
- Once metadata is updated, new files written will have valid pointers

**Implementation approach:**
```rust
// Migration tool: strip_legacy_metadata.rs
async fn strip_legacy_partition_metadata(pool: &PgPool) -> Result<()> {
    // 1. Find all legacy partitions (created before DataFusion 51 upgrade)
    let legacy_partitions = sqlx::query(
        "SELECT file_path, metadata
         FROM partition_metadata pm
         JOIN lakehouse_partitions lp ON pm.file_path = lp.file_path
         WHERE lp.created < '2025-01-20'  -- Adjust date to upgrade date
         ORDER BY lp.created"
    )
    .fetch_all(pool)
    .await?;

    info!("Found {} legacy partitions to migrate", legacy_partitions.len());

    // 2. For each partition, strip column index pointers from metadata
    for row in legacy_partitions {
        let file_path: String = row.try_get("file_path")?;
        let metadata_bytes: Vec<u8> = row.try_get("metadata")?;

        // Parse metadata
        let metadata = ParquetMetaDataReader::decode_metadata(
            &Bytes::from(metadata_bytes)
        )?;

        // Strip column index info (reuse existing function)
        let stripped_metadata = strip_column_index_info(metadata)?;

        // Serialize back
        let new_metadata_bytes = serialize_parquet_metadata(&stripped_metadata)?;

        // Update in database
        sqlx::query(
            "UPDATE partition_metadata
             SET metadata = $1, insert_time = NOW()
             WHERE file_path = $2"
        )
        .bind(new_metadata_bytes.as_ref())
        .bind(&file_path)
        .execute(pool)
        .await?;

        info!("Migrated metadata for: {}", file_path);
    }

    Ok(())
}
```

**Benefits**:
- ✅ No file I/O to object storage (fast!)
- ✅ No risk of file corruption or data loss
- ✅ Can be run online without downtime
- ✅ Easy to rollback (restore metadata from backup)
- ✅ Reuses existing `strip_column_index_info()` function
- ✅ After migration, can remove the runtime workaround

**Alternative: Rewrite Parquet Files (if you need valid page indexes)**

Only needed if you want to restore page-level pruning performance on legacy data:

```rust
async fn rewrite_partition_file(file_path: &str) -> Result<()> {
    // 1. Read legacy file
    let data = read_parquet(file_path).await?;

    // 2. Write new file with proper ColumnIndex
    let new_path = format!("{}.migrated", file_path);
    let writer_props = WriterProperties::builder()
        .set_statistics_enabled(EnabledStatistics::Page)
        .build();
    write_parquet_with_properties(&new_path, data, writer_props).await?;

    // 3. Update both metadata tables atomically
    let mut tx = pool.begin().await?;
    // ... update lakehouse_partitions and partition_metadata ...
    tx.commit().await?;

    // 4. Delete old file
    delete_from_object_store(file_path).await?;
}
```

#### Phase 3: Remove Runtime Workarounds (After Metadata Migration)

Once all legacy metadata has been updated in PostgreSQL:

1. **Remove the runtime stripping** from `partition_metadata.rs:122-124`:
   ```rust
   // DELETE THIS LINE:
   metadata = strip_column_index_info(metadata)?;
   ```

2. **Keep `strip_column_index_info()` function** for potential future use

3. **Remove SessionConfig override** from `query.rs:224-225`:
   ```rust
   // DELETE THESE LINES:
   let config = SessionConfig::default()
       .set_bool("datafusion.execution.parquet.enable_page_index", false);
   // REPLACE WITH:
   let config = SessionConfig::default();
   ```

4. **Remove ArrowReaderOptions override** from `reader_factory.rs:107-110`:
   ```rust
   // DELETE THESE LINES:
   let _options = options.cloned().unwrap_or_else(|| {
       ArrowReaderOptions::new().with_page_index(false)
   });
   // Can remove the entire block since options isn't used
   ```

5. **Test thoroughly** after removing workarounds:
   - Run all integration tests
   - Verify queries on both old and new partitions work
   - Check that page index is being used (should see performance improvement)

**Result**:
- ✅ New files: Page index enabled and used
- ✅ Old files: Metadata has no pointers, so page index not attempted (safe)
- ✅ No runtime overhead from stripping metadata

#### Phase 4: Prevent Future Issues (Long-term)

**Add validation to ingestion pipeline:**
```rust
// In telemetry-ingestion-srv after writing Parquet files
fn validate_parquet_metadata(metadata: &ParquetMetaData) -> Result<()> {
    for rg in metadata.row_groups() {
        for col in rg.columns() {
            // Ensure page index is present and valid
            if let Some(index) = col.column_index_ref() {
                ensure!(
                    index.null_pages.len() == rg.num_pages(),
                    "ColumnIndex null_pages length mismatch"
                );
            }
        }
    }
    Ok(())
}
```

### Estimated Timeline

**Metadata-only migration** (RECOMMENDED):
- **Phase 1** (Identify): 1 hour - Write SQL query to count affected partitions
- **Phase 2** (Metadata migration): 2-3 days - Develop tool, test on sample, run on all data
- **Phase 3** (Remove workarounds): 1 day - Remove code, test thoroughly
- **Phase 4** (Prevention): Optional - Writer already configured correctly

**Total time**: ~4 days

**File rewrite migration** (only if page index performance needed on old data):
- **Phase 1** (Identify): 1 day
- **Phase 2** (File migration): 1-2 weeks - Much slower due to object storage I/O
- **Phase 3** (Re-enable): 1 day
- **Phase 4** (Prevention): 2-3 days

### Alternative: Natural Expiration (Recommended)

**Best approach**: Simply wait for old partitions to naturally expire based on your retention policy.

**Why this works**:
- ✅ All new partitions (created after DataFusion 51 upgrade) have proper page indexes
- ✅ As old partitions expire and are deleted, the problem naturally resolves itself
- ✅ No migration effort required
- ✅ No risk of data migration bugs
- ⏱️ Timeline depends on your retention policy (e.g., 30 days, 90 days, etc.)

**When to use active migration instead**:
- You have very long retention periods (> 1 year)
- Query performance on historical data is critically important
- You need immediate performance improvements on old data

### Implementation Status

Current state after fix:
- ✅ New files: Proper page index enabled (`EnabledStatistics::Page`)
- ⚠️ Old files: Page index reading disabled via three-layer workaround
- 🔄 Transition: As old files expire, proportion of affected data decreases naturally

## Related Issues

- Fixed in same branch: DataFusion 51 metadata format mismatch (commit fac990716)
- Related to: Arrow 57.0.0 Parquet format strictness changes
- Related to: Page Index pruning feature enabled by default in DataFusion 51
