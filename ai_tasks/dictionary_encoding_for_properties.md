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

### Phase 1: Analysis and Design
1. **Analyze Current Implementation**
   - Identify all tables with properties columns
   - Document current storage format and access patterns

2. **Design Dictionary Schema**
   - Define dictionary key type (String/Utf8)
   - Use Int32 as index type (supports up to 2B unique values)
   - Dictionary overflow will use normal error reporting (fail fast with clear error message)
   - Follow existing builder patterns for dictionary management (per-batch dictionaries)

### Phase 2: Core Implementation
1. **Update Data Model**
   - Modify Arrow schema builders to use DictionaryArray for properties
   - Update batch creation in ingestion pipeline
   - Implement dictionary builder utilities

2. **Migration Strategy**
   - Create conversion functions for existing data
   - Implement backward compatibility layer
   - Plan rollback procedure if needed

3. **Query Processing Updates**
   - Update DataFusion queries to handle dictionary arrays
   - Modify property filtering logic
   - Ensure UDFs work with dictionary encoding

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
- `rust/analytics/src/lakehouse/` - Core lakehouse implementation
- `rust/analytics/src/arrow_utils/` - Arrow utilities
- `rust/analytics/src/sql_arrow_bridge.rs` - SQL to Arrow type conversions
- `rust/ingestion/src/` - Ingestion pipeline
- `rust/analytics/src/udfs/` - Update UDFs for dictionary support
- Tests across the codebase

## Success Criteria
- All properties columns use dictionary encoding
- No regression in query correctness
- Measurable improvement in memory usage
- Performance improvement for common queries
- Backward compatibility maintained
