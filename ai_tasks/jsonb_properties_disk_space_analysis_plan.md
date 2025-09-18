# JSONB Properties Disk Space Analysis Plan

## Objective
Investigate the disk space implications of representing properties using a JSON-like structure instead of the current List<Struct> approach in the Micromegas lakehouse/analytics layer (Arrow/Parquet format).

## Background
Currently, Micromegas represents properties in the analytics layer using Apache Arrow/Parquet **List<Struct<key: String, value: String>>** format. This provides efficient columnar analytics but may have storage overhead due to the repeated "key"/"value" field names and struct overhead for each property.

This investigation focuses specifically on the **lakehouse/Parquet representation** where properties are materialized for DataFusion SQL queries. The comparison is between:

- **Current approach**: `List<Struct<key: String, value: String>>` in Arrow/Parquet
- **Alternative approach**: JSONB representation (leveraging existing JSONB integration in analytics crate)

The goal is to determine if a JSONB-based representation could reduce disk space usage in the Parquet files while maintaining query performance for the DataFusion analytics engine.

## Investigation Steps

### Phase 1 Analysis Summary ✅ COMPLETED

**Key Findings from Current Implementation:**

1. **Limited Scope**: Only 2 of 4 tables store properties (log entries and metrics), spans and async events don't use properties
2. **Schema Consistency**: Both tables use identical `List<Struct<key: String, value: String>>` format
3. **Storage Overhead**: Each property requires:
   - List array overhead (offset array, validity bitmap)
   - Struct array overhead (field metadata)
   - Two separate string columns for key and value
   - No key deduplication across events
4. **Performance Considerations**:
   - Linear search required for property access (`property_get` UDF)
   - Dictionary encoding optimization exists (`properties_to_dict` UDF)
   - Both regular and dictionary-encoded array support in UDFs
5. **JSONB Integration**: Analytics crate already has JSONB support infrastructure

**Implications for JSONB Comparison:**
- Smaller scope than initially expected (only log entries and metrics tables)
- Current approach already optimized with dictionary encoding for memory efficiency
- Linear search performance will be key comparison point
- JSONB could eliminate struct overhead and enable key deduplication

