# Plan: properties_to_dict UDF Implementation

## Overview
Implement a DataFusion UDF that converts properties (list of key-value struct pairs) into dictionary-encoded arrays for memory efficiency.

## Input/Output
- **Input**: List<Struct<key: Utf8, value: Utf8>> - A list array containing key-value pairs
- **Output**: Dictionary<Int32, List<Struct<key: Utf8, value: Utf8>>> - Dictionary-encoded version

## Implementation Steps

### 1. Research Current Properties Structure
- [ ] Examine existing properties columns in log_entries_table.rs
- [ ] Understand current data format and usage patterns
- [ ] Check if properties are already using any dictionary encoding

### 2. Create UDF Module
- [ ] Add new module `properties_dict_udf.rs` in `rust/analytics/src/`
- [ ] Define the UDF signature and return type
- [ ] Register with DataFusion's function registry

### 3. Implement Dictionary Builder
- [ ] Create custom builder similar to `ListStructDictionaryBuilder` from the reference
- [ ] Track unique property lists using HashMap
- [ ] Handle null values appropriately
- [ ] Optimize hash computation for property lists

### 4. Core UDF Logic
- [ ] Process input ArrayRef (ListArray of StructArrays)
- [ ] Extract property lists and deduplicate
- [ ] Build dictionary keys array (Int32Array)
- [ ] Build dictionary values array (unique property lists)
- [ ] Return DictionaryArray<Int32Type>

### 5. Memory Optimization
- [ ] Pre-allocate builders with estimated capacity
- [ ] Use efficient hashing for property list comparison
- [ ] Consider using binary encoding for faster comparison

### 6. Testing
- [ ] Unit tests with various property list patterns
- [ ] Test with duplicate property lists (verify dictionary encoding works)
- [ ] Test with empty lists and null values
- [ ] Benchmark memory usage reduction

### 7. Integration
- [ ] Register UDF in analytics initialization
- [ ] Update SQL queries to use properties_to_dict where beneficial
- [ ] Document usage in schema reference

## StringDictionaryBuilder Implementation Analysis

After analyzing Arrow's StringDictionaryBuilder, here's how it works internally and how we can apply its strategy:

### Three-Component Architecture
1. **HashMap for deduplication**: `HashMap<Box<[u8]>, usize>` tracks unique strings → dictionary indices
2. **Keys builder**: Builds array of dictionary indices (references into the dictionary)
3. **Values builder**: Stores unique string values only once

### Core Algorithm
```
append(value) → 
  1. Check HashMap for existing value
  2. If exists: reuse index, append to keys_builder
  3. If new: 
     - Add to values_builder
     - Insert (value → new_index) in HashMap
     - Append new_index to keys_builder
```

### Key Insights for properties_to_dict
The brilliant part is the **separation of concerns**:
- **Deduplication logic** is entirely in the HashMap
- **Storage** is split between keys (many small integers) and values (unique items only)
- **Memory efficiency** comes from storing each unique value once, with repetitions only costing an int16/int32
- The strategy is essentially **memoization at the columnar level** - cache unique values and reference them by index

### Applied to Our Use Case
For `properties_to_dict` handling `List<Struct<key,value>>`:

1. **First version - Keep it simple**:
   - Use standard `HashMap<Vec<(String, String)>, usize>` for deduplication
   - Let Rust's HashMap handle the hashing automatically (uses hashbrown internally since Rust 1.36)
   - Focus on correctness over optimization

2. **Values storage**: Instead of `StringBuilder`, we need `ListBuilder<StructBuilder>`
   - Build unique property lists once
   - Reference them by index

3. **Future optimizations** (after v1 works):
   - Custom hash function for property lists
   - Use hashbrown's RawTable for more control
   - Consider interning strings if there's high key/value repetition

### Performance Considerations
- StringDictionaryBuilder trades CPU (hash lookups) for memory (deduplication)
- For properties with high repetition (>30%), dictionary encoding wins
- The HashMap lookup cost is amortized across the memory savings

## Code Structure

```rust
// rust/analytics/src/properties_dict_udf.rs

use datafusion::prelude::*;
use datafusion::arrow::{
    array::*,
    datatypes::*,
};
use std::collections::HashMap;

pub struct PropertiesToDict;

impl PropertiesToDict {
    pub fn new() -> Self { ... }
    
    fn call(&self, args: &[ArrayRef]) -> Result<ArrayRef> {
        // 1. Validate input is List<Struct>
        // 2. Build dictionary using custom builder
        // 3. Return dictionary array
    }
}

// Simple v1 implementation - focus on correctness
struct PropertiesDictionaryBuilder {
    // 1. HashMap for deduplication
    map: HashMap<Vec<(String, String)>, usize>,
    
    // 2. Storage for unique property lists
    values_builder: ListBuilder<StructBuilder>,
    
    // 3. Keys tracking which dictionary entry each row uses
    keys: Vec<Option<i32>>,
}

impl PropertiesDictionaryBuilder {
    fn append_properties(&mut self, properties: Vec<(String, String)>) -> Result<()> {
        // Simple lookup - let HashMap handle the hashing
        match self.map.get(&properties) {
            Some(&index) => {
                // Reuse existing dictionary entry
                self.keys.push(Some(index as i32));
            }
            None => {
                // Add new unique property list to dictionary
                let new_index = self.map.len();
                self.add_to_values(&properties)?;
                self.map.insert(properties, new_index);
                self.keys.push(Some(new_index as i32));
            }
        }
        Ok(())
    }
}
```

## Expected Benefits
- **Memory Reduction**: 50-80% for datasets with repeated property patterns
- **Query Performance**: Faster equality comparisons on dictionary keys
- **Cache Efficiency**: Better CPU cache utilization with smaller memory footprint

## Risks & Considerations
- Dictionary building overhead for one-time queries
- May not benefit datasets with highly unique property lists
- Need to handle schema evolution gracefully

## Success Criteria
- [ ] UDF successfully converts properties to dictionary encoding
- [ ] Memory usage reduced by at least 40% in typical workloads
- [ ] No performance regression in query execution
- [ ] All existing tests pass with new UDF