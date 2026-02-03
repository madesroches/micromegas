use anyhow::Result;
use datafusion::arrow::array::{ArrayRef, DictionaryArray, Int32Array, RecordBatch, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Int32Type, Schema};
use micromegas_analytics::dfext::string_column_accessor::{
    create_string_accessor, string_column_by_name,
};
use std::sync::Arc;

#[test]
fn test_string_array_accessor() -> Result<()> {
    // Create a simple string array
    let array = StringArray::from(vec![Some("hello"), None, Some("world"), Some("test")]);
    let array_ref: ArrayRef = Arc::new(array);

    let accessor = create_string_accessor(&array_ref)?;

    // Test len
    assert_eq!(accessor.len(), 4);
    assert!(!accessor.is_empty());

    // Test value access
    assert_eq!(accessor.value(0)?, "hello");
    assert_eq!(accessor.value(2)?, "world");
    assert_eq!(accessor.value(3)?, "test");

    // Test null handling
    assert!(!accessor.is_null(0));
    assert!(accessor.is_null(1));
    assert!(!accessor.is_null(2));
    assert!(!accessor.is_null(3));

    Ok(())
}

#[test]
fn test_dictionary_array_accessor() -> Result<()> {
    // Create dictionary values (unique strings)
    let values = StringArray::from(vec!["apple", "banana", "cherry", "date"]);

    // Create indices pointing to the dictionary
    let indices = Int32Array::from(vec![Some(0), Some(1), None, Some(2), Some(0), Some(3)]);

    // Create dictionary array
    let dict_array = DictionaryArray::<Int32Type>::try_new(indices, Arc::new(values))?;
    let array_ref: ArrayRef = Arc::new(dict_array);

    let accessor = create_string_accessor(&array_ref)?;

    // Test len
    assert_eq!(accessor.len(), 6);
    assert!(!accessor.is_empty());

    // Test value access - should resolve through dictionary
    assert_eq!(accessor.value(0)?, "apple");
    assert_eq!(accessor.value(1)?, "banana");
    assert_eq!(accessor.value(3)?, "cherry");
    assert_eq!(accessor.value(4)?, "apple"); // Same as index 0
    assert_eq!(accessor.value(5)?, "date");

    // Test null handling
    assert!(!accessor.is_null(0));
    assert!(!accessor.is_null(1));
    assert!(accessor.is_null(2)); // Null index
    assert!(!accessor.is_null(3));
    assert!(!accessor.is_null(4));
    assert!(!accessor.is_null(5));

    Ok(())
}

#[test]
fn test_empty_arrays() -> Result<()> {
    // Test empty string array
    let empty_string = StringArray::from(Vec::<Option<&str>>::new());
    let array_ref: ArrayRef = Arc::new(empty_string);
    let accessor = create_string_accessor(&array_ref)?;
    assert_eq!(accessor.len(), 0);
    assert!(accessor.is_empty());

    // Test empty dictionary array
    let values = StringArray::from(vec!["unused"]);
    let indices = Int32Array::from(Vec::<Option<i32>>::new());
    let dict_array = DictionaryArray::<Int32Type>::try_new(indices, Arc::new(values))?;
    let array_ref: ArrayRef = Arc::new(dict_array);
    let accessor = create_string_accessor(&array_ref)?;
    assert_eq!(accessor.len(), 0);
    assert!(accessor.is_empty());

    Ok(())
}

#[test]
fn test_all_nulls() -> Result<()> {
    // String array with all nulls
    let all_nulls = StringArray::from(vec![None::<&str>, None, None]);
    let array_ref: ArrayRef = Arc::new(all_nulls);
    let accessor = create_string_accessor(&array_ref)?;
    assert_eq!(accessor.len(), 3);
    assert!(accessor.is_null(0));
    assert!(accessor.is_null(1));
    assert!(accessor.is_null(2));

    // Dictionary array with all null indices
    let values = StringArray::from(vec!["value"]);
    let indices = Int32Array::from(vec![None, None]);
    let dict_array = DictionaryArray::<Int32Type>::try_new(indices, Arc::new(values))?;
    let array_ref: ArrayRef = Arc::new(dict_array);
    let accessor = create_string_accessor(&array_ref)?;
    assert_eq!(accessor.len(), 2);
    assert!(accessor.is_null(0));
    assert!(accessor.is_null(1));

    Ok(())
}

