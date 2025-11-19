# DataFusion 51.0 Migration Plan

## Problem Summary

After upgrading from DataFusion 50.0 to 51.0, the parquet metadata stored in the `partition_metadata` table cannot be decoded by the new Arrow 57.0 parser.

**Error:** `Parquet error: Required field num_rows is missing`

## Root Cause

- DataFusion 51.0 upgraded to Arrow 57.0
- Arrow 57.0 includes a rewritten thrift parser for Parquet metadata (PRs #8530, #8587)
- The new parser is stricter and **requires the `num_rows` field**
- Existing metadata was serialized with DataFusion 50.0 / Arrow 56.0
- The old format is incompatible with the new parser

## Impact

- Python integration tests failing: `test_blocks_query`, `test_blocks_properties_stats`
- Any query that reads from materialized views (blocks, processes, streams, etc.) will fail
- The metadata table contains ~thousands of entries that all need migration

## Options Analysis

### Option 1: Truncate and Regenerate (REJECTED - Data Loss Risk)
- **Pros:** Simple, clean
- **Cons:**
  - Loses all cached metadata
  - Requires re-reading all parquet files from object storage
  - High latency on first queries
  - Could overload object storage with metadata reads
  - **RISK: If object storage files are missing, we lose ability to query that data**

### Option 2: Migration Script (RECOMMENDED)
Create a migration script that:
1. Reads each parquet file from object storage
2. Extracts metadata using the new parser
3. Serializes with the new format
4. Updates the database entry

**Pros:**
- Safe - validates each file exists
- Can report on any missing files
- Preserves query performance after migration
- Can be run incrementally
- Can be tested on subset first

**Cons:**
- More complex
- Takes time to run
- Requires object storage access

### Option 3: Lazy Migration (ALTERNATIVE)
Modify the reader to:
1. Try to decode with new parser
2. If it fails with "Required field num_rows is missing", read from object storage
3. Update the database entry with new format
4. Continue with query

**Pros:**
- Zero downtime
- Automatic migration
- Only migrates data that's actually queried

**Cons:**
- Adds complexity to hot path
- First query after upgrade will be slower
- Migration happens unpredictably

### Option 4: Dual-Format Support (NOT RECOMMENDED)
Add version field to track metadata format and support both.

**Cons:**
- Ongoing maintenance burden
- Complexity in codebase
- Still need to migrate eventually

## Recommended Approach: Option 2 (Migration Script)

### Implementation Steps

1. **Create migration script** (`rust/analytics/bin/migrate-metadata.rs`):
   ```rust
   // For each entry in partition_metadata:
   // 1. Try to parse with new format
   // 2. If successful, skip
   // 3. If fails, read from object storage
   // 4. Extract new metadata
   // 5. Update database
   // 6. Log progress
   ```

2. **Add safety checks**:
   - Backup partition_metadata table first
   - Count total entries to migrate
   - Report progress every N entries
   - Log any failures (missing files, parse errors)
   - Dry-run mode to validate before actual migration

3. **Test on subset**:
   - Run on blocks view first (smallest dataset)
   - Verify queries work
   - Then migrate other views

4. **Monitor**:
   - Track migration progress
   - Measure object storage read costs
   - Verify no data loss

### Migration Script Outline

```rust
use micromegas_analytics::arrow_utils::{parse_parquet_metadata, serialize_parquet_metadata};
use sqlx::PgPool;
use object_store::ObjectStore;

#[tokio::main]
async fn main() {
    let pool = PgPool::connect(&env::var("MICROMEGAS_SQL_CONNECTION_STRING")?).await?;
    let object_store = /* initialize from env */;

    // Get all metadata entries
    let entries = sqlx::query!("SELECT file_path, metadata FROM partition_metadata")
        .fetch_all(&pool)
        .await?;

    println!("Total entries to check: {}", entries.len());

    let mut migrated = 0;
    let mut skipped = 0;
    let mut errors = 0;

    for entry in entries {
        // Try to parse with new format
        match parse_parquet_metadata(&entry.metadata) {
            Ok(_) => {
                skipped += 1;
                continue; // Already in new format
            }
            Err(e) if e.to_string().contains("Required field num_rows") => {
                // Need to migrate this entry
                match migrate_entry(&pool, &object_store, &entry.file_path).await {
                    Ok(_) => migrated += 1,
                    Err(e) => {
                        eprintln!("Failed to migrate {}: {}", entry.file_path, e);
                        errors += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("Unexpected error for {}: {}", entry.file_path, e);
                errors += 1;
            }
        }

        if (migrated + skipped + errors) % 100 == 0 {
            println!("Progress: migrated={}, skipped={}, errors={}", migrated, skipped, errors);
        }
    }

    println!("Migration complete: migrated={}, skipped={}, errors={}", migrated, skipped, errors);
}

async fn migrate_entry(pool: &PgPool, store: &Arc<dyn ObjectStore>, file_path: &str) -> Result<()> {
    // Read parquet file from object storage
    let path = object_store::path::Path::from(file_path);
    let reader = ParquetObjectReader::new(store.clone(), path);

    // Get metadata from the file
    let metadata = reader.get_metadata(None).await?;

    // Serialize with new format
    let new_metadata = serialize_parquet_metadata(&metadata)?;

    // Update database
    sqlx::query!("UPDATE partition_metadata SET metadata = $1 WHERE file_path = $2",
        new_metadata.as_ref(),
        file_path
    )
    .execute(pool)
    .await?;

    Ok(())
}
```

## Timeline

1. **Phase 1: Development** (1-2 hours)
   - Create migration script
   - Add dry-run mode
   - Add progress reporting

2. **Phase 2: Testing** (30 min)
   - Test on small dataset
   - Verify queries work after migration
   - Check for edge cases

3. **Phase 3: Execution** (depends on data size)
   - Backup partition_metadata table
   - Run migration script
   - Monitor progress
   - Verify integration tests pass

4. **Phase 4: Validation** (15 min)
   - Run full test suite
   - Check query performance
   - Verify no data loss

## Rollback Plan

If migration fails:
1. Restore partition_metadata table from backup
2. Revert to DataFusion 50.0
3. Investigate issues
4. Fix migration script
5. Try again

## Success Criteria

- [ ] All partition_metadata entries can be parsed with new format
- [ ] All integration tests pass
- [ ] No data loss (verify row counts match)
- [ ] Query performance is maintained
- [ ] No missing files in object storage

## Next Steps

1. Get approval for migration approach
2. Implement migration script
3. Test on development environment
4. Execute migration
5. Validate results
