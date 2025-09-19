# Properties to Dictionary-Encoded JSONB Migration Plan

## Executive Summary

This plan outlines the migration from the current properties storage format (`Array<Struct<key: String, value: String>>`) to dictionary-encoded JSONB in the Micromegas lakehouse. This change will significantly improve storage efficiency and query performance for properties data while maintaining full backward compatibility.

## Current State Analysis

### 1. Database Schema (PostgreSQL)
- **Type Definition**: `micromegas_property` as `(key TEXT, value TEXT)`
- **Storage**: `micromegas_property[]` arrays in:
  - `processes.properties`
  - `streams.properties`

### 2. Arrow Schema (Analytics Layer)
Currently uses `List<Struct<key: String, value: String>>` format:
```rust
DataType::List(Arc::new(Field::new(
    "Property",
    DataType::Struct(Fields::from(vec![
        Field::new("key", DataType::Utf8, false),
        Field::new("value", DataType::Utf8, false),
    ])),
    false,
)))
```

### 3. Target Schema (Dictionary-Encoded JSONB)
Will transition to:
```rust
DataType::Dictionary(
    Box::new(DataType::Int32),
    Box::new(DataType::Binary),
)
```

## Impact Assessment

### Impacted View Sets (5 of 7 total)

#### High Priority - Core Metadata Views
1. **processes** (`/rust/analytics/src/lakehouse/processes_view.rs`)
   - Schema: `properties` field
   - Usage: Process metadata storage

2. **streams** (`/rust/analytics/src/lakehouse/streams_view.rs`)
   - Schema: `properties` field
   - Usage: Stream metadata storage

#### High Priority - Data Views
3. **blocks** (`/rust/analytics/src/lakehouse/blocks_view.rs`)
   - Schema: `streams.properties` + `processes.properties`
   - Usage: Union view with both property types

4. **log_entries** (`/rust/analytics/src/log_entries_table.rs`)
   - Schema: `properties` + `process_properties` fields
   - Usage: High-volume log data with metadata

5. **measures** (`/rust/analytics/src/metrics_table.rs`)
   - Schema: `properties` + `process_properties` fields
   - Usage: High-volume metrics data with metadata

#### Not Impacted
- **async_events**: No properties fields (optimized for high-frequency data)
- **thread_spans**: No properties fields (focused on timing data)

## Implementation Plan

### Phase 1: sqlx-arrow Bridge Enhancement

#### 1.1 Modify PropertiesColumnReader
**Strategy Change**: Instead of adding a new JSONB column reader, modify the existing `PropertiesColumnReader` in `/rust/analytics/src/sql_arrow_bridge.rs` to output dictionary-encoded JSONB format.

**Changes needed:**
1. **Update field() method** to return dictionary-encoded JSONB schema:
```rust
fn field(&self) -> Field {
    Field::new(
        self.field.name(),
        DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Binary),
        ),
        true,
    )
}
```

2. **Update extract_column_from_row()** to:
   - Extract `Vec<Property>` from PostgreSQL (unchanged)
   - Convert to JSONB binary format using existing logic from `properties_to_jsonb_udf.rs`
   - Use `BinaryDictionaryBuilder<Int32Type>` instead of `ListBuilder`

#### 1.2 Required Imports
Add to `/rust/analytics/src/sql_arrow_bridge.rs`:
```rust
use datafusion::arrow::array::BinaryDictionaryBuilder;  // Currently missing
use jsonb::Value;                                        // For JSONB creation
use std::borrow::Cow;                                   // For JSONB string values
use std::collections::BTreeMap;                        // For key-value mapping
```

**Verified**: The file currently has array builder imports but is missing `BinaryDictionaryBuilder`.

### Phase 2: Blocks Table Schema Update

#### 2.1 Update blocks_view_schema()
Modify `/rust/analytics/src/lakehouse/blocks_view.rs` to use dictionary-encoded JSONB for properties fields:

**Changes needed (verified line numbers):**
- Lines 186-197: `streams.properties` field
- Lines 222-233: `processes.properties` field

**New schema definition:**
```rust
Field::new(
    "streams.properties",
    DataType::Dictionary(
        Box::new(DataType::Int32),
        Box::new(DataType::Binary),
    ),
    false,
),
// ... same for processes.properties
```

#### 2.2 Update Schema Hash (CRITICAL)
**MUST** increment `blocks_file_schema_hash()` from `vec![1]` to `vec![2]` to trigger schema migration.

**Why this is critical:**
- The schema hash is used by the lakehouse to detect schema changes
- Without bumping the hash, existing partitions won't be rebuilt with the new format
- This ensures all cached/materialized data gets regenerated with JSONB format
- Prevents schema mismatch errors between old and new partitions

