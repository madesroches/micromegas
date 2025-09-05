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
- `file_size` - Size in bytes
- `metadata` - bytea
- `insert_time` - Timestamp of metadata creation

### 2. Migration Strategy
- Create new table schema
- Migrate existing metadata from current storage location
- Update all read paths to use new table
- Update all write paths to use new table
- Add proper indexes for query performance

### 3. Implementation Steps

#### Step 1: Database Schema Changes
- [ ] Create migration for new `partition_metadata` table
- [ ] Add indexes on `file_path` and `created_at`
- [ ] Add foreign key constraints if needed

#### Step 2: Update Write Path
- [ ] Modify partition materialization to write metadata to new table
- [ ] Ensure metadata is written in same transaction as partition data

#### Step 3: Update Read Path
- [ ] Update `fetch_sql_partition_spec` to query new table
- [ ] Update `reader_factory` metadata loading to use new table
- [ ] Add fallback logic for backward compatibility during migration

#### Step 4: Handle Metadata Cleanup
- [ ] Delete metadata rows in the same transaction when deleting temporary_files
- [ ] Ensure metadata cleanup happens atomically with temporary_files deletion
- [ ] Update deletion code to handle both tables in single transaction

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
5. **Transaction Safety**: Metadata writes can be properly synchronized with data operations