### 1. Understand Current Properties Storage Implementation in Lakehouse ✅ COMPLETED
- [x] **Analyze current Arrow/Parquet schema for properties representation**
  - **Log entries table**: Two property columns - `properties` and `process_properties`
  - **Metrics table**: Identical schema - `properties` and `process_properties`
  - **Spans table**: No properties columns (spans don't store properties in lakehouse)
  - **Schema**: `List<Struct<key: String, value: String>>` where struct has exactly 2 fields
  - **Parquet implications**: Each property requires List overhead + Struct overhead + 2 string columns

- [x] **Examine the Arrow conversion code in analytics crate**
  - `arrow_properties.rs`: Builds properties via `ListBuilder<StructBuilder>` with key/value string builders
  - `property_get.rs` UDF: Searches through struct arrays linearly to find property by key name
  - `properties_to_dict_udf.rs`: Converts List<Struct> to dictionary-encoded format for memory efficiency
  - **Key insight**: Current approach supports both regular arrays and dictionary-encoded arrays for optimization

- [x] **Document current storage patterns in Parquet files**
  - **Property structure**: Each event can have 0-N properties stored as a list of key-value structs
  - **Duplication**: Property keys are repeated for every event (no deduplication at schema level)
  - **Memory optimization**: `properties_to_dict` UDF exists specifically to reduce memory usage during queries
  - **Access pattern**: Linear search through property list via `property_get` UDF

- [x] **Identify all Parquet tables that store properties**
  - **Tables with properties**: `log_entries_table.rs`, `metrics_table.rs`
  - **Tables without properties**: `span_table.rs`, `async_events_table.rs`
  - **Schema consistency**: Both tables use identical property column definitions
  - **Column names**: `properties` (event-specific) and `process_properties` (process-level context)

### 2. Design JSONB Schema for Parquet ✅ COMPLETED
- [x] **Design JSONB schema**: `properties` column as JSONB object `{"key1": "value1", "key2": "value2"}`
  - **Direct key-value mapping** in JSON object format
  - **Eliminates struct overhead** and "key"/"value" field repetition
  - **Enables natural property access** via JSON path operators
- [x] **Leverage existing JSONB integration** in analytics crate for Arrow conversion
- [x] **Consider Parquet compression implications** for JSONB serialization (dictionary encoding, RLE, etc.)
- [x] **Evaluate impact** on existing DataFusion UDFs and query patterns with JSONB access

### 3. Implement Properties-to-JSONB Conversion UDF 🎯 NEXT PHASE
- [ ] Create `properties_to_jsonb` UDF in analytics crate:
  - Input: `List<Struct<key: String, value: String>>`
  - Output: `String` (JSONB serialized object format `{"key1": "value1", "key2": "value2"}`)
  - Leverage existing JSONB serialization infrastructure
  - Handle empty properties lists and null values appropriately
- [ ] Create reverse `jsonb_to_properties` UDF for testing:
  - Input: `String` (JSONB object)
  - Output: `List<Struct<key: String, value: String>>`
  - Enable round-trip conversion validation
- [ ] Add comprehensive tests for both UDFs:
  - Empty properties, single property, multiple properties
  - Special characters in keys/values, null handling
  - Performance benchmarks vs existing `property_get` UDF
- [ ] Register UDFs in DataFusion session context for query testing

### 4. Create Disk Space Estimation Model for Parquet Storage
- [ ] Build calculation model for current List<Struct> approach overhead:
  - Arrow List array overhead (offset arrays, validity bitmaps)
  - Struct array overhead per property (field metadata, child arrays)
  - String dictionary encoding efficiency for repeated keys
  - Parquet column chunk compression ratios
- [ ] Build calculation model for JSONB approach:
  - JSONB serialized string storage overhead in Parquet
  - JSONB parsing overhead vs. direct struct access in DataFusion
  - Compression characteristics of JSONB strings in Parquet
  - Dictionary encoding potential for JSONB serialized data
  - Leverage existing JSONB-to-Arrow conversion performance metrics
- [ ] Account for Parquet file-level optimizations (column pruning, predicate pushdown)

### 5. Implement Proof of Concept Test with Parquet Files
- [ ] Create test Arrow schemas for both storage approaches
- [ ] Generate representative test data sets:
  - High-frequency events (100k+ events/second scenarios)
  - Varying property counts (1-50 properties per event)
  - Mixed property types (strings, numbers, booleans)
  - Realistic key repetition patterns
- [ ] Create Parquet files using both approaches with identical logical data:
  - Original format: `properties` as `List<Struct>`
  - JSONB format: `jsonb_properties` using `properties_to_jsonb` UDF
- [ ] Measure actual file sizes and compression ratios
- [ ] Analyze Parquet metadata and column statistics

### 6. Performance Impact Assessment for DataFusion Queries
- [ ] Benchmark query performance for common property access patterns:
  - Filter by specific property values (struct access vs. JSONB query operators)
  - Aggregate queries involving properties
  - Property existence checks
  - Range queries on property values
- [ ] Test DataFusion's JSONB handling capabilities and performance (using existing integration)
- [ ] Measure query compilation time differences (Arrow schema complexity)
- [ ] Benchmark memory usage during query execution for both approaches
- [ ] Compare `property_get` vs `jsonb_get` UDF performance for property access

### 7. Storage Efficiency Analysis for Parquet Files
- [ ] Compare Parquet file sizes across different data volumes:
  - Small datasets (1M events)
  - Medium datasets (100M events)
  - Large datasets (1B+ events)
- [ ] Analyze compression ratios for both approaches across different Parquet compression algorithms
- [ ] Document storage growth patterns and compression effectiveness over time
- [ ] Measure impact on Parquet file metadata size and column statistics

### 8. Cost-Benefit Analysis for Lakehouse Storage
- [ ] Calculate storage cost differences for cloud object storage (S3, GCS)
- [ ] Factor in query performance changes and compute costs
- [ ] Consider development effort for Arrow schema migration
- [ ] Evaluate impact on analytics query latency and throughput
- [ ] Assess effect on backup/restore operations for Parquet files

### 9. Documentation and Recommendations
- [ ] Document findings with concrete disk usage numbers
- [ ] Create comparison matrix of trade-offs
- [ ] Provide recommendation with supporting data
- [ ] Outline implementation strategy if JSONB approach is favorable

## Key Questions to Answer
1. What is the Parquet file size difference between `List<Struct>` vs JSONB representation at scale?
2. How does DataFusion query performance change with JSONB operations vs. direct struct access?
3. What are the compression advantages/disadvantages in Parquet for JSONB vs struct approach?
4. How does Arrow memory usage differ during query execution?
5. What is the impact on existing DataFusion UDFs and query patterns with JSONB?
6. How do the approaches compare for common property access patterns (filters, aggregations)?
7. What are the migration complexity and risks for existing Parquet files (considering existing JSONB integration)?

## Dependencies
- Access to representative Parquet files from existing Micromegas lakehouse deployments
- DataFusion test environment with Arrow/Parquet capabilities
- Understanding of current property distribution patterns in materialized views
- Knowledge of existing `property_get` and `properties_to_dict` UDF performance characteristics
- Familiarity with Parquet compression algorithms and Arrow schema optimization

## Timeline Estimate
- Investigation and analysis: 3-4 days
- Proof of concept implementation: 2-3 days
- Testing and measurement: 2-3 days
- Documentation and recommendations: 1-2 days

**Total estimated effort: 8-12 days**

## Risk Factors
- Representative test data may not capture all real-world property distribution scenarios in Parquet files
- DataFusion JSONB handling performance may vary significantly from Arrow struct access
- Migration complexity could affect existing analytics workflows and view materialization
- Arrow schema changes might require updates to analytics service UDFs (though existing JSONB integration may minimize this)
- Performance regression risk in query execution time for property-heavy workloads
- Potential limitations in DataFusion's JSONB query optimization capabilities