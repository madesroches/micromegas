# DataFusion 51 Partition Format Versioning

## Executive Summary

**Status:** ✅ **IMPLEMENTED**

**Solution:** Multi-version format support with explicit version tracking in database tables, allowing users to skip backend upgrade versions without forced sequential migrations.

**Migration:** v4 → v5 (adds `partition_format_version` columns)

**Files Modified:**
- `rust/analytics/src/lakehouse/migration.rs` - v4→v5 migration
- `rust/analytics/src/lakehouse/write_partition.rs` - Write path with version=2
- `rust/analytics/src/lakehouse/partition_metadata.rs` - Read path with version-based dispatch
- `rust/analytics/tests/test_metadata_compat.rs` - Comprehensive test suite

---

## Problem Context

The Arrow 56.0 → 57.0 upgrade introduced metadata compatibility issues (see `datafusion51_metadata_bug.md` and `datafusion51_metadata_fix.md`).

Initial approaches used compatibility layers and on-access migration, but this forced users through sequential backend versions. A better approach was needed that allows:
1. Users to skip backend versions during upgrades
2. Clean separation between old and new format handling
3. No performance overhead for new partitions

## Solution: Explicit Format Versioning

Instead of implicit detection and compatibility layers, we now explicitly track the partition format version in the database.

### Format Versions

- **Version 1**: Arrow 56.0 format (DataFusion 50.x)
  - Metadata may have `num_rows=0`
  - Requires legacy parser with `num_rows` injection from `lakehouse_partitions.num_rows`
  - Used by existing partitions before migration v5

- **Version 2**: Arrow 57.0 format (DataFusion 51.x+)
  - Metadata has correct `num_rows` field
  - Uses standard `parse_parquet_metadata()` parser
  - Used by all new partitions after migration v5

### Database Schema

Migration v4→v5 adds `partition_format_version INTEGER NOT NULL DEFAULT 1` to:
- `lakehouse_partitions` table
- `partition_metadata` table

Both tables track the version to avoid joins during queries.

### Implementation Details

#### Write Path (`write_partition.rs:321-350`)

New partitions are explicitly marked as version 2:

```rust
// partition_metadata INSERT
sqlx::query(
    "INSERT INTO partition_metadata (file_path, metadata, insert_time, partition_format_version)
     VALUES ($1, $2, $3, 2)",
)

// lakehouse_partitions INSERT
sqlx::query(
    "INSERT INTO lakehouse_partitions VALUES($1, $2, ..., $12, 2);",
)
```

The constant `2` is inline in SQL (not bound) since it never changes at runtime.

#### Read Path (`partition_metadata.rs:68-122`)

Optimized for the common case (version 2):

```rust
pub async fn load_partition_metadata(pool: &PgPool, file_path: &str) -> Result<Arc<ParquetMetaData>> {
    // Fast path: query only partition_metadata table (no join)
    let row = sqlx::query(
        "SELECT metadata, partition_format_version
         FROM partition_metadata
         WHERE file_path = $1",
    )
    .fetch_one(pool).await?;

    let metadata_bytes: Vec<u8> = row.try_get("metadata")?;
    let partition_format_version: i32 = row.try_get("partition_format_version")?;

    // Dispatch based on format version
    let mut metadata = match partition_format_version {
        1 => {
            // Arrow 56.0 format - need num_rows from lakehouse_partitions for legacy parser
            let num_rows = /* additional query */ ...;
            metadata_compat::parse_legacy_and_upgrade(&metadata_bytes, num_rows)?
        }
        2 => {
            // Arrow 57.0 format - use standard parser (no additional query needed)
            parse_parquet_metadata(&metadata_bytes.into())?
        }
        _ => {
            return Err(anyhow!("unsupported partition_format_version {}", partition_format_version));
        }
    };

    // Strip column index info for DataFusion compatibility
    metadata = strip_column_index_info(metadata)?;
    Ok(Arc::new(metadata))
}
```

**Performance characteristics:**
- Version 2 (new data): Single query, no join, direct parsing
- Version 1 (legacy data): Two queries (metadata + num_rows), compatibility parsing

#### Migration (`migration.rs:387-409`)

```rust
async fn upgrade_v4_to_v5(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    // Add partition_format_version column to lakehouse_partitions
    // Default to 1 for existing partitions (Arrow 56.0 format)
    tr.execute(
        "ALTER TABLE lakehouse_partitions
         ADD COLUMN partition_format_version INTEGER NOT NULL DEFAULT 1;",
    ).await?;

    // Add partition_format_version column to partition_metadata
    // Default to 1 for existing metadata (Arrow 56.0 format)
    tr.execute(
        "ALTER TABLE partition_metadata
         ADD COLUMN partition_format_version INTEGER NOT NULL DEFAULT 1;",
    ).await?;

    tr.execute("UPDATE lakehouse_migration SET version=5;").await?;
    info!("added partition_format_version columns to both tables");
    Ok(())
}
```

Existing partitions default to version 1, ensuring compatibility with old metadata.

### Testing

Comprehensive test suite in `test_metadata_compat.rs`:

1. **test_legacy_parser_handles_new_format**: Validates legacy parser works with Arrow 57.0 metadata
2. **test_legacy_parser_injects_num_rows_when_zero**: Tests `num_rows` injection for Arrow 56.0 format
3. **test_legacy_parser_preserves_existing_num_rows**: Ensures non-zero values aren't overwritten

## Benefits

1. **No forced sequential upgrades**: Users can skip backend versions. The version number is checked at runtime.
2. **Performance optimized**: New partitions have zero compatibility overhead.
3. **Clean separation**: Version 1 and Version 2 use completely separate code paths.
4. **No join overhead**: Version stored in both tables avoids joins during metadata loading.
5. **Future-proof**: Easy to add Version 3, 4, etc. as Arrow/DataFusion evolve.

## Upgrade Path

When users upgrade from DataFusion 50.x to 51.x+:

1. **Before upgrade**: All partitions have `partition_format_version=1`
2. **Run migration v5**: Adds version columns, defaults to 1
3. **After upgrade**:
   - Existing partitions remain version 1, use legacy parser
   - New partitions are version 2, use standard parser
4. **Over time**: Old partitions naturally get replaced/refreshed as version 2

No manual intervention or bulk migration required.

## Relationship to Other Tasks

- **Supersedes**: `datafusion51_metadata_bug.md` (problem description)
- **Supersedes**: `datafusion51_metadata_fix.md` (initial compatibility layer approach)
- **Complements**: `datafusion51_null_pages_fix.md` (separate ColumnIndex stripping issue)

The versioning approach is the production-ready solution that replaced the initial on-access migration strategy.

## Future Considerations

### When Arrow 58+ is released

If Arrow 58+ introduces new metadata format changes:

1. Add Version 3 to the dispatch logic in `load_partition_metadata`
2. Update write path to use version 3 for new partitions
3. Old versions (1, 2) continue to work without changes

### Eventual Cleanup

Once all production data has migrated to Version 2 (natural replacement over time):

1. Remove Version 1 handling from `load_partition_metadata`
2. Remove `metadata_compat.rs` legacy parser
3. Deprecate `parquet::format` dependency (already deprecated by arrow-rs)

This is a long-term cleanup, not urgent.

## References

- Migration v5 schema version: `LATEST_LAKEHOUSE_SCHEMA_VERSION = 5`
- Legacy parser: `rust/analytics/src/lakehouse/metadata_compat.rs`
- Column index stripping: `partition_metadata.rs:strip_column_index_info()`
- Arrow 57.0 metadata changes: https://github.com/apache/arrow-rs/pull/8530