```rust
/// Returns the file schema hash for the blocks view.
pub fn blocks_file_schema_hash() -> Vec<u8> {
    vec![2]  // Bumped from vec![1] for JSONB migration
}
```

#### 2.3 Schema Version Management
**Important considerations:**
- Each view set maintains its own schema version via the hash
- When schema changes, ALL partitions for that view will be invalidated
- The system will automatically recreate partitions with the new schema
- This ensures consistency across all stored data

#### 2.4 No Database Changes Required
Since blocks table reads from existing `processes.properties` and `streams.properties` columns, no database schema changes are needed for this phase.

### Phase 3: Expanding to Other View Sets

#### 3.1 Schema Version Bumps for Each View
When expanding beyond blocks table, each view set needs:

**1. Schema Definition Update:**
```rust
// Example for processes view
fn properties_field_v2() -> Field {
    Field::new(
        "properties",
        DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Binary),
        ),
        true,
    )
}
```

**2. Schema Hash Increment:**
```rust
// Each view must bump its hash when schema changes
pub fn processes_file_schema_hash() -> Vec<u8> {
    vec![2]  // Increment from current value
}

pub fn streams_file_schema_hash() -> Vec<u8> {
    vec![2]  // Increment from current value
}

pub fn log_entries_file_schema_hash() -> Vec<u8> {
    vec![2]  // Increment from current value
}

pub fn metrics_file_schema_hash() -> Vec<u8> {
    vec![2]  // Increment from current value
}
```

**Critical: Without hash bumps, the lakehouse will not detect schema changes and existing partitions will cause conflicts!**

#### 3.2 SQL Queries Remain Unchanged
**Important**: Since we're transforming the data at the Arrow layer (in `PropertiesColumnReader`), the SQL queries remain exactly the same:
```sql
-- No changes needed to SQL queries!
SELECT properties FROM processes  -- Still reads micromegas_property[]
SELECT properties FROM streams    -- Still reads micromegas_property[]

-- The transformation to JSONB happens in PropertiesColumnReader
-- when converting from PostgreSQL to Arrow format
```

This is a key advantage of our approach - no SQL changes required!

### Phase 4: Arrow Builder Updates (For Non-SQL Sources)

**Important Discovery**: Log entries and metrics tables don't read from SQL like blocks table. They use `add_properties_to_builder()` from `arrow_properties.rs` to build Arrow arrays directly from in-memory data.

#### 4.1 Log Entries Table
Update `/rust/analytics/src/log_entries_table.rs`:
- Schema changes at lines 77-87 (properties) and 89-99 (process_properties)
- Builder code at line 191 uses `add_properties_to_builder()`
- Need new JSONB builder function in arrow_properties.rs

#### 4.2 Metrics Table
Update `/rust/analytics/src/metrics_table.rs`:
- Similar schema and builder changes as log_entries_table
- Uses same `add_properties_to_builder()` function

#### 4.3 Arrow Properties Utilities
Update `/rust/analytics/src/arrow_properties.rs`:
- Create new `add_properties_to_jsonb_builder()` function
- Use `BinaryDictionaryBuilder<Int32Type>` instead of ListBuilder
- Keep existing function for backward compatibility

### Phase 5: UDF Status (Mostly Ready)

#### 5.1 properties_to_jsonb UDF
**Already implemented** in `/rust/analytics/src/properties/properties_to_jsonb_udf.rs`:
- Supports conversion from List<Struct> to Dictionary<Int32, Binary>
- Has pass-through optimization for already-converted data
- Ready to use without modifications

#### 5.2 property_get UDF
**Already supports JSONB** in `/rust/analytics/src/properties/property_get.rs`:
- Has `extract_from_jsonb()` function (lines 57-73)
- Handles Dictionary<Int32, Binary> input (lines 171-220)
- No modifications needed

#### 5.3 Optional Migration Helpers
Consider adding for monitoring:
- `properties_dict_stats()`: Analyze compression ratios
- `validate_properties_jsonb()`: Data validation during migration

### Phase 6: Documentation Updates

#### 6.1 Code Documentation
Update rustdoc comments in modified files:
- `/rust/analytics/src/sql_arrow_bridge.rs` - Document JSONB conversion in PropertiesColumnReader
- `/rust/analytics/src/lakehouse/blocks_view.rs` - Update schema documentation
- `/rust/analytics/src/arrow_properties.rs` - Document new JSONB builder functions

#### 6.2 View Factory Documentation
Update `/rust/analytics/src/lakehouse/view_factory.rs`:
- Lines 117, 130, 153: Change from `Array of {key: utf8, value: utf8}` to `Dictionary-encoded JSONB`
- Add note about compression benefits

#### 6.3 User-Facing Documentation
Update MkDocs documentation:

