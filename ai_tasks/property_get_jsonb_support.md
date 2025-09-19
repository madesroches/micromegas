# Task: Enable property_get to Access JSONB Columns

## Objective
Extend the `property_get` UDF to support extracting properties from dictionary-encoded JSONB columns, paving the way for a schema change where properties are stored as `Dictionary<Int32, Binary>` (JSONB) instead of `List<Struct<key, value>>`.

## Background
Currently, properties are stored as `List<Struct<key: String, value: String>>` and accessed via the `property_get` UDF. With the introduction of `properties_to_jsonb` UDF, we can now convert properties to JSONB format. The next step is to enable `property_get` to read from dictionary-encoded JSONB columns directly, where the new format will be `Dictionary<Int32, Binary>` with the Binary containing JSONB objects.

## Current State Analysis

### Existing Components:
1. **property_get UDF** (`rust/analytics/src/properties/property_get.rs`)
   - Currently handles:
     - List<Struct> (primary current format)
     - Dictionary<Int32, List<Struct>> (when List<Struct> gets dictionary-encoded by DataFusion)
   - Returns: Dictionary<Int32, Utf8>
   - Core logic: Iterates through struct array to find matching key

2. **JSONB Infrastructure** (`rust/analytics/src/dfext/jsonb/`)
   - `jsonb_get`: Extracts values from JSONB by name
   - Uses `RawJsonb` from jsonb crate
   - Returns Binary (JSONB) type

3. **properties_to_jsonb UDF**
   - Converts List<Struct> → Binary (JSONB)
   - Creates JSONB object format: {"key1": "value1", ...}
   - Can be wrapped in Dictionary<Int32, Binary> for efficient storage

## Design Approach

### Key Design Decisions:
1. **New Storage Format**: Properties will be stored as `Dictionary<Int32, Binary>`
   - Moving from: `List<Struct<key, value>>` (current)
   - Moving to: `Dictionary<Int32, Binary>` where Binary contains JSONB objects
   - Dictionary encoding provides compression for repeated property sets
   - JSONB provides efficient binary encoding of the property objects

2. **Input Type Support**: property_get will handle:
   - Dictionary<Int32, Binary> → **NEW: dictionary-encoded JSONB logic (future primary format)**
   - List<Struct> → **CURRENT: existing logic (current primary format)**
   - Dictionary<Int32, List<Struct>> → existing logic (when DataFusion dictionary-encodes List<Struct>)
   - Binary (JSONB) → support for non-dictionary JSONB (completeness)

3. **JSONB Extraction**:
   - Use `RawJsonb::get_by_name()` to extract values from JSONB bytes
   - Handle JSONB binary format directly without full deserialization
   - Convert extracted JSONB values to strings for return

4. **Return Type Consistency**:
   - Maintain Dictionary<Int32, Utf8> return type for consistency
   - Use StringDictionaryBuilder as before
   - Preserves dictionary encoding benefits in query results

### Implementation Steps:

1. **Add JSONB type detection in invoke_with_args()**:
   ```rust
   match args[0].data_type() {
       DataType::Dictionary(_, value_type) => {
           match value_type.as_ref() {
               DataType::Binary => // Handle dictionary-encoded JSONB (PRIMARY)
               DataType::List(_) => // Existing dictionary-encoded List<Struct>
           }
       }
       DataType::List(_) => // Existing non-dictionary logic
       DataType::Binary => // Handle non-dictionary JSONB
   }
   ```

2. **Create JSONB extraction function**:
   ```rust
   fn extract_from_jsonb(jsonb_bytes: &[u8], name: &str) -> anyhow::Result<Option<String>>
   ```

3. **Handle JSONB arrays**:
   - Direct Binary arrays
   - Dictionary-encoded Binary arrays

## Testing Strategy

### Unit Tests:
1. Test Dictionary<Int32, Binary> (new dictionary-encoded JSONB format)
2. Test direct Binary (non-dictionary JSONB) access
3. Test mixed null/non-null JSONB values
4. Test missing properties in JSONB objects
5. Test backward compatibility with List<Struct> (current primary format)
6. Test backward compatibility with Dictionary<Int32, List<Struct>> (when DataFusion dictionary-encodes)

### Integration Tests:
1. Query with JSONB properties column
2. Query mixing List and JSONB columns
3. Performance comparison between formats

## Migration Path

