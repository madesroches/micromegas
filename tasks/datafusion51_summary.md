# DataFusion 51.0 Metadata Migration - Final Approach

## Problem
Arrow 57.0 (included in DataFusion 51.0) made the `num_rows` field **required** in the Parquet FileMetaData thrift structure. Old metadata stored in the database is missing this field and fails to parse.

## Solution
Add a v4→v5 lakehouse schema migration that patches all existing metadata entries using the deprecated `parquet::format` thrift types.

## Implementation

### 1. Update Schema Version
Change `LATEST_LAKEHOUSE_SCHEMA_VERSION` from 4 to 5

### 2. Add Migration Step
In `execute_lakehouse_migration()`, add:
```rust
if 4 == current_version {
    info!("upgrade lakehouse schema to v5");
    let mut tr = pool.begin().await?;
    upgrade_v4_to_v5(&mut tr).await?;
    current_version = read_lakehouse_schema_version(&mut tr).await;
    tr.commit().await?;
}
```

### 3. Implement `upgrade_v4_to_v5()`
```rust
#[allow(deprecated)]
async fn upgrade_v4_to_v5(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    use parquet::format::FileMetaData as TFileMetaData;
    use parquet::format::thrift::protocol::{
        TCompactInputProtocol, TCompactOutputProtocol,
        TInputProtocol, TOutputProtocol,
    };

    info!("Patching partition_metadata for Arrow 57 compatibility");

    // Get all entries that need migration
    let entries = sqlx::query(
        "SELECT pm.file_path, pm.metadata, lp.num_rows
         FROM partition_metadata pm
         LEFT JOIN lakehouse_partitions lp ON pm.file_path = lp.file_path"
    ).fetch_all(&mut **tr).await?;

    let mut migrated = 0;

    for entry in entries {
        let file_path: String = entry.try_get("file_path")?;
        let metadata_bytes: Vec<u8> = entry.try_get("metadata")?;
        let num_rows: Option<i64> = entry.try_get("num_rows")?;

        // Try to parse - if it works, skip
        if parse_parquet_metadata(&Bytes::from(metadata_bytes.clone())).is_ok() {
            continue;
        }

        // Need num_rows to migrate
        let Some(num_rows) = num_rows else {
            warn!("Cannot migrate {} - missing num_rows", file_path);
            continue;
        };

        // Decode old format
        let mut cursor = std::io::Cursor::new(&metadata_bytes);
        let mut input_protocol = TCompactInputProtocol::new(&mut cursor);
        let mut t_metadata = TFileMetaData::read_from_in_protocol(&mut input_protocol)?;

        // Patch num_rows
        t_metadata.num_rows = num_rows;

        // Re-encode
        let mut buffer = BytesMut::new();
        {
            let mut writer = buffer.writer();
            let mut output_protocol = TCompactOutputProtocol::new(&mut writer);
            t_metadata.write_to_out_protocol(&mut output_protocol)?;
        }

        // Update database
        sqlx::query("UPDATE partition_metadata SET metadata = $1 WHERE file_path = $2")
            .bind(buffer.freeze().as_ref())
            .bind(&file_path)
            .execute(&mut **tr)
            .await?;

        migrated += 1;
        if migrated % 100 == 0 {
            info!("Migrated {}/{} entries", migrated, entries.len());
        }
    }

    info!("Migrated {} partition metadata entries", migrated);

    // Update schema version
    tr.execute("UPDATE lakehouse_migration SET version = 5")
        .await?;

    Ok(())
}
```

## Benefits
- ✅ Uses existing migration framework
- ✅ Runs automatically on service startup
- ✅ One-time operation
- ✅ Uses deprecated API (fine for one-time migration)
- ✅ Validates each entry
- ✅ Reports progress
- ✅ Handles missing num_rows gracefully

## Files to Modify
- `rust/analytics/src/lakehouse/migration.rs`:
  - Change `LATEST_LAKEHOUSE_SCHEMA_VERSION` to 5
  - Add v4→v5 migration step in `execute_lakehouse_migration()`
  - Implement `upgrade_v4_to_v5()` function

## Testing
1. Services will auto-run migration on next startup
2. Run Python integration tests to verify
3. Check logs for migration progress
4. Verify all tests pass
