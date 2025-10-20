# Task: Add Dictionary-Encoded Binary Support to jsonb_format_json

## Status: 🔄 IN PROGRESS

**Progress**: Phase 1 (Test Creation) Complete ✅ | Phase 2 (Implementation) Pending ⏳

## Problem Statement
The `jsonb_format_json` function currently only accepts `Binary` type columns and fails when provided with `Dictionary<Int32, Binary>` columns. This creates an inconsistency with other JSONB functions that support dictionary-encoded inputs and forces users to add manual casts, defeating the purpose of dictionary encoding.

### Current Behavior
```sql
-- Works: Direct binary JSONB
SELECT jsonb_format_json(properties) FROM processes;

-- Fails: Dictionary-encoded JSONB (requires explicit cast)
SELECT jsonb_format_json(arrow_cast(properties, 'Binary')) FROM processes;
```

### Expected Behavior
```sql
-- Should work without cast
SELECT jsonb_format_json(properties) FROM processes;
```

## Background

Dictionary encoding is widely used in the codebase for memory efficiency:
- `properties_to_jsonb` returns `Dictionary<Int32, Binary>`
- Property columns are often dictionary-encoded for compression
- Other JSONB functions (via `BinaryColumnAccessor`) support both formats

The `jsonb_format_json` function is the only JSONB function that doesn't support dictionary-encoded inputs, creating an API inconsistency.

## Current Implementation Analysis

### File: `rust/analytics/src/dfext/jsonb/format_json.rs`

**Current Issues:**
1. **Line 35**: UDF signature only declares `DataType::Binary` as accepted input
2. **Lines 18-21**: Direct downcast to `GenericBinaryArray<i32>` without type checking
3. **No dictionary support**: Function doesn't handle `Dictionary<Int32, Binary>` type

```rust
fn jsonb_format_json(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
    // ...
    let jsonb_array: &GenericBinaryArray<i32> =
        src_arrays[0].as_any().downcast_ref::<_>().ok_or_else(|| {
            DataFusionError::Execution("error casting jsonb as GenericBinaryArray".into())
        })?;
    // ... processes only GenericBinaryArray
}

pub fn make_jsonb_format_json_udf() -> ScalarUDF {
    create_udf(
        "jsonb_format_json",
        vec![DataType::Binary],  // Only accepts Binary, not Dictionary
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(&jsonb_format_json),
    )
}
```

## Design Approach

### Solution: Use BinaryColumnAccessor Pattern

The codebase already has a well-established pattern for handling both `Binary` and `Dictionary<Int32, Binary>` through the `BinaryColumnAccessor` trait (see `rust/analytics/src/dfext/binary_column_accessor.rs`).

### Reference Implementation

Other code already uses this pattern successfully:
- `PropertiesColumnAccessor` (rust/analytics/src/properties/properties_column_accessor.rs:6)
- Uses `create_binary_accessor` to handle both formats transparently

### Implementation Strategy

1. **Import BinaryColumnAccessor utilities**
   ```rust
   use crate::dfext::binary_column_accessor::create_binary_accessor;
   ```

2. **Update function signature to accept both types**
   ```rust
   // Option 1: Accept multiple signatures (simpler, more explicit)
   vec![DataType::Binary]  // Keep existing
   // Add second signature variant for Dictionary

   // Option 2: Make signature flexible to accept both (requires UDF framework support)
   ```

3. **Replace direct downcast with create_binary_accessor**
   ```rust
   fn jsonb_format_json(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
       if values.len() != 1 {
           return Err(DataFusionError::Execution(
               "wrong number of arguments to jsonb_format_json".into(),
           ));
       }
       let src_arrays = ColumnarValue::values_to_arrays(values)?;

       // Use create_binary_accessor to handle both Binary and Dictionary<Int32, Binary>
       let binary_accessor = create_binary_accessor(&src_arrays[0])
           .map_err(|e| DataFusionError::Execution(format!("Invalid input type: {}", e)))?;

       let mut builder = StringBuilder::with_capacity(binary_accessor.len(), 1024);
       for index in 0..binary_accessor.len() {
           if binary_accessor.is_null(index) {
               builder.append_null();
           } else {
               let src_buffer = binary_accessor.value(index);
               let jsonb = RawJsonb::new(src_buffer);
               builder.append_value(jsonb.to_string());
           }
       }
       Ok(ColumnarValue::Array(Arc::new(builder.finish())))
   }
   ```

