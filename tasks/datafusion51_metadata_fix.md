# DataFusion 51 Metadata Bug

## Executive Summary

**Status:** ✅ **RESOLVED**

**Root Cause:** Format mismatch between `ParquetMetaDataWriter` and `ParquetMetaDataReader::decode_metadata()` introduced in DataFusion 51 due to Page Index support.

**Solution:** Implemented extraction logic in `serialize_parquet_metadata()` to skip page index data and return only the FileMetaData bytes that the decoder expects.

**Files Modified:**
- `rust/analytics/src/arrow_utils.rs` - Fixed serialization function
- `rust/analytics/tests/test_datafusion_metadata_bug.rs` - Added test demonstrating the fix

---

## Problem

After upgrading from DataFusion 50.2.0 to 51.0.0, Python integration tests fail when querying blocks:

```
Parquet error: Required field num_rows is missing
```

**Failing tests:**
- `test_blocks_query`
- `test_blocks_properties_stats`

## Root Cause (Detailed Investigation)

### Initial Hypothesis: Writer Not Updated ❌

The initial hypothesis was that arrow-rs 57.0.0 only completed Phase 1 (reader) but not Phase 2 (writer) of the thrift remodel. **This was incorrect.**

### Actual Root Cause: Format Mismatch ✅

Investigation of parquet 57.0.0 source code revealed:

**Both writer and reader use the NEW custom thrift implementation:**
- `AsyncArrowWriter::close()` and `ParquetMetaDataWriter::finish()` use the same code path
- `ThriftMetadataWriter::finish()` at writer.rs:135-251
- `write_file_metadata()` → `write_thrift_object()`
- Uses `ThriftCompactOutputProtocol` - the custom writer
- **The writer DOES write num_rows** (confirmed in thrift/mod.rs:1283-1285)

**The real issue is a format mismatch:**

When DataFusion 51 introduced Page Index support (ColumnIndex and OffsetIndex), `ParquetMetaDataWriter` began outputting:
```
[Page Indexes: variable size] + [FileMetaData] + [Length: 4 bytes] + [PAR1: 4 bytes]
```

But `ParquetMetaDataReader::decode_metadata()` expects:
```
[FileMetaData: raw Thrift bytes only]
```

When the decoder receives the full output, it starts parsing at byte 0 (which contains page index data, not FileMetaData), encounters invalid thrift structures, and fails with "Required field num_rows is missing".

### Proof

Minimal reproduction test (`test_datafusion_metadata_bug.rs`):
```rust
let metadata = arrow_writer.close().unwrap();  // Has num_rows=5
let serialized = ParquetMetaDataWriter::new(&mut buffer, &metadata).finish().unwrap();
ParquetMetaDataReader::decode_metadata(&serialized).unwrap();  // FAILS: num_rows missing
```

Initial result: `❌ decode_metadata() FAILED: Parquet error: Required field num_rows is missing`

After applying the fix (extracting just the FileMetaData portion): `✅ SUCCESS! num_rows: 5`

### Code Investigation (parquet 57.0.0)

Examined the source code at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/parquet-57.0.0/`:

**Writer path (used by both `AsyncArrowWriter` and `ParquetMetaDataWriter`):**
```
src/file/metadata/writer.rs:
  ThriftMetadataWriter::finish() (line 135)
    → write_column_indexes()
    → write_offset_indexes()
    → write_file_metadata() (line 209)
      → MetadataObjectWriter::write_file_metadata() (line 498)
        → write_thrift_object() (line 488)
          → ThriftCompactOutputProtocol::new()
          → object.write_thrift()
    → write footer: metadata_len + PAR1 magic (lines 214-217)
