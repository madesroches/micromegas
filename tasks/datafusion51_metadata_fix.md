# DataFusion 51.0 Metadata Migration Fix

## Problem

After upgrading from DataFusion 50.0 to 51.0, Python integration tests fail when querying blocks. Arrow 57.0 (included in DataFusion 51.0) rewrote the Parquet metadata parser and now **requires** the `num_rows` field, which was optional in Arrow 56.0.

**Error:** `Parquet error: Required field num_rows is missing`

**Failing tests:**
- `test_blocks_query`
- `test_blocks_properties_stats`

## Root Cause

Metadata in the `partition_metadata` table was serialized with DataFusion 50.0 / Arrow 56.0 where `num_rows` was optional in the thrift schema. Arrow 57.0's new custom thrift parser requires this field.

## Solution

Use the deprecated `parquet::format` thrift API to parse legacy metadata, inject the missing `num_rows` from the `lakehouse_partitions` table, re-serialize with Arrow 57.0, and update the database.

### Key Findings from Testing

✅ **Legacy metadata CAN be parsed** using `parquet::format::FileMetaData::read_from_in_protocol()`
✅ **New format IS backwards compatible** - old thrift parser can read new metadata
✅ **`num_rows` available** in `lakehouse_partitions.num_rows` column

## Implementation

### Step 1: Add Dependencies

Already added to `rust/analytics/Cargo.toml`:
```toml
[dev-dependencies]
parquet = "57.0"
thrift = "0.17"
```

Move these to regular dependencies for production use.

### Step 2: Create Compatibility Parser

File: `rust/analytics/src/lakehouse/metadata_compat.rs`

```rust
use anyhow::{Context, Result};
use bytes::Bytes;
use parquet::format::FileMetaData as ThriftFileMetaData;
use parquet::thrift::TSerializable;
use thrift::protocol::{TCompactInputProtocol, TCompactOutputProtocol};
use datafusion::parquet::file::metadata::ParquetMetaData;

/// Parse legacy metadata (Arrow 56.0) and convert to new format (Arrow 57.0)
#[allow(deprecated)]
pub fn parse_legacy_and_upgrade(
    metadata_bytes: &[u8],
    num_rows: i64,
) -> Result<ParquetMetaData> {
    // Parse with old thrift API
    let mut transport = thrift::transport::TBufferChannel::with_capacity(
        metadata_bytes.len(),
        0
    );
    transport.set_readable_bytes(metadata_bytes);
    let mut protocol = TCompactInputProtocol::new(transport);

    let mut thrift_meta = ThriftFileMetaData::read_from_in_protocol(&mut protocol)
        .context("parsing legacy metadata with thrift")?;

    // Inject num_rows if missing or zero
    if thrift_meta.num_rows == 0 {
        thrift_meta.num_rows = num_rows;
    }

    // Re-serialize with thrift (now has num_rows)
    let mut out_transport = thrift::transport::TBufferChannel::with_capacity(0, 8192);
    let mut out_protocol = TCompactOutputProtocol::new(&mut out_transport);
    thrift_meta.write_to_out_protocol(&mut out_protocol)
        .context("serializing corrected thrift metadata")?;
    out_protocol.flush()?;

    let corrected_bytes = out_transport.write_bytes();

    // Parse with Arrow 57.0 (should work now)
    datafusion::parquet::file::metadata::decode_metadata(corrected_bytes)
        .context("re-parsing with Arrow 57.0")
}
```

### Step 3: Update partition_metadata.rs

**SIMPLIFIED APPROACH:** Just use the legacy reader for now. After migration, we'll switch back to the standard reader.

```rust
pub async fn load_partition_metadata(
    pool: &PgPool,
    file_path: &str,
) -> Result<Arc<ParquetMetaData>> {
    // Query both metadata and num_rows
    let row = sqlx::query(
        "SELECT pm.metadata, lp.num_rows
         FROM partition_metadata pm
         JOIN lakehouse_partitions lp ON pm.file_path = lp.file_path
         WHERE pm.file_path = $1"
    )
    .bind(file_path)
    .fetch_one(pool)
    .await
    .with_context(|| format!("loading metadata for file: {}", file_path))?;

    let metadata_bytes: Vec<u8> = row.try_get("metadata")?;
    let num_rows: i64 = row.try_get("num_rows")?;

    // Use legacy parser (works with both old and new formats)
    let metadata = metadata_compat::parse_legacy_and_upgrade(&metadata_bytes, num_rows)
        .with_context(|| format!("parsing metadata for {}", file_path))?;

    Ok(Arc::new(metadata))
}
```