4. **Update UDF signature** (Investigation needed)
   - Research how other UDFs declare support for both Binary and Dictionary types
   - DataFusion may need multiple UDF registrations or a flexible signature approach
   - Check if signature validation can be relaxed to accept Dictionary<_, Binary>

### Key Benefits of This Approach

1. **Consistency**: Matches pattern used throughout the codebase
2. **No code duplication**: Reuses existing `BinaryColumnAccessor` infrastructure
3. **Null handling**: Accessor already handles nulls correctly
4. **Performance**: Dictionary encoding remains efficient (no unnecessary materialization)
5. **Future-proof**: Automatically supports any new binary column formats added to `create_binary_accessor`

## Implementation Plan

### Phase 1: Test Creation ✅ COMPLETED

**Objective**: Create comprehensive tests demonstrating the bug and defining expected behavior.

**File Created**: `rust/analytics/tests/jsonb_format_json_tests.rs`

**Completed Tasks**:
- [x] Created test file with helper functions for creating JSONB data
- [x] Implemented `test_jsonb_format_json_with_binary` - Tests current working Binary format
- [x] Implemented `test_jsonb_format_json_with_dictionary` - Tests Dictionary<Int32, Binary> (FAILING)
- [x] Implemented `test_jsonb_format_json_with_dictionary_and_nulls` - Tests null handling with Dictionary (FAILING)
- [x] Implemented `test_jsonb_format_json_empty_object` - Tests edge case with empty JSONB
- [x] All tests compile and run successfully
- [x] Confirmed bug with clear error messages

**Test Results**:
```
running 4 tests
test test_jsonb_format_json_with_binary ... ok
test test_jsonb_format_json_empty_object ... ok
test test_jsonb_format_json_with_dictionary ... FAILED
test test_jsonb_format_json_with_dictionary_and_nulls ... FAILED

Error: No function matches the given name and argument types
'jsonb_format_json(Dictionary(Int32, Binary))'.
You might need to add explicit type casts.
Candidate functions:
  jsonb_format_json(Binary)
```

**Key Findings**:
1. Binary format works correctly ✅
2. Dictionary format is rejected at SQL planning stage ❌
3. Error message confirms the function signature only accepts `Binary`
4. Need to update both function implementation AND UDF signature
5. Tests use `serialize_properties_to_jsonb` from existing codebase for JSONB creation

**Helper Functions Implemented**:
- `create_jsonb_bytes()` - Creates JSONB bytes from HashMap
- `create_jsonb_binary_array()` - Creates Binary array from JSONB data
- `create_jsonb_dictionary_array()` - Creates Dictionary<Int32, Binary> array with key mapping
- `create_record_batch()` - Creates RecordBatch for SQL testing
- `execute_jsonb_format_json()` - Executes function via DataFusion SQL

### Phase 2: Core Implementation ⏳ PENDING
- [ ] Add `create_binary_accessor` import to `format_json.rs`
- [ ] Replace direct `GenericBinaryArray` downcast with `create_binary_accessor` call
- [ ] Add explicit null handling in the loop
- [ ] Handle errors from `create_binary_accessor` with clear error messages
- [ ] Run tests to verify implementation works

### Phase 3: UDF Signature Update ⏳ PENDING
- [ ] Research DataFusion UDF signature patterns for accepting multiple types
- [ ] Update `make_jsonb_format_json_udf` to accept Dictionary<Int32, Binary>
- [ ] Options to investigate:
  - Multiple UDF registrations with different signatures
  - Signature wildcards/matching rules
  - Runtime type checking without strict signature
- [ ] Verify all tests pass

### Phase 4: Documentation ⏳ PENDING
- [ ] Update function documentation in code
- [ ] Update query guide if `jsonb_format_json` is documented there
- [ ] Add example showing dictionary-encoded usage
- [ ] Note compatibility with `properties_to_jsonb` output

## Testing Strategy ✅ COMPLETED

### Unit Tests
**File**: `rust/analytics/tests/jsonb_format_json_tests.rs`

**Implemented Tests**:

1. ✅ `test_jsonb_format_json_with_binary()` - **PASSING**
   - Tests direct Binary input (existing behavior)
   - Verifies correct JSON string output
   - Validates multiple JSONB objects in single batch

2. ✅ `test_jsonb_format_json_with_dictionary()` - **FAILING (Expected)**
   - Tests Dictionary<Int32, Binary> input
   - Uses dictionary keys [0, 1, 0, 1] to demonstrate deduplication
   - Verifies same dictionary key produces same output
   - **Error**: Function signature doesn't accept Dictionary type