**schema-reference.md** (`/mkdocs/docs/query-guide/schema-reference.md`):
- Line 40: Update `properties` type from `Map` to `Dictionary<Int32, Binary> (JSONB)`
- Line 70: Update stream properties type
- Lines 151-152: Update List<Struct> to Dictionary<Int32, Binary>
- Lines 265-266: Update metrics properties types
- Lines 447-457: Update "Common properties fields" section with new format

**functions-reference.md** (`/mkdocs/docs/query-guide/functions-reference.md`):
- Line 386: Add note that `property_get` supports both legacy and JSONB formats
- Line 490: Update `properties_to_jsonb` documentation to mention dictionary encoding
- Add performance comparison section

**quick-start.md** (`/mkdocs/docs/query-guide/quick-start.md`):
- Line 222: Add note about JSONB optimization

#### 6.4 Migration Guide
Create new documentation file: `/mkdocs/docs/migration/properties-jsonb.md`
- Explain the format change and benefits
- Show before/after storage comparisons
- Provide query examples that work unchanged
- Document any breaking changes (none expected)

### Phase 7: Testing Strategy

#### 7.1 Unit Tests
- Test each new column reader independently
- Verify JSONB serialization/deserialization
- Test dictionary compression efficiency
- Property UDF correctness with new format

#### 7.2 Integration Tests
- End-to-end data flow tests
- Performance benchmarks (storage + query time)
- Backward compatibility verification
- Migration data integrity checks

#### 7.3 Performance Testing
- Compare storage size: Array<Struct> vs Dictionary<JSONB>
- Query performance: complex property searches
- Ingestion performance: new format overhead
- Memory usage during processing

## Migration Timeline - Blocks Table First Approach

### Phase 1: Blocks Table JSONB Implementation
1. ✅ Research and planning (current task)
2. **Modify PropertiesColumnReader** to output dictionary-encoded JSONB
3. **Update blocks_view_schema()** to use JSONB types
4. **Test blocks table** with existing data - no database changes needed
5. **Validate query performance** and storage efficiency

### Phase 2: Property UDF Integration
1. **Test property_get UDF** with new JSONB format from blocks table
   - **Note**: `property_get` already supports Dictionary<Int32, Binary> format (verified in code)
   - Has existing `extract_from_jsonb` function for JSONB property extraction
2. **Verify properties_to_jsonb UDF** pass-through optimization works
3. **Integration testing** with real workloads
4. **Performance benchmarking** vs original format

### Phase 3: Expand to Other View Sets
1. Apply same pattern to **processes** and **streams** views
2. Update **log_entries** and **measures** tables
3. Add database schema migrations for native JSONB columns
4. Comprehensive testing across all view sets

### Phase 4: Production Rollout
1. Staged deployment starting with blocks table
2. Monitor performance and storage improvements
3. Gradual rollout to other view sets
4. Legacy format cleanup

## Risk Assessment

### High Risk
- **Data integrity**: Ensure no data loss during migration
- **Performance regression**: New format must not slow down queries
- **Backward compatibility**: Existing queries must continue working

### Medium Risk
- **Storage overhead**: Dictionary encoding effectiveness varies with data
- **Migration complexity**: Multiple systems need coordinated updates
- **Testing coverage**: Complex data types harder to test comprehensively

### Mitigation Strategies
- Dual-column approach allows gradual rollout and easy rollback
- Comprehensive testing at each phase
- Performance monitoring and optimization throughout
- Staged deployment with careful validation

## Success Metrics

### Storage Efficiency
- Target: 30-50% reduction in properties storage size
- Measurement: Compare parquet file sizes before/after

### Query Performance
- Target: Maintain or improve property search times
- Measurement: Benchmark common property query patterns

### Development Velocity
- Target: No impact on developer productivity
- Measurement: Time to implement new property-based features

## Conclusion

This migration to dictionary-encoded JSONB represents a significant architectural improvement for properties storage in Micromegas. **The blocks table first approach minimizes risk** by:

1. **No database changes required** for initial testing and validation
2. **Immediate feedback loop** - can test entire JSONB pipeline with existing data
3. **Gradual rollout capability** - validate approach before broader implementation
4. **Easy rollback** - simply revert PropertiesColumnReader changes if issues arise

**Key Advantages of Starting with Blocks Table:**
- **Pure read transformation** - converts existing `micromegas_property[]` data to JSONB on read
- **Independent testing** - blocks table is self-contained for validation
- **Real-world validation** - uses actual production data patterns
- **Performance baseline** - establishes compression and query performance metrics

The existing `properties_to_jsonb` UDF shows that this migration path has been anticipated in the codebase design. By starting with the blocks table, we can validate the entire approach with minimal risk before expanding to other view sets.