This approach:
- ✅ Simple - just use the legacy parser always
- ✅ Works with both old and new formats (proven by tests)
- ✅ No database writes during normal operation
- ✅ After migration is done, we'll switch back to standard reader

### Step 4: Add metadata_compat Module

Update `rust/analytics/src/lakehouse/mod.rs`:

```rust
mod metadata_compat;
```

### Step 5: Testing

Run the integration tests:
```bash
cd python/micromegas
poetry run pytest -v tests/test_blocks.py::test_blocks_query
poetry run pytest -v tests/test_blocks.py::test_blocks_properties_stats
```

## Migration Strategy

### Phase 1: Deploy with Legacy Reader

1. Deploy code that uses legacy parser (Step 3)
2. System works with both old and new metadata formats
3. No migration happens yet - just compatibility mode

### Phase 2: Batch Migration

Use the lakehouse migration system to upgrade all metadata in the database.

Add a new migration function `upgrade_v4_to_v5()` in `rust/analytics/src/lakehouse/migration.rs`:

```rust
async fn upgrade_v4_to_v5(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    // Upgrade partition_metadata entries from Arrow 56.0 to Arrow 57.0 format
    migrate_arrow56_to_arrow57_metadata(tr)
        .await
        .with_context(|| "migrating metadata from Arrow 56.0 to 57.0")?;

    tr.execute("UPDATE lakehouse_migration SET version=5;")
        .await
        .with_context(|| "Updating lakehouse schema version to 5")?;
    Ok(())
}

async fn migrate_arrow56_to_arrow57_metadata(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    info!("migrating metadata from Arrow 56.0 to Arrow 57.0 format");

    // Get all file paths that need migration (small data)
    let file_paths: Vec<String> = sqlx::query_scalar(
        "SELECT pm.file_path
         FROM partition_metadata pm
         ORDER BY pm.file_path",
    )
    .fetch_all(&mut **tr)
    .await?;

    let total_to_migrate = file_paths.len();
    info!(
        "found {} partition metadata entries to check and potentially migrate",
        total_to_migrate
    );

    let mut total_migrated = 0;
    let mut already_new = 0;
    let mut failed = 0;
    let batch_size = 10; // Small batch size since metadata can be large

    // Process in batches to avoid loading too much metadata at once
    for chunk in file_paths.chunks(batch_size) {
        // Build a query to fetch just this batch with num_rows from lakehouse_partitions
        let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("${}", i)).collect();
        let query_str = format!(
            "SELECT pm.file_path, pm.metadata, lp.num_rows
             FROM partition_metadata pm
             JOIN lakehouse_partitions lp ON pm.file_path = lp.file_path
             WHERE pm.file_path IN ({})",
            placeholders.join(", ")
        );

        let mut query = sqlx::query(&query_str);
        for path in chunk {
            query = query.bind(path);
        }

        let rows = query.fetch_all(&mut **tr).await?;

        for row in rows {
            let file_path: String = row.try_get("file_path")?;
            let metadata_bytes: Vec<u8> = row.try_get("metadata")?;
            let num_rows: i64 = row.try_get("num_rows")?;

            // Check if already in Arrow 57.0 format (standard parser works)
            if parse_parquet_metadata(&metadata_bytes.clone().into()).is_ok() {
                already_new += 1;
                continue;
            }

            // Need migration - parse with legacy and upgrade
            match crate::lakehouse::metadata_compat::parse_legacy_and_upgrade(&metadata_bytes, num_rows) {
                Ok(metadata) => {
                    // Serialize with Arrow 57.0 format
                    match crate::arrow_utils::serialize_parquet_metadata(&metadata) {
                        Ok(new_bytes) => {
                            // Update database
                            if let Err(e) = sqlx::query(
                                "UPDATE partition_metadata SET metadata = $1 WHERE file_path = $2"
                            )
                            .bind(&new_bytes[..])
                            .bind(&file_path)
                            .execute(&mut **tr)
                            .await {
                                error!("failed to update metadata for {}: {}", file_path, e);
                                failed += 1;
                            } else {
                                total_migrated += 1;
                            }
                        }
                        Err(e) => {
                            error!("failed to serialize metadata for {}: {}", file_path, e);
                            failed += 1;
                        }
                    }
                }
                Err(e) => {
                    error!("failed to parse legacy metadata for {}: {}", file_path, e);
                    failed += 1;
                }
            }
        }

        if (total_migrated + already_new + failed) % 100 == 0 || (total_migrated + already_new + failed) == total_to_migrate {
            info!(
                "progress: {}/{} (migrated: {}, already new: {}, failed: {})",
                total_migrated + already_new + failed, total_to_migrate, total_migrated, already_new, failed
            );
        }
    }

    info!(
        "metadata migration complete: {} total, {} migrated, {} already new format, {} failed",
        total_to_migrate, total_migrated, already_new, failed
    );

    if failed > 0 {
        return Err(anyhow::anyhow!("Failed to migrate {} metadata entries", failed));
    }

    Ok(())
}
```

