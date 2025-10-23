# Building Dictionary Arrays with Nested Types in Arrow/DataFusion

## Overview

Apache Arrow's Rust implementation provides dictionary builders for primitive types and strings, but not for complex nested types like dictionaries of lists of structs. This document explains how to build such structures manually.

## Available Dictionary Builders

Arrow provides these built-in dictionary builders for simple types:

- `StringDictionaryBuilder<K>` - For string dictionaries
- `PrimitiveDictionaryBuilder<K, V>` - For primitive type dictionaries
- `BinaryDictionaryBuilder<K>` - For binary data dictionaries
- `LargeStringDictionaryBuilder<K>` - For large string dictionaries

These builders handle dictionary encoding automatically but only work with their respective simple types.

## Manual Approach for Dictionary<List<Struct>>

For a dictionary containing lists of key-value structs, you need to build it manually:

```rust
use datafusion::arrow::{
    array::{
        ArrayRef, DictionaryArray, ListArray, StructArray,
        Int32Array, StringBuilder, ListBuilder, StructBuilder
    },
    datatypes::{DataType, Field, Fields, Int32Type},
};
use std::sync::Arc;
use std::collections::HashMap;

// Define the struct schema for key-value pairs
let key_value_fields = Fields::from(vec![
    Field::new("key", DataType::Utf8, false),
    Field::new("value", DataType::Utf8, false),
]);

// Create the list builder for list<struct>
let mut list_builder = ListBuilder::new(
    StructBuilder::from_fields(key_value_fields.clone(), 100)
);

// Track unique lists and their indices
let mut unique_lists = HashMap::new();
let mut dict_keys = Vec::new();

// Example data
let data = vec![
    vec![("name", "Alice"), ("age", "30")],
    vec![("name", "Bob"), ("age", "25")],
    vec![("name", "Alice"), ("age", "30")], // Duplicate
];

// Build unique lists and track dictionary keys
for list_data in data {
    let list_key = format!("{:?}", list_data); // Simple hash key
    
    let dict_index = unique_lists.entry(list_key).or_insert_with(|| {
        // Build this unique list
        let struct_builder = list_builder.values();
        for (k, v) in &list_data {
            struct_builder.field_builder::<StringBuilder>(0).unwrap().append_value(k);
            struct_builder.field_builder::<StringBuilder>(1).unwrap().append_value(v);
            struct_builder.append(true);
        }
        list_builder.append(true);
        
        unique_lists.len() as i32 - 1
    });
    
    dict_keys.push(*dict_index);
}

// Create the dictionary array
let keys = Int32Array::from(dict_keys);
let values = Arc::new(list_builder.finish());
let dictionary = DictionaryArray::<Int32Type>::try_new(keys, values)?;

// Convert to ArrayRef for use in DataFusion
let column: ArrayRef = Arc::new(dictionary);
```

## Custom Builder Pattern

For repeated use, you can create a custom builder:

```rust
struct ListStructDictionaryBuilder {
    values: Vec<Vec<(String, String)>>,
    value_to_key: HashMap<Vec<(String, String)>, i32>,
    keys: Vec<i32>,
}

impl ListStructDictionaryBuilder {
    fn new() -> Self {
        Self {
            values: Vec::new(),
            value_to_key: HashMap::new(),
            keys: Vec::new(),
        }
    }
    
    fn append(&mut self, value: Vec<(String, String)>) {
        let key = self.value_to_key.entry(value.clone())
            .or_insert_with(|| {
                let new_key = self.values.len() as i32;
                self.values.push(value);
                new_key
            });
        self.keys.push(*key);
    }
    
    fn finish(self) -> Result<DictionaryArray<Int32Type>> {
        // Build the list array from unique values
        let key_value_fields = Fields::from(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
        ]);
        
        let mut list_builder = ListBuilder::new(
            StructBuilder::from_fields(key_value_fields, self.values.len())
        );
        
        for list_data in &self.values {
            let struct_builder = list_builder.values();
            for (k, v) in list_data {
                struct_builder.field_builder::<StringBuilder>(0)?.append_value(k);
                struct_builder.field_builder::<StringBuilder>(1)?.append_value(v);
                struct_builder.append(true);
            }
            list_builder.append(true);
        }
        
        let keys = Int32Array::from(self.keys);
        let values = Arc::new(list_builder.finish());
        DictionaryArray::<Int32Type>::try_new(keys, values)
    }
}
```

## Key Points

1. **No built-in support**: Arrow doesn't provide dictionary builders for complex nested types
2. **Manual encoding**: You must track unique values and build the dictionary manually
3. **Memory efficiency**: Dictionary encoding is beneficial when you have repeated complex values
4. **Type safety**: The resulting `DictionaryArray` maintains full type information

## References

- [Arrow DictionaryArray Documentation](https://docs.rs/arrow/latest/arrow/array/struct.DictionaryArray.html)
- [Arrow Builder Module](https://docs.rs/arrow/latest/arrow/array/builder/index.html)
- Micromegas examples: `rust/analytics/src/log_entries_table.rs` shows `StringDictionaryBuilder` usage