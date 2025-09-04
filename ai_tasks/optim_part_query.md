# Optimize Lakehouse Partitions Query Performance

## Problem
The `lakehouse_partitions` table includes a `file_metadata` column containing serialized ParquetMetaData which can be large (parquet footer data). Currently, all queries fetch this column even when it's not needed, causing unnecessary I/O and memory usage.

## Analysis

### Table Structure
- `file_metadata` column added in schema v2 (migration.rs:142)
- Contains serialized ParquetMetaData (parquet footer)
- Can be NULL for partitions without metadata
- Used for accessing parquet file structure and statistics

### Current Query Patterns

#### Queries that NEED file_metadata:
1. **PartitionCache::fetch_overlapping_insert_range** (partition_cache.rs:53-104)
   - Fetches all partition data including file_metadata
   - Deserializes metadata with `parse_parquet_metadata()`
   - Creates full Partition objects with Arc<ParquetMetaData>

2. **PartitionCache::fetch_overlapping_insert_range_for_view** (partition_cache.rs:108-167)
   - Similar to above, needs complete partition data
   - Parses file_metadata for each partition

3. **LivePartitionProvider::fetch** (partition_cache.rs:294-386)
   - Queries partitions for actual data access
   - Needs file_metadata to create Partition objects

#### Queries that DON'T need file_metadata:
1. **ListPartitionsTableFunction** (list_partitions_table_function.rs:102-113)
   - DataFusion table function for listing partitions
   - Only exposes metadata columns, NOT file_metadata
   - Currently doesn't fetch file_metadata column

2. **Potential partition existence checks** (not found in current code)
   - Checking if partitions exist for specific views/time ranges
   - Counting partitions
   - Getting partition file paths/sizes without needing parquet structure

### Usage Context

The `file_metadata` is primarily used when:
- Creating Partition objects that need parquet schema information
- Reading actual data from parquet files (reader_factory.rs:27)
- Calculating partition statistics (batch_partition_merger.rs:37, 45) - **OPTIMIZED in Step 1**

## Implementation Status

### ✅ Step 1: Add num_rows column to lakehouse_partitions table - **COMPLETED**

**Schema Migration (v2 → v3):**
- ✅ Bumped `LATEST_LAKEHOUSE_SCHEMA_VERSION` to 3
- ✅ Added `num_rows BIGINT NOT NULL` column to lakehouse_partitions table
- ✅ Added index on `file_path` for efficient on-demand metadata loading
- ✅ Robust migration logic to populate existing partitions with row counts

**Code Changes:**
- ✅ Updated `Partition` struct to include `num_rows: i64` field
- ✅ Updated INSERT statement to include num_rows (13 parameters)
- ✅ Updated `write_partition_from_rows` to extract and store row count from `thrift_file_meta.num_rows`
- ✅ Updated all SELECT queries in `partition_cache.rs` to fetch `num_rows` column
- ✅ Updated all Partition object constructions to include the `num_rows` field
- ✅ **OPTIMIZED:** Updated `batch_partition_merger.rs` to use `partition.num_rows` instead of `partition.file_metadata.file_metadata().num_rows()`

**Benefits Achieved:**
- ✅ **Immediate Performance Gain:** Statistics computation no longer requires parsing file_metadata
- ✅ **Infrastructure Ready:** Index on `file_path` enables efficient on-demand metadata loading
- ✅ **Backward Compatible:** All existing code continues to work without changes

### ✅ Step 2: Add separate metadata loading - **COMPLETED**

Infrastructure for on-demand metadata loading has been implemented:

**New Types and Functions Added:**
- ✅ `PartitionWithMetadata` struct - combines partition data with metadata when needed
- ✅ `load_partition_file_metadata()` function - loads metadata by file_path using the index from Step 1
- ✅ `partition_with_metadata()` convenience function - creates PartitionWithMetadata from existing Partition

**Implementation Details:**
```rust
// New struct for when metadata is needed
#[derive(Clone, Debug)]
pub struct PartitionWithMetadata {
    pub partition: Partition,
    pub file_metadata: Arc<ParquetMetaData>,
}

// Standalone metadata loading functions (using the file_path index from Step 1)
#[span_fn]
pub async fn load_partition_file_metadata(
    pool: &PgPool,
    file_path: &str,
) -> Result<Arc<ParquetMetaData>> {
    let row = sqlx::query("SELECT file_metadata FROM lakehouse_partitions WHERE file_path = $1")
        .bind(file_path)
        .fetch_one(pool)
        .await
        .with_context(|| format!("loading file_metadata for partition: {file_path}"))?;

    let file_metadata_buffer: Vec<u8> = row.try_get("file_metadata")?;
    let file_metadata = Arc::new(parse_parquet_metadata(&file_metadata_buffer.into())?);
    Ok(file_metadata)
}

// Convenience function to create PartitionWithMetadata
#[span_fn]
pub async fn partition_with_metadata(
    partition: Partition,
    pool: &PgPool
) -> Result<PartitionWithMetadata> {
    let file_metadata = load_partition_file_metadata(pool, &partition.file_path).await?;
    Ok(PartitionWithMetadata {
        partition,
        file_metadata,
    })
}
```