#[test]
fn test_string_column_by_name() -> Result<()> {
    // Create a record batch with both string and dictionary columns
    let schema = Schema::new(vec![
        Field::new("string_col", DataType::Utf8, true),
        Field::new(
            "dict_col",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        ),
    ]);

    let string_array = StringArray::from(vec![Some("a"), Some("b"), None, Some("c")]);

    let dict_values = StringArray::from(vec!["x", "y", "z"]);
    let dict_indices = Int32Array::from(vec![Some(0), Some(1), Some(2), None]);
    let dict_array = DictionaryArray::<Int32Type>::try_new(dict_indices, Arc::new(dict_values))?;

    let batch = RecordBatch::try_new(
        Arc::new(schema),
        vec![Arc::new(string_array), Arc::new(dict_array)],
    )?;

    // Test accessing string column
    let string_accessor = string_column_by_name(&batch, "string_col")?;
    assert_eq!(string_accessor.len(), 4);
    assert_eq!(string_accessor.value(0)?, "a");
    assert_eq!(string_accessor.value(1)?, "b");
    assert!(string_accessor.is_null(2));
    assert_eq!(string_accessor.value(3)?, "c");

    // Test accessing dictionary column
    let dict_accessor = string_column_by_name(&batch, "dict_col")?;
    assert_eq!(dict_accessor.len(), 4);
    assert_eq!(dict_accessor.value(0)?, "x");
    assert_eq!(dict_accessor.value(1)?, "y");
    assert_eq!(dict_accessor.value(2)?, "z");
    assert!(dict_accessor.is_null(3));

    // Test missing column
    let result = string_column_by_name(&batch, "missing");
    assert!(result.is_err());
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("Column 'missing' not found")
    );

    Ok(())
}

#[test]
fn test_unsupported_types() -> Result<()> {
    // Try with Int32 array (not a string type)
    let int_array = Int32Array::from(vec![1, 2, 3]);
    let array_ref: ArrayRef = Arc::new(int_array);
    let result = create_string_accessor(&array_ref);
    assert!(result.is_err());
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("Unsupported array type")
    );

    Ok(())
}

#[test]
fn test_large_dictionary() -> Result<()> {
    // Test with a larger dictionary to ensure performance
    let num_unique = 100;
    let num_entries = 10000;

    // Create dictionary values
    let values: Vec<String> = (0..num_unique).map(|i| format!("value_{}", i)).collect();
    let values_array = StringArray::from(values.clone());

    // Create indices with repetition
    let indices: Vec<Option<i32>> = (0..num_entries)
        .map(|i| Some((i % num_unique) as i32))
        .collect();
    let indices_array = Int32Array::from(indices);

    let dict_array = DictionaryArray::<Int32Type>::try_new(indices_array, Arc::new(values_array))?;
    let array_ref: ArrayRef = Arc::new(dict_array);

    let accessor = create_string_accessor(&array_ref)?;

    // Verify length
    assert_eq!(accessor.len(), num_entries);

    // Spot check some values
    assert_eq!(accessor.value(0)?, "value_0");
    assert_eq!(accessor.value(100)?, "value_0");
    assert_eq!(accessor.value(101)?, "value_1");
    assert_eq!(accessor.value(199)?, "value_99");

    // Check that accessor is Send (required for async contexts)
    fn assert_send<T: Send>(_: &T) {}
    assert_send(&accessor);

    Ok(())
}

#[test]
fn test_unicode_strings() -> Result<()> {
    // Test with unicode strings
    let unicode_array = StringArray::from(vec![
        Some("Hello 世界"),
        Some("Здравствуй мир"),
        Some("مرحبا بالعالم"),
        Some("🌍🌎🌏"),
    ]);
    let array_ref: ArrayRef = Arc::new(unicode_array);

    let accessor = create_string_accessor(&array_ref)?;

    assert_eq!(accessor.value(0)?, "Hello 世界");
    assert_eq!(accessor.value(1)?, "Здравствуй мир");
    assert_eq!(accessor.value(2)?, "مرحبا بالعالم");
    assert_eq!(accessor.value(3)?, "🌍🌎🌏");

    Ok(())
}

#[test]
fn test_dictionary_with_duplicate_values() -> Result<()> {
    // Test that dictionary encoding handles duplicate values correctly
    let values = StringArray::from(vec!["a", "b", "a", "c"]); // Note: "a" appears twice
    let indices = Int32Array::from(vec![Some(0), Some(1), Some(2), Some(3)]);

    let dict_array = DictionaryArray::<Int32Type>::try_new(indices, Arc::new(values))?;
    let array_ref: ArrayRef = Arc::new(dict_array);

    let accessor = create_string_accessor(&array_ref)?;

    // Values should be accessed correctly even with duplicates in dictionary
    assert_eq!(accessor.value(0)?, "a");
    assert_eq!(accessor.value(1)?, "b");
    assert_eq!(accessor.value(2)?, "a"); // Same value as index 0
    assert_eq!(accessor.value(3)?, "c");

    Ok(())
}
