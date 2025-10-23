# Advisory Lock Timing Issue

## Problem
Currently in `rust/analytics/src/lakehouse/write_partition.rs`, the advisory lock is acquired in `write_partition_metadata_attempt()` after the Parquet file has already been written to object storage.

This creates a race condition where:
1. Process A writes Parquet file
2. Process B writes Parquet file (same partition)
3. Process A acquires lock, retires old partitions, inserts metadata
4. Process B acquires lock, retires Process A's partition, inserts metadata

Result: Process A's file becomes orphaned in object storage.

## Solution
Move advisory lock acquisition to `write_partition_from_rows()` before creating the Parquet file, so the entire partition creation process (file write + metadata) is atomic.

## Note
The current implementation is correct but not efficient - it writes files that may become orphaned, wasting storage and write bandwidth.

## Files to change
- `rust/analytics/src/lakehouse/write_partition.rs`
  - Move lock logic from `write_partition_metadata_attempt()` to `write_partition_from_rows()`
  - Pass lock through the call chain or restructure the transaction scope