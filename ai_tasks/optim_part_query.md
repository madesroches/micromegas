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
- Calculating partition statistics (batch_partition_merger.rs:37, 45)

## Implementation Strategy

### Step 1: Add num_rows column to lakehouse_partitions table
Since this involves a schema change, it should be done first. The `batch_partition_merger.rs` currently uses `partition.file_metadata.file_metadata().num_rows()` to get row counts for statistics.

1. **Add new migration** (bump LATEST_LAKEHOUSE_SCHEMA_VERSION to 3):
```sql
ALTER TABLE lakehouse_partitions ADD num_rows BIGINT;
CREATE INDEX lakehouse_partitions_file_path ON lakehouse_partitions(file_path);
```

The index on `file_path` will optimize the on-demand metadata loading queries:
```sql
SELECT file_metadata FROM lakehouse_partitions WHERE file_path = $1
```

2. **Populate num_rows column in migration** - Process partitions one at a time to avoid loading all metadata at once:
```rust
// In the migration function, process partitions individually
async fn populate_num_rows_column(pool: &PgPool) -> Result<()> {
    // Get all partitions that have file_metadata but no num_rows
    let partitions = sqlx::query("SELECT file_path, file_metadata FROM lakehouse_partitions WHERE file_metadata IS NOT NULL AND num_rows IS NULL")
        .fetch_all(pool)
        .await?;
    
    for row in partitions {
        let file_path: String = row.try_get("file_path")?;
        let file_metadata_buffer: Vec<u8> = row.try_get("file_metadata")?;
        
        // Parse metadata only for this partition
        let file_metadata = parse_parquet_metadata(&file_metadata_buffer.into())?;
        let num_rows = file_metadata.file_metadata().num_rows();
        
        // Update just this partition
        sqlx::query("UPDATE lakehouse_partitions SET num_rows = $1 WHERE file_path = $2")
            .bind(num_rows)
            .bind(file_path)
            .execute(pool)
            .await?;
    }
    Ok(())
}
```

3. **Update write_partition.rs** to store row count when creating partitions:
```rust
// Extract num_rows from ParquetMetaData during partition creation
let num_rows = partition.file_metadata.file_metadata().num_rows();
// Store in database INSERT query
```

4. **Add num_rows field to Partition struct**:
```rust
pub struct Partition {
    // ... existing fields ...
    pub num_rows: i64,  // Add this field
}
```

5. **Update all queries** to fetch num_rows column

6. **Add num_rows field to Partition struct**:
```rust
pub struct Partition {
    // ... existing fields ...
    pub file_metadata: Arc<ParquetMetaData>,  // Keep this for now
    pub num_rows: i64,  // Add this field
}
```

7. **Update compute_partition_stats to use stored row count**:
```rust
// Instead of: partition.file_metadata.file_metadata().num_rows()
// Use: partition.num_rows
```

This eliminates the dependency on file_metadata for basic statistics.

### Step 2: Add separate metadata loading
```rust
// New struct for when metadata is needed
pub struct PartitionWithMetadata {
    pub partition: Partition,
    pub file_metadata: Arc<ParquetMetaData>,
}

// Standalone metadata loading functions
pub async fn load_partition_file_metadata(
    pool: &PgPool, 
    file_path: &str
) -> Result<Arc<ParquetMetaData>> {
    let row = sqlx::query("SELECT file_metadata FROM lakehouse_partitions WHERE file_path = $1")
        .bind(file_path)
        .fetch_one(pool)
        .await?;
    
    let file_metadata_buffer: Vec<u8> = row.try_get("file_metadata")?;
    let file_metadata = Arc::new(parse_parquet_metadata(&file_metadata_buffer.into())?);
    Ok(file_metadata)
}

// Convenience function to create PartitionWithMetadata
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

### Step 3: Remove file_metadata from Partition struct and update queries
Now that we have separate metadata loading and num_rows field, remove the heavy `file_metadata` field and update all queries:

1. **Update Partition struct**:
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
    // pub file_metadata: Arc<ParquetMetaData>,  <-- Remove this
}
```

2. **Update all query methods** in `partition_cache.rs` to remove file_metadata from SELECT:
   - **PartitionCache::fetch_overlapping_insert_range** - Remove file_metadata from SELECT, add num_rows
   - **PartitionCache::fetch_overlapping_insert_range_for_view** - Remove file_metadata from SELECT, add num_rows  
   - **LivePartitionProvider::fetch** - Remove file_metadata from SELECT, add num_rows

This is the primary breaking change that forces all consumers to handle metadata separately.

### Step 4: Update consumers that need metadata
Files that use `partition.file_metadata` need to be updated:

1. **reader_factory.rs:27** - Change to load metadata on-demand
2. **batch_partition_merger.rs:37,45** - Remove dependency on file_metadata for stats


## Expected Benefits
- **Reduced I/O**: Skip reading large file_metadata blobs when not needed
- **Lower Memory Usage**: Avoid deserializing ParquetMetaData unnecessarily  
- **Faster Query Response**: Smaller result sets and less data transfer
- **Better Scalability**: Performance improvement scales with partition count

## Risk Mitigation
- This is a breaking change that will require updating all consumers
- Add comprehensive tests for new query patterns
- Monitor performance metrics to validate improvements

## Query Examples
Instead of:
```sql
SELECT view_set_name, view_instance_id, ..., file_metadata 
FROM lakehouse_partitions 
WHERE ...
```

Use:
```sql  
-- For lightweight partition info (default)
SELECT view_set_name, view_instance_id, ..., source_data_hash 
FROM lakehouse_partitions WHERE ...

-- For metadata when needed (separate query)
SELECT file_metadata 
FROM lakehouse_partitions WHERE file_path = $1
```