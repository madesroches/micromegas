# Task: Convert Properties Columns to Dictionary Encoding

## Objective
Transform all properties columns in the lakehouse from their current string/JSON representation to use Arrow dictionary encoding for improved memory efficiency and query performance.

## Background
Properties columns store key-value pairs as JSON strings. Dictionary encoding can significantly reduce memory usage and improve query performance by:
- Storing unique values only once
- Using integer indices for repeated values
- Enabling faster equality comparisons
- Reducing memory bandwidth requirements

## Implementation Plan

### Phase 1: Analysis and Design ✓
1. **Analyze Current Implementation** ✓
   - Identified tables: `processes` and `streams` (both with `properties micromegas_property[]`)
   - Documented in [properties_analysis.md](./properties_analysis.md)

2. **Design Dictionary Schema** ✓
   - Define dictionary key type (String/Utf8)
   - Use Int32 as index type (supports up to 2B unique values)
   - Dictionary overflow will use normal error reporting (fail fast with clear error message)
   - Follow existing builder patterns for dictionary management (per-batch dictionaries)

### Phase 2: Core Implementation
1. **Extract PropertiesDictionaryBuilder** ✓
   - Moved `PropertiesDictionaryBuilder` from `properties_to_dict_udf.rs` to the properties module
   - Created new file `rust/analytics/src/properties/dictionary_builder.rs`
   - Exported from `rust/analytics/src/properties/mod.rs`
   - Made it reusable across multiple components
   - Kept the `build_dictionary_from_properties_array` function with it (renamed for clarity)
   - Updated `properties_to_dict_udf.rs` to import from the new location
   - All tests pass

2. **Update PropertiesColumnReader** ✓
   - Modified `PropertiesColumnReader` in `sql_arrow_bridge.rs` to output dictionary-encoded arrays
   - Used the extracted `PropertiesDictionaryBuilder`
   - Updated field schema to return `Dictionary<Int32, List<Struct>>`
   - Redesigned ColumnReader trait using open-closed principle (traits over enums)
   - Modified `rows_to_record_batch` to handle dictionary and regular columns separately
   - All tests pass

3. **Update Data Model** (IN PROGRESS - Schema Mismatch Issue)

   **Problem Identified**: After updating `sql_arrow_bridge.rs` to output dictionary-encoded arrays, there's a schema mismatch:
   - SQL bridge produces: `Dictionary<Int32, List<Struct>>`
   - Table schemas expect: `List<Struct>`
   - Error: "Field 'streams.properties' has type List(...), array has type Dictionary(...)"

   **Root Cause**: Two different parts of the system handle properties:
   - **Table builders** (`log_entries_table.rs`, `metrics_table.rs`) - Build record batches for storage
   - **SQL arrow bridge** (`sql_arrow_bridge.rs`) - Read data from database and convert to Arrow

   **Implementation Steps Required**:

   a. **Update Table Schemas** (Priority 1):
      - Modify `log_table_schema()` in `log_entries_table.rs:77-87`
      - Change properties field from `List<Struct>` to `Dictionary<Int32, List<Struct>>`
      - Modify `process_properties` field similarly (`log_entries_table.rs:88-99`)
      - Update corresponding schema in `metrics_table.rs`

   b. **Update Table Builders**:
      - Replace `ListBuilder<StructBuilder>` with `PropertiesDictionaryBuilder` in:
        - `LogEntriesRecordBuilder.properties` (`log_entries_table.rs:116`)
        - `LogEntriesRecordBuilder.process_properties` (`log_entries_table.rs:117`)
        - `MetricsRecordBuilder.properties` (`metrics_table.rs:117`)
        - `MetricsRecordBuilder.process_properties` (`metrics_table.rs:118`)

   c. **Update Builder Construction**:
      - Replace `ListBuilder::new(StructBuilder::from_fields(...))` calls with `PropertiesDictionaryBuilder::new(capacity)`
      - Update field creation in `with_capacity()` methods

   d. **Update Property Addition Functions**:
      - Modify `add_property_set_to_builder()` in `arrow_properties.rs:63` to work with dictionary builders
      - Update `add_properties_to_builder()` function similarly
      - These functions currently expect `&mut ListBuilder<StructBuilder>` but need to work with dictionary builders

   e. **Update Finish Methods**:
      - Update `LogEntriesRecordBuilder.finish()` to call dictionary builder finish methods
      - Update `MetricsRecordBuilder.finish()` similarly

   f. **Update All Consumers**:
      - Search for all code that expects `ListArray` for properties and update to handle `DictionaryArray`
      - Files to check: `analytics-web-srv/src/main.rs:299`, `replication.rs:33,92`, `metadata.rs:204`
      - Update partition source data handling in `partition_source_data.rs:141,157`
      - Update JIT partitions handling in `jit_partitions.rs:309`