**Key changes:**
1. Update `LATEST_LAKEHOUSE_SCHEMA_VERSION` from `4` to `5`
2. Add the `upgrade_v4_to_v5()` function to the migration chain in `execute_lakehouse_migration()`
3. Import `metadata_compat` module (need to make it `pub` in mod.rs)
4. Import `serialize_parquet_metadata` from `arrow_utils`

**Migration runs automatically:**
- When analytics server starts, it calls `migrate_lakehouse()`
- Uses database locking to ensure only one instance runs the migration
- Processes metadata in batches of 10 to avoid memory issues
- Logs progress every 100 entries
- Fails the migration if any entries fail (ensures data integrity)

**IMPORTANT - Remove migration from FlightSQL server:**
- Migration can be slow (large databases may take minutes)
- ECS will kill the FlightSQL server if it doesn't respond to load balancer health checks
- Need to remove `migrate_lakehouse()` call from FlightSQL server startup
- **Migration will run when the daemon is updated** - the daemon doesn't have load balancer health check constraints
- Deployment sequence:
  1. Update daemon with new code (includes migration logic)
  2. Daemon runs migration on startup (safe from ECS timeouts)
  3. Deploy FlightSQL server with compatibility reader (works during migration)
  4. After migration completes and stabilizes, switch to standard reader

### Phase 3: Switch Back to Standard Reader

After migration is complete, update `load_partition_metadata()` to use the standard Arrow 57.0 parser:

```rust
pub async fn load_partition_metadata(
    pool: &PgPool,
    file_path: &str,
) -> Result<Arc<ParquetMetaData>> {
    let row = sqlx::query("SELECT metadata FROM partition_metadata WHERE file_path = $1")
        .bind(file_path)
        .fetch_one(pool)
        .await
        .with_context(|| format!("loading metadata for file: {}", file_path))?;

    let metadata_bytes: Vec<u8> = row.try_get("metadata")?;

    // Use standard Arrow 57.0 parser (all metadata is now in new format)
    let metadata = parse_parquet_metadata(&Bytes::from(metadata_bytes))
        .with_context(|| format!("parsing metadata for file: {}", file_path))?;

    Ok(Arc::new(metadata))
}
```

### Phase 4: Remove Compatibility Code

Once the standard reader is deployed and stable:

1. Delete `rust/analytics/src/lakehouse/metadata_compat.rs`
2. Remove from `rust/analytics/src/lakehouse/mod.rs`
3. Remove `parquet = "57.0"` and `thrift = "0.17"` from dependencies
4. Delete test files:
   - `rust/analytics/tests/test_legacy_metadata_parse.rs`
   - `rust/analytics/tests/test_forward_compat.rs`

Timeline: Can be done anytime before parquet 59.0.0 (when `format` module is removed)

## Advantages

✅ Zero downtime - system works in compatibility mode while migration runs
✅ Forward compatible - legacy parser works with both old and new formats
✅ Backwards compatible - new metadata can be read by old code if needed
✅ No object storage reads - pure database operation
✅ Uses existing data from `lakehouse_partitions.num_rows`
✅ Simple deployment - just deploy code, run migration script, switch reader
✅ Temporary code - clean removal path after migration

## Files Changed

- `rust/analytics/Cargo.toml` - add dependencies
- `rust/analytics/src/lakehouse/metadata_compat.rs` - NEW compatibility parser
- `rust/analytics/src/lakehouse/partition_metadata.rs` - add fallback logic
- `rust/analytics/src/lakehouse/mod.rs` - add module
