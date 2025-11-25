# Fix test_blocks.py Failures with v1 Partition Metadata

## Problem Summary

The `test_blocks.py` tests fail with FlightSQL errors when querying blocks:
```
FlightInternalError: Parquet error: External: [reader_factory] loading metadata for views/blocks/global/2025-08-12/00-00-00_414d3520-4718-41fc-a7f1-a96a62bcafbc.parquet
```

**Root Cause Identified:** ✅ FIXED

**Insufficient buffer sizes in thrift serialization when processing v1 partition metadata.**

When processing v1 (Arrow 56.0) partition metadata, the system uses Apache Thrift to parse and re-serialize metadata in two places:

1. **`metadata_compat::parse_legacy_and_upgrade()`** - Parses v1 metadata and injects `num_rows` field
2. **`strip_column_index_info()`** - Removes legacy column index data for compatibility

Both functions used **hardcoded 8KB buffers** (`TBufferChannel::with_capacity(0, 8192)`) that were too small for large partition files with many row groups or columns. When metadata exceeded the buffer size, thrift returned "transport error" causing query failures.

**Why errors appeared non-deterministic:**
- Only affected partitions with large metadata (>8KB)
- DataFusion's parallel execution meant different partitions were processed on each run
- Errors propagated with misleading context messages

**The Fix:**
Changed buffer allocation from fixed 8192 bytes to **dynamic sizing based on input**:
- `metadata_compat.rs`: `metadata_bytes.len() * 2`
- `partition_metadata.rs`: `file_metadata_bytes.len() * 2`

This allows the system to handle large partition metadata regardless of size.

## Investigation Needed

The error suggests the metadata loading is failing, but initial investigation shows contradictory data:

1. ✅ File exists in `lakehouse_partitions` with `num_rows=1166873`
2. ✅ File exists in `partition_metadata` with version 1 metadata
3. ✅ File exists on disk
4. ❌ Query returns 0 rows during actual execution

**Possible causes:**
- Race condition during query execution
- Transaction isolation issue
- Connection pool using wrong database/schema
- Query parameter binding issue
- Different file_path string (whitespace, encoding, etc.)

## Side Issue: Expired Temporary Files

Unrelated to the test failure, but discovered during investigation:

- 1,822 files in `temporary_files` expired on Oct 30, 2025 (3+ weeks ago)
- Maintenance daemon not running in development environment
- These are retired partitions waiting for cleanup
- Should be cleaned up with: `telemetry-admin delete-expired-temp`

## Investigation Steps

### 1. Reproduce the Error with Logging

**Priority:** HIGH

Run the test with detailed SQL logging to see the actual query and parameters:

```bash
cd /home/madesroches/git/micromegas/python
RUST_LOG=sqlx=trace,micromegas=debug python3 -m pytest micromegas/tests/test_blocks.py::test_blocks_query -v -s
```

Check FlightSQL service logs to see:
- Exact SQL query being executed
- Parameters being bound
- Database connection details

### 2. Verify Database Connection Context

**Priority:** HIGH

Check if the query is using the correct database:

```rust
// Add debug logging in partition_metadata.rs around line 93
info!("Loading num_rows for file_path: {:?}", file_path);
info!("Database pool info: {:?}", pool);
let num_rows_row = sqlx::query("SELECT num_rows FROM lakehouse_partitions WHERE file_path = $1")
    .bind(file_path)
    .fetch_one(pool)
    .await;
info!("Query result: {:?}", num_rows_row);
```

### 3. Check for String Encoding Issues

**Priority:** MEDIUM

The file_path might have different encoding or whitespace:

```sql
-- Check for exact match
SELECT 
    length(file_path) as len,
    octet_length(file_path) as bytes,
    file_path
FROM lakehouse_partitions 
WHERE file_path LIKE '%76f72d4c-5c8e-4739-9ff5-f02c24f4bf52%';
```

### 4. Test Metadata Loading Directly

**Priority:** HIGH

Create a minimal test case:

```rust
// In rust/analytics/src/lakehouse/
#[tokio::test]
async fn test_load_v1_metadata_directly() {
    let pool = PgPool::connect("postgres://telemetry:telemetry@localhost:6432").await.unwrap();
    let file_path = "views/blocks/global/2025-08-23/00-00-00_76f72d4c-5c8e-4739-9ff5-f02c24f4bf52.parquet";
    
    let result = load_partition_metadata(&pool, file_path).await;
    assert!(result.is_ok(), "Failed to load metadata: {:?}", result.err());
}

## Potential Solutions (Speculative - Root Cause Unknown)

### Option 1: Fix Query Execution Context

If the issue is a transaction isolation or connection pool problem:
- Ensure metadata loading uses the correct database connection
- Check for transaction isolation level issues
- Verify connection pool configuration

### Option 2: Add Retry Logic

If this is a transient race condition:
- Add retry logic to the num_rows query
- Add better error logging
- Consider caching num_rows in memory

### Option 3: Upgrade to v2 Format

Eliminate dependency on lakehouse_partitions for metadata:
- Regenerate all v1 metadata as v2 format (Arrow 57.0)
- v2 format includes num_rows in the metadata itself
- No separate query needed

**Note:** This doesn't fix the underlying bug but might work around it

## Solution Implemented ✅

### Changes Made

**File: `rust/analytics/src/lakehouse/metadata_compat.rs`**
- Line 45: Changed buffer size from `8192` to `metadata_bytes.len() * 2`
- Added comment explaining why larger buffer is needed

**File: `rust/analytics/src/lakehouse/partition_metadata.rs`**
- Line 55: Changed buffer size from `8192` to `file_metadata_bytes.len() * 2`
- Added comment explaining dynamic sizing

### Testing
- All 5 tests in `test_blocks.py` now pass consistently
- No performance degradation observed
- Handles partitions with large metadata (>10KB) correctly

### Code Quality
- ✅ Formatted with `cargo fmt`
- ✅ Passes `cargo clippy` with no warnings
- ✅ Removed all debug logging added during investigation

## Acceptance Criteria

- [x] Root cause identified and documented
- [x] All `test_blocks.py` tests pass (5/5 passing)
- [x] Query `SELECT COUNT(*) FROM blocks` succeeds reliably
- [x] Code formatted and linted with no warnings

## References

- **Metadata loading code:** `rust/analytics/src/lakehouse/partition_metadata.rs:73-119`
- **Reader factory:** `rust/analytics/src/lakehouse/reader_factory.rs`
- **FlightSQL service:** `rust/public/src/servers/flight_sql_service_impl.rs:311`
- **Test file:** `python/micromegas/tests/test_blocks.py`

## Investigation Log

- 2025-11-25: Initial investigation incorrectly assumed issue was retired partitions
- 2025-11-25: Verified failing file IS in lakehouse_partitions with correct num_rows
- 2025-11-25: Root cause still unknown - query returns 0 rows despite data existing