This change enables a gradual migration:
1. **Phase 1**: property_get supports both formats ✅ **COMPLETED**
   - List<Struct> (current format)
   - Dictionary<Int32, Binary> (new JSONB format)
   - Dictionary<Int32, List<Struct>> (DataFusion dictionary-encoded current format)
   - Binary (non-dictionary JSONB for completeness)
2. **Phase 2**: New data written as Dictionary<Int32, Binary>, old data remains as List<Struct>
3. **Phase 3**: Background migration of old data from List<Struct> to Dictionary<Int32, Binary>
4. **Phase 4**: Deprecate List<Struct> format support

## Success Criteria

1. ✅ property_get successfully extracts values from JSONB columns
2. ✅ All existing tests pass (backward compatibility)
3. ✅ New tests for JSONB functionality pass
4. ⏳ Performance is comparable or better than List<Struct> approach (benchmarks pending)
5. ✅ Dictionary encoding is preserved for efficient storage

## Implementation Checklist

- [x] Extend property_get to detect Dictionary<Int32, Binary> input type
- [x] Implement JSONB value extraction logic using RawJsonb
- [x] Handle Dictionary<Int32, Binary> as primary format
- [x] Support non-dictionary Binary for completeness
- [x] Add comprehensive unit tests for all formats
- [x] Verify backward compatibility with existing formats
- [ ] Run performance benchmarks comparing formats
- [ ] Update documentation

## Current Implementation Status ✅ COMPLETED

**Commit:** `6b567892` - feat: enable property_get UDF to access JSONB columns

### ✅ What Was Implemented:

1. **JSONB Type Detection**: Extended `invoke_with_args()` to handle:
   - `DataType::Binary` - non-dictionary JSONB arrays
   - `DataType::Dictionary(_, DataType::Binary)` - dictionary-encoded JSONB (primary new format)
   - Maintained existing support for List<Struct> formats

2. **JSONB Extraction Function**: Created `extract_from_jsonb()` that:
   - Uses `RawJsonb::get_by_name()` for property extraction
   - Properly handles string unescaping via `as_str()` method
   - Falls back to JSON representation for non-string values
   - Returns `Option<String>` with proper NULL semantics

3. **Dictionary Encoding Support**: Full support for `Dictionary<Int32, Binary>`:
   - Extracts JSONB from dictionary values array
   - Preserves dictionary encoding benefits in output
   - Handles null dictionary entries correctly

4. **Comprehensive Test Coverage** (10 tests total):
   - **Binary JSONB**: Direct binary array access
   - **Dictionary-encoded JSONB**: Dictionary<Int32, Binary> format
   - **Missing properties**: Returns SQL NULL (not string "null")
   - **Null JSONB**: Handles null binary entries
   - **Escaped strings**: Validates proper unescaping of quotes, newlines, tabs, unicode
   - **NULL vs "null"**: Distinguishes missing properties (SQL NULL) from JSON null values (string "null")
   - **Backward compatibility**: All existing List<Struct> tests pass

### ✅ Key Technical Achievements:

- **Proper String Handling**: Fixed initial quote-stripping approach to use `RawJsonb.as_str()` for correct JSON string unescaping
- **NULL Semantics**: Correctly handles distinction between missing properties and explicit JSON null values
- **Performance**: Maintains dictionary encoding throughout the pipeline for memory efficiency
- **Backward Compatibility**: 100% compatible with existing List<Struct> and Dictionary<Int32, List<Struct>> formats

### ✅ Test Results:
```
running 10 tests
test test_property_get_returns_dictionary ... ok
test test_property_get_null_vs_missing_jsonb_properties ... ok
test test_property_get_with_binary_jsonb ... ok
test test_property_get_with_missing_jsonb_property ... ok
test test_property_get_with_escaped_jsonb_strings ... ok
test test_property_get_with_dictionary_encoded_jsonb ... ok
test test_property_get_with_nulls ... ok
test test_property_get_with_repeated_values ... ok
test test_property_get_with_null_jsonb ... ok
test test_property_get_return_type ... ok

test result: ok. 10 passed; 0 failed
```

## Notes

- Moving from `List<Struct<key, value>>` to `Dictionary<Int32, Binary>` provides:
  - Dictionary encoding for repeated property sets (major compression win)
  - JSONB binary format for compact object representation
  - Better memory locality and cache performance
- JSONB format is more space-efficient for sparse properties
- This change is fully backward compatible - existing queries with List<Struct> continue to work
- The new format Dictionary<Int32, Binary> will become the primary storage format
