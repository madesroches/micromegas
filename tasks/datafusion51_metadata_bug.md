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

### Option 5: Backwards-Compatible Parser with Metadata Rewrite
Implement a custom backwards-compatible thrift parser that:
1. Handles metadata from DataFusion 50.0 (Arrow 56.0) that's missing the `num_rows` field
2. Injects the `num_rows` value from the `lakehouse_partitions.num_rows` column during parsing
3. Re-serializes the corrected metadata back to the database with the new format
4. After all metadata is rewritten, switch back to the standard parser

**Advantages:**
- Graceful migration without downtime
- Leverages existing `num_rows` data in `lakehouse_partitions` table
- No need to read parquet files from object storage
- Can be done incrementally as partitions are accessed
- One-time migration code that can be removed later

**Implementation approach:**
- Create a temporary compatibility layer in `arrow_utils.rs`
- Modify the thrift deserializer to make `num_rows` optional
- Inject the value from the partition record when missing
- Re-serialize and update the database
- After migration is complete, remove the compatibility layer

## Solution

**Status:** ✅ **SUPERSEDED by `datafusion51_partition_format_versioning.md`**

This document describes the initial problem investigation. The final production solution uses explicit partition format versioning instead of on-access migration.

**Initial approach (implemented but superseded):**

After investigation and testing, we determined:
- Legacy metadata CAN be parsed using deprecated `parquet::format` API
- New format IS backwards compatible with old parsers
- We can use `lakehouse_partitions.num_rows` to fix the metadata

The initial solution used a compatibility parser that:
1. Detects legacy metadata parse failures
2. Parses with deprecated thrift API
3. Injects `num_rows` from database
4. Re-serializes with Arrow 57.0
5. Caches upgraded metadata

**Production solution:**

See `datafusion51_partition_format_versioning.md` for the final implementation that uses explicit version tracking (`partition_format_version` column) to:
- Avoid forced sequential backend upgrades
- Provide zero-overhead performance for new partitions
- Cleanly separate Version 1 (Arrow 56.0) and Version 2 (Arrow 57.0) handling

## File Changes

### Affected Files in DataFusion 51.0 upgrade:
- `rust/Cargo.toml`: datafusion 50.0 → 51.0
- `rust/Cargo.lock`: Updated dependencies
- `rust/analytics/src/lakehouse/reader_factory.rs`: API changes (FileMeta → PartitionedFile)
- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs`: API changes
- `rust/analytics/src/lakehouse/write_partition.rs`: API changes