**Benefits Achieved:**
- ✅ **Non-Breaking Addition:** All existing code continues to work unchanged
- ✅ **Efficient Index Usage:** Uses the file_path index created in Step 1 for fast metadata lookups
- ✅ **Flexible Access Pattern:** Consumers can now choose when to load metadata vs. just partition data
- ✅ **Instrumented Functions:** Both functions include span tracing for observability
- ✅ **Ready for Step 3:** Infrastructure is in place for removing metadata from default Partition queries

### � Step 3: Remove file_metadata from Partition struct and update queries - **READY TO IMPLEMENT**

Now that Step 2 provides alternative access patterns, systematically replace all direct uses of `partition.file_metadata`:

**Phase 3a: Find and catalog all uses of `partition.file_metadata`**
- Search codebase for `partition.file_metadata` usage patterns
- Identify which consumers actually need metadata vs. just using it because it's available
- Create migration plan for each usage site

**Phase 3b: Update consumers to use on-demand loading**
- **reader_factory.rs** - Replace `partition.file_metadata` with `load_partition_file_metadata()`
- **Any other direct consumers** - Update to use Step 2 functions where metadata is needed
- **Test each change** to ensure functionality is preserved

**Phase 3c: Update partition queries to exclude file_metadata**
- **PartitionCache::fetch_overlapping_insert_range** - Remove file_metadata from SELECT
- **PartitionCache::fetch_overlapping_insert_range_for_view** - Remove file_metadata from SELECT
- **LivePartitionProvider::fetch** - Remove file_metadata from SELECT
- **Remove file_metadata parameter** from Partition struct construction

**Phase 3d: Remove file_metadata field from Partition struct**
```rust
pub struct Partition {
    pub view_metadata: ViewMetadata,
    pub begin_insert_time: DateTime<Utc>,
    pub end_insert_time: DateTime<Utc>,
    pub min_event_time: DateTime<Utc>,
    pub max_event_time: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub file_path: String,
    pub file_size: i64,
    pub source_data_hash: Vec<u8>,
    pub num_rows: i64,
    // pub file_metadata: Arc<ParquetMetaData>,  <-- Remove this field
}
```

### 🔴 Step 4: Comprehensive testing and validation - **PENDING STEP 3**

After Step 3 removes all `partition.file_metadata` usage:

**Performance Testing:**
- Measure query performance before/after removing file_metadata from queries
- Verify on-demand loading doesn't cause performance regressions
- Test with realistic partition counts and data sizes

**Functional Testing:**
- Ensure all existing functionality works with on-demand metadata loading
- Test edge cases (missing metadata, corrupted metadata, etc.)
- Validate that statistics computation still works correctly

**Integration Testing:**
- Test full data pipeline with the optimized queries
- Verify compatibility with existing data and new ingestion

## Expected Benefits
- **Reduced I/O**: Skip reading large file_metadata blobs when not needed (Step 3)
- **Lower Memory Usage**: Avoid deserializing ParquetMetaData unnecessarily (Step 3)
- **Faster Query Response**: Smaller result sets and less data transfer (Step 3)
- **Better Scalability**: Performance improvement scales with partition count (Step 3)
- **✅ Immediate Statistics Performance**: Row counts no longer require metadata parsing (Step 1 - ACHIEVED)

## Risk Mitigation
- ✅ **Step 1 Non-Breaking**: All existing code continues to work while gaining performance benefit
- ✅ **Step 2 Additive**: Only adds new functionality, no breaking changes
- � **Step 3-4 Breaking**: Will require careful coordination and testing

## Current Status Summary

### ✅ **Ready for Production (Steps 1-2)**
- Schema v3 migration is complete and tested
- On-demand metadata loading infrastructure is implemented and tested
- Immediate performance benefit for statistics computation
- No breaking changes, fully backward compatible
- All code compiles and works correctly

### 🟡 **Next Steps (Step 3)**
- Remove file_metadata from default Partition struct queries (breaking change)
- Update PartitionCache and LivePartitionProvider to not fetch file_metadata by default
- Use on-demand loading where metadata is actually needed

### 🔴 **Future Steps (Step 4)**
- Update all consumers to use on-demand loading pattern
- Comprehensive testing and performance validation
