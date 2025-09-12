# Task: Make property_get return Dictionary instead of Array

## Problem Statement
The `property_get` UDF currently returns a string array, but dictionary encoding provides significant memory savings (28.2 KB vs 11.9 KB for the same data). Users need to manually cast to dictionary type which is inefficient.

## Justification
- **Memory efficiency**: ~58% reduction in memory usage (11.9 KB vs 28.2 KB)
- **Performance**: Dictionary encoding reduces redundancy for repeated values
- **User experience**: Eliminates need for manual `arrow_cast` operations

## Implementation Plan

### 1. Update property_get_function.rs
- Modify return type from `Utf8Array` to `DictionaryArray<Int32Type>`
- Build dictionary during extraction to deduplicate values
- Use Arrow's `DictionaryBuilder` for efficient construction

### 2. Key Technical Changes
```rust
// Current signature
property_get(properties, key) -> Utf8Array

// New signature  
property_get(properties, key) -> DictionaryArray<Int32Type, Utf8Array>
```

### 3. Implementation Steps
1. Update function signature and return type declaration
2. Replace `StringBuilder` with `StringDictionaryBuilder<Int32Type>`
3. Modify value extraction logic to use dictionary builder
4. Update function registration to reflect new return type
5. Ensure backward compatibility or migration path

### 4. Testing Requirements
- Update existing tests in `properties_to_dict_tests.rs`
- Add test cases for:
  - High cardinality values (many unique strings)
  - Low cardinality values (few unique strings, high repetition)
  - Null handling
  - Empty properties
- Benchmark memory usage and query performance

### 5. Validation
- Compare output with manual `arrow_cast` approach
- Verify identical results with better performance
- Test with production-like data volumes

## Benchmark Results (Reference)

### Current Implementation (String Array)
```sql
SELECT property_get(properties, 'build-version') as version
FROM processes
```
- **Schema**: `version: string`
- **Memory**: 28.2 KB
- **Rows**: 2,156
- **Wall time**: 5.21s

### With Manual Dictionary Cast
```sql
SELECT arrow_cast(property_get(properties, 'build-version'), 'Dictionary(Int32, Utf8)') as version
FROM processes
```
- **Schema**: `version: dictionary<values=string, indices=int32, ordered=0>`
- **Memory**: 11.9 KB
- **Rows**: 2,156
- **Wall time**: 5.06s

### Performance Improvements
- **Memory reduction**: 58% (16.3 KB saved)
- **Query time**: 3% faster (0.15s improvement)
- **Compression ratio**: 2.37x

## Expected Benefits
- Automatic dictionary encoding without user intervention
- Reduced memory footprint for queries (demonstrated 58% reduction)
- Better cache utilization
- Improved query performance for downstream operations
- Elimination of manual `arrow_cast` operations

## Risks & Mitigation
- **Breaking change**: Existing queries expecting string arrays will need updates
  - Mitigation: Consider feature flag or versioned function
- **Performance overhead**: Dictionary building has small overhead for unique values
  - Mitigation: Profile and optimize builder usage

## Success Criteria
- Memory usage reduction matches manual casting approach (~58% savings)
- No performance regression for typical queries
- Clean API without requiring manual type conversions