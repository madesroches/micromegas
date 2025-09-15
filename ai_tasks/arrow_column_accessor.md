# Arrow Column Accessor for String Arrays

## Objective
Develop a unified column accessor for Apache Arrow that can handle:
1. Simple string arrays (StringArray)
2. Dictionary-encoded string columns (DictionaryArray with string values)

The goal is to replace all uses of `typed_column_by_name` and `typed_column` with string types to allow dictionary-encoding to be used transparently.

## Background
Arrow supports multiple representations for string data:
- **StringArray**: UTF-8 strings with 32-bit offsets
- **DictionaryArray**: Indices pointing to a dictionary of unique values (more memory efficient for repeated strings)

Dictionary encoding is particularly useful for columns with low cardinality (few unique values relative to total rows).

## Requirements

### Functional Requirements
1. Provide a unified interface to access string values regardless of underlying encoding
2. Support iteration over string values
3. Support indexed access to retrieve specific values
4. Handle null values appropriately
5. Preserve performance characteristics of dictionary encoding where possible

### Non-Functional Requirements
1. Minimal performance overhead for simple arrays
2. Avoid unnecessary string allocations when working with dictionary arrays
3. Type-safe interface using Rust's type system
4. Clear error handling for unsupported column types

## Design Approach

### Core Interface
Match StringArray's API to ensure drop-in compatibility:
```rust
trait StringColumnAccessor: Send {
    /// Returns the string value at index (matches StringArray::value())
    fn value(&self, index: usize) -> &str;
    
    /// Returns the length of the array (matches StringArray::len())
    fn len(&self) -> usize;
    
    /// Returns true if value at index is null (matches StringArray::is_null())
    fn is_null(&self, index: usize) -> bool;
    
    /// Returns true if the array is empty (matches StringArray::is_empty())
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// Factory function to create appropriate implementation
// Returns Send trait object for use in async contexts
fn create_string_accessor(array: &ArrayRef) -> Result<Box<dyn StringColumnAccessor + Send>>;
```

Note: Using the same method names as StringArray (value() instead of get()) to ensure it can be a drop-in replacement.

### Implementation Strategy
1. Define a trait for string column access behavior
2. Create separate implementations for each array type (StringArray, DictionaryArray)
3. Use dynamic dispatch (Box<dyn StringColumnAccessor>) for runtime polymorphism
4. Factory method to create appropriate implementation based on array type
5. Leverage Arrow's existing APIs for value access

### Key Considerations
- Dictionary arrays store indices + dictionary, need to resolve through lookup
- Simple arrays store values directly
- Both support null bitmaps that must be checked
- Focus on indexed access patterns rather than iteration
- Use trait objects for extensibility (Open-Closed Principle)
- New array types can be supported by adding new implementations without modifying existing code
- Trait must be Send for use in async contexts (can be moved between threads)
- Sync not required since we're not sharing references across threads

## Implementation Plan

### Phase 1: Core Structure ✅ COMPLETED
- ✅ Define StringColumnAccessor trait
- ✅ Create factory function for constructing accessors
- ✅ Set up module structure in analytics crate

### Phase 2: Simple String Arrays ✅ COMPLETED
- ✅ Implement accessor for StringArray
- ✅ Add null handling (via is_null() method)

### Phase 3: Dictionary Arrays ✅ COMPLETED
- ✅ Implement accessor for DictionaryArray<Int32, Utf8>
- ✅ Handle index resolution to dictionary values
- ✅ Support Int32 index type (Int8, Int16 deferred - not needed currently)

### Phase 4: Testing & Optimization
- ⏳ Unit tests for all array types
- ⏳ Property-based tests for consistency
- ⏳ Integration with existing analytics code

## Success Criteria
1. Seamless access to string values regardless of encoding
2. Performance overhead < 5% vs direct array access
3. All tests passing including edge cases
4. Successfully integrated into property query functions
5. All uses of `typed_column_by_name::<StringArray>` replaced with new accessor across all crates

## Location
Implementation: `rust/analytics/src/dfext/string_column_accessor.rs` ✅ CREATED
Tests will be in: `rust/analytics/tests/`

## Key Files to Update
Files that use typed_column_by_name or typed_column with StringArray will need updating to use the new accessor for transparent dictionary encoding support.

## Implementation Notes
- ✅ Created helper function `string_column_by_name` that returns `Box<dyn StringColumnAccessor + Send>`
- ✅ This will be a drop-in replacement for `typed_column_by_name::<StringArray>`
- ✅ The accessor automatically handles both StringArray and DictionaryArray<Int32, Utf8>

## Current Implementation Status

### Completed Components
1. **StringColumnAccessor trait** - Defines the unified interface with methods:
   - `value(&self, index: usize) -> &str`
   - `len(&self) -> usize`
   - `is_null(&self, index: usize) -> bool`
   - `is_empty(&self) -> bool`

2. **StringArrayAccessor** - Implementation for simple StringArray
   - Direct access to string values
   - Null handling via Arrow's built-in methods

3. **DictionaryStringAccessor** - Implementation for DictionaryArray<Int32Type>
   - Resolves indices to dictionary values
   - Handles null values correctly

4. **Factory Functions**:
   - `create_string_accessor(array: &ArrayRef)` - Creates appropriate accessor based on array type
   - `string_column_by_name(batch: &RecordBatch, name: &str)` - Helper for column access by name

### Next Steps
1. Write comprehensive unit tests
2. Integrate into existing codebase by replacing `typed_column_by_name::<StringArray>` calls
3. Performance benchmarking to ensure < 5% overhead