3. ✅ `test_jsonb_format_json_with_dictionary_and_nulls()` - **FAILING (Expected)**
   - Tests null handling with Dictionary<Int32, Binary>
   - Uses pattern [Some(0), None, Some(0)]
   - Verifies nulls are preserved in output
   - **Error**: Function signature doesn't accept Dictionary type

4. ✅ `test_jsonb_format_json_empty_object()` - **PASSING**
   - Tests edge case with empty JSONB object
   - Verifies output is "{}"

**Test Coverage**:
- ✅ Binary format (backward compatibility)
- ✅ Dictionary format (main feature)
- ✅ Null handling in both formats
- ✅ Empty objects
- ✅ Repeated values (dictionary efficiency)

### Integration Tests (Future)
Once implementation is complete, test in production scenarios:

```sql
-- Test 1: Direct usage with dictionary-encoded properties
SELECT jsonb_format_json(properties) FROM processes LIMIT 10;

-- Test 2: Pipeline with properties_to_jsonb (returns Dictionary)
SELECT jsonb_format_json(properties_to_jsonb(properties))
FROM processes LIMIT 10;

-- Test 3: Verify output is valid JSON
SELECT
    jsonb_format_json(properties) as json_str,
    length(jsonb_format_json(properties)) as json_len
FROM processes
WHERE properties IS NOT NULL
LIMIT 10;
```

## Expected Benefits

### 1. API Consistency
- All JSONB functions accept both Binary and Dictionary<Int32, Binary>
- Eliminates special case handling for `jsonb_format_json`

### 2. User Experience
- No manual casts required
- Works seamlessly with `properties_to_jsonb` output
- Cleaner SQL queries

### 3. Performance
- Maintains dictionary encoding benefits
- No unnecessary materialization of dictionary values
- Memory-efficient processing

### 4. Maintainability
- Reuses existing `BinaryColumnAccessor` infrastructure
- Consistent error handling
- Single code path for all binary column types

## Dependencies

- `rust/analytics/src/dfext/binary_column_accessor.rs` - Core accessor infrastructure
- DataFusion UDF framework - For signature handling
- `jsonb` crate - For JSONB parsing (already used)

## Research Questions

1. **UDF Signatures**: How do other DataFusion UDFs handle accepting both a base type and its dictionary-encoded variant?
2. **Type Validation**: Is signature validation strict in DataFusion, or can we use runtime type checking?
3. **Performance**: Does using `BinaryColumnAccessor` introduce any performance overhead vs direct array access?

## Success Criteria

1. ✅ Function accepts both `Binary` and `Dictionary<Int32, Binary>` inputs
2. ✅ No manual casts required in user queries
3. ✅ All existing tests continue to pass
4. ✅ New tests cover dictionary-encoded scenarios
5. ✅ Documentation updated to reflect both supported types
6. ✅ No performance regression measured in benchmarks

## Implementation Notes

- Follow existing patterns from `properties_column_accessor.rs`
- Ensure error messages are clear when invalid types are provided
- Consider adding a comment explaining why we support both formats
- May need to update DataFusion UDF registration approach

## Related Tasks

- **Completed**: `jsonb_support_all_properties_functions.md` - Established pattern for JSONB function consistency
- **Completed**: `property_get_dict_return.md` - Dictionary encoding benefits and patterns
- **Reference**: `dictionary_encoding_for_properties.md` - Original dictionary encoding implementation

## Notes

This is a small but important fix that eliminates an API inconsistency. The solution is straightforward using existing infrastructure (`BinaryColumnAccessor`), with the main unknown being the DataFusion UDF signature handling.

## Progress Log

### 2025-10-20: Phase 1 Complete - Test Creation

**Created comprehensive test suite** (`rust/analytics/tests/jsonb_format_json_tests.rs`):
- 4 tests total: 2 passing (Binary format), 2 failing (Dictionary format)
- Tests compile and run correctly
- Clear error messages confirm the bug: function signature rejects Dictionary<Int32, Binary>
- Used existing `serialize_properties_to_jsonb` helper from codebase
- Tests execute via DataFusion SQL for realistic scenario testing

**Key Insights**:
1. The issue is both in function implementation AND UDF signature registration
2. DataFusion validates types at SQL planning stage before calling the function
3. Tests demonstrate dictionary encoding benefits (repeated keys)
4. Need to support null handling properly in dictionary format
5. `BinaryColumnAccessor` pattern is the right solution (already proven in other code)

**Next Steps**:
- Phase 2: Implement core function changes using `BinaryColumnAccessor`
- Phase 3: Update UDF signature to accept Dictionary<Int32, Binary>
- Verify all tests pass after implementation