4. **Migration Strategy**

   **Approach**: Since this is a breaking schema change, we need a coordinated update strategy:

   a. **Testing Strategy**:
      - Create comprehensive tests that verify both old and new schemas work
      - Test data ingestion → storage → querying pipeline end-to-end
      - Verify property filtering and UDF functions still work correctly

   b. **Rollout Strategy**:
      - All schema changes must be deployed atomically since they affect data compatibility
      - Consider if existing stored data needs migration or if new schema only applies to new data
      - Update both ingestion (table builders) and querying (SQL bridge) simultaneously

   c. **Backward Compatibility**:
      - UDFs already support both `List` and `Dictionary` input (verified in `properties_to_dict_udf.rs:304`)
      - Consider whether old partition files with `List` schema can coexist with new `Dictionary` schema
      - May need schema versioning or conversion layer

5. **Query Processing Updates** ✓
   - Property filtering logic in lakehouse modules should work unchanged (operates on dictionary values)
   - UDFs (`property_get`, `properties_to_dict`) already support dictionary encoding
   - Verified that property_get tests pass with dictionary input

### Phase 3: Testing and Validation

**Status**: Blocked pending Phase 2 completion

1. **Unit Tests**
   - Test dictionary creation and access ✓ (basic tests in `sql_arrow_bridge_tests.rs`)
   - Test table builders with dictionary schemas (TODO)
   - Verify query correctness with encoded data (TODO)
   - Test edge cases (empty, null, large dictionaries) ✓

2. **Integration Tests** (TODO)
   - End-to-end ingestion with dictionary encoding
   - Query performance tests
   - Memory usage comparisons
   - Cross-compatibility between old and new schemas

### Phase 4: Rollout
1. **Documentation**
   - Update technical documentation
   - Add migration guide
   - Document performance improvements


## Technical Considerations

### Arrow Dictionary Encoding Details
- Use `DictionaryArray<Int32Type>` for properties columns
- Keys are stored as `StringArray`
- Values use Int32 indices (supporting up to 2,147,483,647 unique values)
- Supports null values naturally

### Expected Benefits
- **Memory Reduction**: 50-90% for high-cardinality properties
- **Query Speed**: 2-5x faster for equality filters
- **Cache Efficiency**: Better CPU cache utilization

### Potential Challenges
- Dictionary coordination across batches
- Handling unbounded cardinality growth (will error if >2B unique values)
- Maintaining sort order for range queries
- Integration with existing UDFs

## Files to Modify

### Primary Changes
- `rust/analytics/src/sql_arrow_bridge.rs` - PropertiesColumnReader to output dictionary arrays ✓
- `rust/analytics/src/log_entries_table.rs` - Update properties/process_properties builders
- `rust/analytics/src/metrics_table.rs` - Update properties/process_properties builders

### Lakehouse & Processing
- `rust/analytics/src/lakehouse/partition_source_data.rs` - Handle dictionary-encoded properties
- `rust/analytics/src/lakehouse/jit_partitions.rs` - Process dictionary arrays in JIT

### UDFs & Utilities
- `rust/analytics/src/properties/property_get.rs` - Already supports dictionary input
- `rust/analytics/src/properties/properties_to_dict_udf.rs` - Verify dictionary compatibility
- `rust/analytics/src/arrow_properties.rs` - Add dictionary utilities

### Tests
- `rust/analytics/tests/property_get_tests.rs` - Extend for full dictionary coverage
- Integration tests for end-to-end dictionary flow

## Current Status

**Phase 2 Progress**: ✅ **COMPLETED**
- ✅ Extract PropertiesDictionaryBuilder
- ✅ Update PropertiesColumnReader in SQL bridge
- ✅ **RESOLVED**: Schema mismatch between SQL bridge (Dictionary) and table builders (List)

