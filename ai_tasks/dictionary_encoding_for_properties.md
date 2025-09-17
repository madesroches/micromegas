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
1. **Update Data Model**
   - Modify Arrow schema from `List<Struct<key: Utf8, value: Utf8>>` to use dictionary encoding
   - Update `ListBuilder<StructBuilder>` in `log_entries_table.rs` and `metrics_table.rs`
   - Implement dictionary builder utilities for property keys and values

2. **Migration Strategy**
   - Create conversion functions for existing data
   - Implement backward compatibility layer
   - Plan rollback procedure if needed

3. **Query Processing Updates**
   - Update `PropertiesColumnReader` in `sql_arrow_bridge.rs` to output dictionary arrays
   - Modify property filtering logic in lakehouse modules
   - Ensure UDFs (`property_get`, `properties_to_dict`) work with dictionary encoding

### Phase 3: Testing and Validation
1. **Unit Tests**
   - Test dictionary creation and access
   - Verify query correctness with encoded data
   - Test edge cases (empty, null, large dictionaries)

2. **Integration Tests**
   - End-to-end ingestion with dictionary encoding
   - Query performance tests
   - Memory usage comparisons

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
- `rust/analytics/src/sql_arrow_bridge.rs` - PropertiesColumnReader to output dictionary arrays
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

## Success Criteria
- All properties columns use dictionary encoding
- No regression in query correctness
- Measurable improvement in memory usage
- Performance improvement for common queries
- Backward compatibility maintained