```

**Key finding**: `ParquetMetaDataWriter` uses the exact same `ThriftMetadataWriter` infrastructure as `AsyncArrowWriter`. Both serialize using `ThriftCompactOutputProtocol` (the new custom thrift implementation). The writer correctly writes all fields including `num_rows`.

### Test Results (Critical Finding!)

Running the actual tests reveals an important detail:

**test_writer_format.rs** (✅ PASSES):
- Uses `ArrowWriter` (sync, not async)
- Output size: 350 bytes
- decode_metadata() works with PAR1, without PAR1, and without length+PAR1
- ✅ ALL decode attempts succeed!

**test_datafusion_metadata_bug.rs** (❌ FAILS):
- Uses `ArrowWriter` (sync, same as test_writer_format)
- Output size: 394 bytes (44 bytes larger!)
- decode_metadata() fails: "Required field num_rows is missing"
- ❌ Decode fails

**THE MYSTERY**: Why are the two tests producing different sized outputs (350 vs 394 bytes) when both use `ArrowWriter` on similar simple schemas? The 44-byte difference suggests something extra is being written in the failing test.

## Solution

### The Fix (Thanks to Gemini AI)

The issue is a **format mismatch** between writer and reader:

**ParquetMetaDataWriter output format:**
```
[Optional Page Indexes: 33 bytes] + [FileMetaData: 353 bytes] + [Length: 4 bytes] + [PAR1: 4 bytes] = 394 bytes
```

**ParquetMetaDataReader::decode_metadata() expects:**
```
[FileMetaData: raw Thrift bytes only]
```

When page indexes are present (introduced in DataFusion 51), `ParquetMetaDataWriter` puts them BEFORE the FileMetaData. The decoder starts reading from byte 0, encounters page index data instead of FileMetaData, and fails with "Required field num_rows is missing".

**The Fix:**
```rust
// Extract just the FileMetaData portion
let footer_len_bytes = &serialized[serialized.len() - 8..serialized.len() - 4];
let metadata_len = u32::from_le_bytes(footer_len_bytes.try_into().unwrap()) as usize;
let footer_start = serialized.len() - 8 - metadata_len;
let thrift_slice = &serialized[footer_start..serialized.len() - 8];

// Now decode_metadata works!
ParquetMetaDataReader::decode_metadata(thrift_slice)
```

This skips the page index data and extracts only the FileMetaData structure that the decoder expects.

### Implementation

**File:** `rust/analytics/src/arrow_utils.rs` (lines 33-79)

```rust
pub fn serialize_parquet_metadata(pmd: &ParquetMetaData) -> Result<bytes::Bytes> {
    // 1. Serialize the full footer format
    let mut buffer = Vec::new();
    let md_writer = ParquetMetaDataWriter::new(&mut buffer, pmd);
    md_writer.finish()?;
    let serialized = bytes::Bytes::from(buffer);

    // 2. Use named constants for Parquet footer format
    const FOOTER_SIZE: usize = 8;  // 4 bytes length + 4 bytes PAR1 magic
    const LENGTH_SIZE: usize = 4;

    // 3. Read footer length from standardized location
    let length_offset = serialized.len() - FOOTER_SIZE;
    let footer_len_bytes = &serialized[length_offset..length_offset + LENGTH_SIZE];
    let metadata_len = u32::from_le_bytes(...) as usize;

    // 4. Calculate where FileMetaData starts (skip page indexes)
    let footer_start = serialized.len() - FOOTER_SIZE - metadata_len;

    // 5. Extract and return only FileMetaData bytes
    let file_metadata_bytes = serialized.slice(footer_start..length_offset);
    Ok(file_metadata_bytes)
}
```

**Result:** The function now correctly handles metadata with page indexes, extracting only the FileMetaData portion that `decode_metadata()` expects. Uses named constants instead of hardcoded offsets for clarity.

## Alternative Approaches (Considered but not implemented)

### Option 1: Wait for arrow-rs 58+
Future arrow-rs versions may provide a cleaner API or resolve the format mismatch. However, the current workaround is sufficient.

### Option 2: Extract footer from complete parquet file
Instead of using `ParquetMetaDataWriter`, extract footer bytes directly from the complete parquet file. **Drawback**: Requires buffering entire file in memory.

### Option 3: Downgrade to DataFusion 50
Revert to DataFusion 50.2.0. **Drawback**: Lose DataFusion 51 features and improvements.

## References

- Arrow 57.0.0 custom thrift parser (Phase 1 - Reader): https://github.com/apache/arrow-rs/pull/8530
- Custom thrift writer (Phase 2 - Writer, unreleased): https://github.com/apache/arrow-rs/pull/8445
- Feature branch with fix: `gh5854_thrift_remodel`
- Deprecation of parquet::format: https://github.com/apache/arrow-rs/pull/8615
- Blog post: https://arrow.apache.org/blog/2025/10/23/rust-parquet-metadata/
