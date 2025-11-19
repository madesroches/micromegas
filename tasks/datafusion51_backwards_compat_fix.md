# DataFusion 51.0: Backwards Compatible Fix

## Discovery

We **DO** have the `num_rows` data! It's stored in the `lakehouse_partitions` table (added in schema v3 migration).

This means we can potentially patch the old metadata format to make it compatible with Arrow 57's stricter parser.

## New Approach: Patch Metadata on Load

Instead of migrating all metadata upfront, we can modify the `parse_parquet_metadata` function to:

1. Try parsing with the new parser
2. If it fails with "Required field num_rows is missing":
   - Manually add the `num_rows` field to the thrift-encoded metadata
   - Re-parse with the new parser
   - Return the patched metadata

This is a **backwards-compatible fix** that requires zero migration and zero downtime.

## Implementation

### Option A: Thrift-level Patching (Complex but Clean)

Decode the thrift bytes, add the missing `num_rows` field, re-encode:

```rust
pub fn parse_parquet_metadata(bytes: &Bytes) -> Result<ParquetMetaData> {
    // First try: parse as-is
    match ParquetMetaDataReader::decode_metadata(bytes) {
        Ok(metadata) => Ok(metadata),
        Err(e) if e.to_string().contains("Required field num_rows") => {
            // Backwards compatibility: The old format didn't include num_rows
            // We need to patch the thrift-encoded metadata

            // Decode thrift, add num_rows field, re-encode
            // This is complex and requires understanding the thrift format
            todo!("Implement thrift patching")
        }
        Err(e) => Err(e).context("parsing ParquetMetaData"),
    }
}
```

**Problem:** This requires deep thrift knowledge and is brittle.

### Option B: Join with lakehouse_partitions (Simple)

Modify `load_partition_metadata` to join with `lakehouse_partitions` and get `num_rows`:

```rust
pub async fn load_partition_metadata(
    pool: &PgPool,
    file_path: &str,
) -> Result<Arc<ParquetMetaData>> {
    let row = sqlx::query!(
        "SELECT pm.metadata, lp.num_rows
         FROM partition_metadata pm
         LEFT JOIN lakehouse_partitions lp ON pm.file_path = lp.file_path
         WHERE pm.file_path = $1",
        file_path
    )
    .fetch_one(pool)
    .await
    .with_context(|| format!("loading metadata for file: {}", file_path))?;

    let metadata_bytes = Bytes::from(row.metadata);

    // Try to parse
    match parse_parquet_metadata(&metadata_bytes) {
        Ok(metadata) => Ok(Arc::new(metadata)),
        Err(e) if e.to_string().contains("Required field num_rows") => {
            // Old format - need to patch it
            // But we have num_rows from the join!

            // Ideally, we'd patch the metadata and re-serialize it to update the DB
            // For now, fallback to reading from object storage
            warn!("Old metadata format for {}, falling back to file read", file_path);

            // TODO: Read from object storage and update DB
            Err(e).context("Old metadata format not yet supported")
        }
        Err(e) => Err(e).context(format!("parsing metadata for file: {}", file_path)),
    }
}
```

**Problem:** We still can't easily patch the thrift bytes.

### Option C: Read from Object Storage on Failure (Lazy Migration)

This is the most practical approach:

```rust
pub async fn load_partition_metadata(
    pool: &PgPool,
    object_store: &Arc<dyn ObjectStore>,
    file_path: &str,
) -> Result<Arc<ParquetMetaData>> {
    // Try to load from database
    let row = sqlx::query("SELECT metadata FROM partition_metadata WHERE file_path = $1")
        .bind(file_path)
        .fetch_optional(pool)
        .await?;

    if let Some(row) = row {
        let metadata_bytes: Vec<u8> = row.try_get("metadata")?;
        match parse_parquet_metadata(&Bytes::from(metadata_bytes)) {
            Ok(metadata) => return Ok(Arc::new(metadata)),
            Err(e) if e.to_string().contains("Required field num_rows") => {
                debug!("Old metadata format for {}, reading from object storage", file_path);
                // Fall through to object storage read below
            }
            Err(e) => return Err(e).context("parsing metadata"),
        }
    }

    // Load from object storage
    let path = object_store::path::Path::from(file_path);
    let reader = ParquetObjectReader::new(object_store.clone(), path);
    let metadata = reader.get_metadata(None).await
        .with_context(|| format!("reading metadata from object storage for {}", file_path))?;

    // Update database with new format
    let new_metadata_bytes = serialize_parquet_metadata(&metadata)?;
    sqlx::query("INSERT INTO partition_metadata (file_path, metadata) VALUES ($1, $2)
                 ON CONFLICT (file_path) DO UPDATE SET metadata = $2")
        .bind(file_path)
        .bind(new_metadata_bytes.as_ref())
        .execute(pool)
        .await
        .with_context(|| "updating partition_metadata")?;

    Ok(metadata)
}
```

**Benefits:**
- ✅ Zero migration needed
- ✅ Backwards compatible
- ✅ Auto-migrates on first access
- ✅ Simple to implement
- ✅ No downtime

**Drawbacks:**
- ❌ First query after upgrade is slower (reads from object storage)
- ❌ Need to pass `object_store` to the function

## Recommended Solution

**Option C: Lazy Migration** is the best approach:

1. Modify `load_partition_metadata` to accept `object_store` parameter
2. Try to parse cached metadata
3. If parsing fails with "Required field num_rows", read from object storage
4. Update the database with the new format
5. Return the metadata

Over time, as views are queried, all metadata will be automatically migrated to the new format.

## Implementation Tasks

1. [ ] Modify `load_partition_metadata` signature to include `object_store`
2. [ ] Add fallback logic for old metadata format
3. [ ] Update database with new format on fallback
4. [ ] Update all callers to pass `object_store`
5. [ ] Test with old metadata
6. [ ] Verify auto-migration works

## Testing

1. Verify old metadata triggers fallback
2. Verify fallback reads from object storage
3. Verify database is updated with new format
4. Verify subsequent reads use cached metadata
5. Run integration tests

This approach gives us backwards compatibility without requiring a migration script!
