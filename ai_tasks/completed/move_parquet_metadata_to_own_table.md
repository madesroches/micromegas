# Move Parquet Metadata to Dedicated Table

## Objective
Create a dedicated table for parquet file metadata to fix race condition in reading parquet metadata ([#501](https://github.com/madesroches/micromegas/issues/501)).

## Problem Analysis
Currently, parquet metadata appears to be stored alongside partition data, causing race conditions when:
- Partition materialization tries to load metadata for a parquet file
- The metadata query returns no rows because the partition has been deleted
- This breaks the `fetch_sql_partition_spec` → `make_batch_partition_spec` flow

## Solution Design

### 1. Create New Metadata Table
Create a dedicated `partition_metadata` table with:
- `file_path` (PRIMARY KEY) - Full path to the parquet file
- `metadata` - bytea
- `insert_time` - Timestamp of metadata creation

### 2. Migration Strategy
- Create new table schema
- Migrate existing metadata from current storage location
- Update all read paths to use new table
- Update all write paths to use new table
- Add proper indexes for query performance

### 3. Implementation Steps

#### Step 1: Database Schema Changes ✅ COMPLETED
- [x] Create migration for new `partition_metadata` table (migration.rs:274)
- [x] Add PRIMARY KEY index on `file_path`
- [x] Table created with columns: file_path (text), metadata (bytea), insert_time (timestamp)

#### Step 2: Infrastructure for Metadata Operations ✅ COMPLETED
- [x] Created `partition_metadata.rs` module with full CRUD operations:
  - `load_partition_metadata()` - Load metadata by file path
  - `insert_partition_metadata()` - Insert new metadata
  - `delete_partition_metadata()` - Delete single metadata entry
  - `delete_partition_metadata_batch()` - Delete multiple entries in batch
  - `metadata_exists()` - Check if metadata exists
  - `get_metadata_insert_time()` - Get metadata creation timestamp

#### Step 3: Update Write Path ✅ COMPLETED
- [x] Found and updated partition materialization code in `write_partition.rs:295-310`
- [x] Metadata is now written to dedicated table within same transaction as partition data
- [x] Updated `retire_partitions()` and `retire_expired_partitions()` to clean up metadata
- [x] Updated `delete_expired_temporary_files()` in `temp.rs` to clean up metadata
- [x] All write path operations now use the new `partition_metadata` table

#### Step 4: Update Read Path ✅ COMPLETED
- [x] Updated `partition_cache.rs:load_partition_file_metadata()` to use only the new `partition_metadata` table
- [x] Removed fallback logic since `file_metadata` column was dropped from `lakehouse_partitions` 
- [x] Reader factory now transparently uses the new metadata table via existing abstraction
- [x] All read path operations now exclusively use the new `partition_metadata` table
- [x] Clean error handling when metadata is missing

#### Step 5: Handle Metadata Cleanup ✅ COMPLETED  
- [x] Updated `delete_expired_temporary_files()` in `temp.rs` to also delete metadata
- [x] Updated `retire_partitions()` and `retire_expired_partitions()` to clean up metadata
- [x] Ensured metadata cleanup happens atomically with partition deletion
- [x] All deletion code now handles both tables in single transaction

## Files to Modify

### Database/SQL
- `rust/analytics/src/lakehouse/migrations/` - Add new migration for partition_metadata table
- `rust/analytics/src/lakehouse/partition_metadata.rs` - New module for metadata operations

### Core Logic
- `rust/analytics/src/lakehouse/partition.rs` - Update partition operations
- `rust/analytics/src/lakehouse/reader_factory.rs` - Update metadata loading
- `rust/analytics/src/lakehouse/materialize.rs` - Update materialization logic

## Benefits
1. **Eliminates Race Condition**: Metadata queries will be isolated from partition data operations
2. **Better Performance**: Dedicated indexes and table structure optimized for metadata queries
3. **Cleaner Architecture**: Separation of concerns between data and metadata
4. **Transaction Safety**: Metadata writes can be properly synchronized with data operations

## Implementation Complete! 🎉

All steps have been completed:
- ✅ **Step 1**: Database schema and migration 
- ✅ **Step 2**: Metadata operations infrastructure
- ✅ **Step 3**: Write path integration
- ✅ **Step 4**: Read path integration  
- ✅ **Step 5**: Cleanup operations

The race condition in parquet metadata loading has been resolved by separating metadata storage into its own dedicated table.

## Post-Implementation Improvements ✅

After completing the core implementation, several code quality improvements were made:

### Function Consolidation and Cleanup
- **Merged metadata loading functions**: Eliminated `load_partition_file_metadata()` wrapper, now using `load_partition_metadata()` directly
- **Simplified error handling**: Changed `load_partition_metadata()` to return `Result<Arc<ParquetMetaData>>` instead of `Result<Option<...>>` - missing metadata is an error, not a normal case
- **Removed unused functions**: Cleaned up `partition_metadata.rs` by removing:
  - `StoredPartitionMetadata` struct (never used)
  - `insert_partition_metadata()` (write path uses manual INSERT)
  - `delete_partition_metadata()` (only batch version needed)
  - `metadata_exists()` (never called)
  - `get_metadata_insert_time()` (never called)

### Batch Operation Optimization  
- **Improved `delete_partition_metadata_batch()`**: Replaced placeholder-based approach with PostgreSQL's `ANY($1)` to handle unlimited file paths without parameter limits

### Function Naming Improvements
- **Merged partition insertion functions**: Eliminated `write_partition_metadata()` wrapper around `write_partition_metadata_attempt()`
- **Better naming**: Renamed to `insert_partition()` to accurately reflect that it handles complete partition insertion (locking, retirement, metadata, and data)

### Final Architecture
The implementation now has a clean, focused API:
- **`load_partition_metadata()`**: Loads metadata from dedicated table with proper error handling
- **`delete_partition_metadata_batch()`**: Efficiently deletes multiple metadata entries using PostgreSQL arrays
- **`insert_partition()`**: Complete partition insertion with advisory locking and transactional consistency

