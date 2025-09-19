# Task: Add JSONB Support to All Properties Functions

## Objective
Extend all properties-related UDFs to support JSONB formats (Binary and Dictionary<Int32, Binary>) for consistency and optimal performance across the properties ecosystem.

## Background
Currently, only `property_get` has been updated to support JSONB formats. The other properties functions (`properties_length`, `properties_to_dict`, `properties_to_array`, `properties_to_jsonb`) still only work with List<Struct> formats, creating an inconsistent API and limiting the benefits of JSONB property storage.

## Current State Analysis

### Functions with JSONB Support:
1. ✅ **property_get** - Full JSONB support (Dictionary<Int32, Binary>, Binary)

### Functions Needing Updates:
1. ❌ **properties_length** - Only supports List<Struct> and Dictionary<Int32, List<Struct>>
2. ❌ **properties_to_jsonb** - Currently returns Binary, should return Dictionary<Int32, Binary>

### Functions to Deprecate:
1. 🗑️ **properties_to_dict** - Creates Dictionary<Int32, List<Struct>> which goes against JSONB migration direction
2. 🗑️ **properties_to_array** - Converts back to legacy List<Struct> format which goes against JSONB migration direction

## Design Approach

### Key Design Decisions:
1. **Consistent API**: Core properties functions should accept the same input formats:
   - `List<Struct<key, value>>` (legacy format)
   - `Dictionary<Int32, List<Struct>>` (dictionary-encoded legacy)
   - `Binary` (JSONB format)
   - `Dictionary<Int32, Binary>` (dictionary-encoded JSONB - primary format)

2. **Deprecation Path**: Remove `properties_to_dict` as it creates the wrong format for our JSONB migration

3. **Performance**: JSONB operations should be optimized for performance
4. **Backward Compatibility**: All existing usage patterns must continue to work
5. **Type Safety**: Functions should handle type validation gracefully

### Implementation Strategy:

#### 1. **properties_length(properties)**
Add support for:
- `Binary`: Parse JSONB and count object keys
- `Dictionary<Int32, Binary>`: Handle dictionary-encoded JSONB

**Implementation:**
```rust
fn count_jsonb_properties(jsonb_bytes: &[u8]) -> anyhow::Result<i32> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    // Count object keys
    jsonb.object_len().map(|len| len as i32)
}
```

#### 2. **properties_to_jsonb(properties)**
Update return type from `Binary` to `Dictionary<Int32, Binary>` for optimal storage

**Strategy:**
- Convert List<Struct> properties to JSONB format
- Apply dictionary encoding to compress repeated property sets
- Return Dictionary<Int32, Binary> for consistent API with other functions



## Implementation Plan

### Phase 1: Update properties_length
- [ ] Add JSONB type detection in invoke_with_args()
- [ ] Implement Binary JSONB property counting
- [ ] Implement Dictionary<Int32, Binary> property counting
- [ ] Add comprehensive tests

### Phase 2: Update properties_to_jsonb
- [ ] Change return type from Binary to Dictionary<Int32, Binary>
- [ ] Implement dictionary encoding for JSONB output
- [ ] Update function signature and return type logic
- [ ] Add comprehensive tests

### Phase 3: Documentation Updates
- [ ] Update all function documentation
- [ ] Update query examples in mkdocs only where current examples are no longer valid

## Testing Strategy

### Unit Tests for Each Function:
1. **properties_length**:
   - Test Binary JSONB property counting
   - Test Dictionary<Int32, Binary> property counting
   - Test mixed null/non-null JSONB values
   - Test backward compatibility

2. **properties_to_jsonb**:
   - Test conversion to Dictionary<Int32, Binary>
   - Test dictionary encoding compression
   - Test backward compatibility with Binary return type expectations

### Integration Tests:
1. Cross-function compatibility (property_get + properties_length)
2. Performance comparison: JSONB vs List<Struct> workflows
3. Real-world query patterns with mixed property formats

## Expected Benefits

### Performance Improvements:
- **properties_length**: Direct JSONB key counting vs struct iteration
- **properties_to_jsonb**: Passthrough optimization for JSONB inputs
- **Consistent API**: No need for format conversions between functions

### Developer Experience:
- **Uniform API**: All properties functions work with all property formats
- **Future-proof**: Ready for JSONB-primary storage migration
- **Simplified Queries**: No need to track property format in query logic

## Migration Path

1. **Phase 1**: Extend functions to support JSONB (this task)
2. **Phase 2**: Update documentation and examples
3. **Phase 3**: Gradual adoption of JSONB formats in production
4. **Phase 4**: Consider deprecating List<Struct> formats

## Success Criteria

1. ✅ All properties functions accept Binary and Dictionary<Int32, Binary> inputs
2. ✅ Performance is equal or better for JSONB operations
3. ✅ 100% backward compatibility maintained
4. ✅ Comprehensive test coverage for all new functionality
5. ✅ Updated documentation with JSONB examples
6. ✅ Benchmarks demonstrate performance benefits

## Implementation Notes

- Use shared utility functions to avoid code duplication
- Follow the same patterns established in property_get implementation
- Maintain consistent error handling and NULL semantics
- Consider memory efficiency for large property sets
- Ensure proper JSONB validation and error messages

## Dependencies

- Existing `jsonb` crate functionality
- `property_get` implementation patterns (reference)
- Arrow array manipulation utilities
- DataFusion UDF framework

This task builds upon the successful property_get JSONB implementation to create a fully consistent and high-performance properties function ecosystem.