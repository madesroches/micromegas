# DataFusion 51.0 Metadata Parsing Bug

## Problem

After upgrading from DataFusion 50.0 to 51.0, Python integration tests are failing when querying blocks. The error occurs when trying to load parquet metadata from the database.

### Error Message
```
Parquet error: External: [reader_factory] loading metadata for views/blocks/global/2025-11-19/21-36-43_463a5192-1c4d-4143-b59b-3fefb811b5b2.parquet
```

### Failing Tests
- `test_blocks_query`
- `test_blocks_properties_stats`

## Root Cause

DataFusion 51.0 upgraded to Arrow 57.0, which includes a rewritten thrift parser for Parquet metadata (PR #8530, #8587). The new parser is stricter and **requires the `num_rows` field** in the Parquet metadata structure.

The metadata currently stored in the `partition_metadata` table was serialized using DataFusion 50.0 / Arrow 56.0, and when deserialized with the new parser, it fails with:

```
Parquet error: Required field num_rows is missing
```

## Investigation Details

### Location
- File: `rust/analytics/src/lakehouse/reader_factory.rs:46`
- Function: `load_parquet_metadata()` calls `parse_parquet_metadata()`
- File: `rust/analytics/src/arrow_utils.rs:23`
- Function: `parse_parquet_metadata()` calls `ParquetMetaDataReader::decode_metadata()`

### Test Results
Created test in `rust/analytics/tests/test_metadata_decode.rs` that confirmed:
- Metadata exists in database (9181 bytes for the test file)
- The bytes are being loaded correctly
- The `ParquetMetaDataReader::decode_metadata()` fails with "Required field num_rows is missing"

## Solution Options

### Option 1: Regenerate Metadata
Drop and regenerate all partition metadata by reading the actual parquet files from object storage. This would be the cleanest solution but requires:
- Reading all parquet files
- Extracting metadata with the new parser
- Updating the database

### Option 2: Migration Script
Create a migration that:
1. Reads existing metadata from partition_metadata table
2. Reads the actual parquet files from object storage
3. Re-serializes with the new format
4. Updates the database

### Option 3: Fallback to File Reading
Modify the reader to fallback to reading metadata from the actual parquet file if database decode fails. This adds latency but provides compatibility.

### Option 4: Database Schema Migration
Add a version field to track metadata format version, and handle migrations automatically.

## Recommended Solution

**Option 1 is recommended**: Truncate the `partition_metadata` table and let the system regenerate metadata on-demand from the parquet files. This is clean and leverages existing code paths.

The metadata table is a cache - the source of truth is the parquet files in object storage. We can safely regenerate it.

## Implementation Plan

1. Truncate the `partition_metadata` table
2. Restart the services
3. Run the integration tests - they will regenerate metadata as needed
4. Verify all tests pass

## File Changes

### Affected Files in DataFusion 51.0 upgrade:
- `rust/Cargo.toml`: datafusion 50.0 → 51.0
- `rust/Cargo.lock`: Updated dependencies
- `rust/analytics/src/lakehouse/reader_factory.rs`: API changes (FileMeta → PartitionedFile)
- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs`: API changes
- `rust/analytics/src/lakehouse/write_partition.rs`: API changes