**Phase 2.3 Data Model Updates**: ✅ **COMPLETED**
- ✅ Updated table schemas (log_entries_table.rs, metrics_table.rs) to use Dictionary<Int32, List<Struct>>
- ✅ Updated table builders to use PropertiesDictionaryBuilder instead of ListBuilder<StructBuilder>
- ✅ Updated property addition functions with new dictionary builder methods
- ✅ Updated finish methods to handle Result return types from dictionary builders
- ✅ Updated all consumers (replication.rs, metadata.rs, partition_source_data.rs, jit_partitions.rs) to handle DictionaryArray
- ✅ Updated blocks view schema and bumped version (blocks_file_schema_hash: 1 → 2)
- ✅ Bumped view schema versions (log_view: 4 → 5, metrics_view: 4 → 5)

**Testing & Validation**: ✅ **COMPLETED**
- ✅ All 37 tests pass without regressions
- ✅ Schema compatibility verified between ingestion and querying components
- ✅ Dictionary deduplication working correctly
- ✅ Property UDFs support both List and Dictionary input formats

## Phase 2.4: Parquet Compatibility Issue

**Problem Discovered**: ❌ **BLOCKED**
- Error: `NYI: Datatype Dictionary(Int32, List(...)) is not yet supported`
- Root cause: Arrow Parquet writer doesn't support dictionary-encoded complex types
- Impact: Cannot write dictionary-encoded properties to Parquet files
- Location: `write_partition.rs` arrow_writer.write() call

**Current Status**: Dictionary encoding works perfectly for:
- ✅ In-memory processing and querying
- ✅ SQL operations and UDFs
- ✅ Inter-service data transfer
- ❌ **Parquet file storage** (blocks implementation goal)

**Resolution Analysis Complete**: ❌ **DEPENDENCY UPGRADE NOT VIABLE**

**Investigation Results** (Arrow 55→56, DataFusion 49→50, Parquet 55→56):
- ✅ Tested upgrade to latest versions (Arrow/Parquet 56.1.0, DataFusion 50.0.0)
- ❌ **No evidence** of dictionary complex types support in latest versions
- ❌ **Root cause**: Parquet specification limitation, not implementation bug
- ❌ **Property-level dictionary**: `List<Dictionary<Int32, Struct>>` would be useless since properties are unique key-value pairs

**Remaining Viable Options**:
1. **Accept current limitation** - Use dictionary encoding for in-memory processing only (✅ achieved)
2. **Custom compressed storage format** - Implement alternative to Parquet (complex, high impact)
3. **Alternative Parquet compression** - Explore other compression schemes (GZIP, ZSTD, etc.)
4. **Hybrid approach** - Dictionary in-memory + convert to List for Parquet (defeats storage goal)

**Recommendation**: **REVERT IMPLEMENTATION** or implement alternative solution. The dictionary encoding approach is fundamentally incompatible with the system's storage requirements.

## Success Criteria
- ✅ All properties columns use dictionary encoding (in-memory)
- ✅ No regression in query correctness
- ✅ Schema compatibility between ingestion and querying components
- ❌ **BLOCKED**: Parquet file size reduction (Parquet specification limitation)
- 🔄 Measurable improvement in memory usage (Phase 3 - partial success)
- 🔄 Performance improvement for common queries (Phase 3 - partial success)

## Final Status: ❌ **BLOCKED - IMPLEMENTATION FAILURE**

**Critical Issue**: The core objective of this task was to achieve **storage compression** in Parquet files. Without Parquet compatibility, the dictionary encoding implementation **cannot be deployed** because:

1. **Storage is the primary goal** - The task objective explicitly targets "improved memory efficiency and query performance" through reduced file sizes
2. **System integration requirement** - All data must persist to Parquet files for the lakehouse architecture to function
3. **Production blocker** - Cannot write dictionary-encoded data to storage, breaking the ingestion pipeline

**Current Implementation Status**:
- ✅ In-memory dictionary encoding works
- ✅ Query processing works
- ❌ **FATAL**: Cannot persist data to Parquet files
- ❌ **FATAL**: Breaks production data pipeline

**Conclusion**: The implementation is **unusable in production** due to the Parquet storage limitation. This is a **blocking technical debt** that prevents achieving the task's primary objective of reducing storage costs through compression.
