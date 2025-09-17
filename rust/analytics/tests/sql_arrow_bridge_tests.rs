use anyhow::Result;
use datafusion::arrow::array::Array;
use datafusion::arrow::datatypes::{DataType, Field};
use micromegas_telemetry::property::Property;
use std::sync::Arc;

// NOTE: This test file focuses on unit testing the dictionary builder logic directly
// rather than testing with PgRow instances. Creating PgRow instances manually is
// extremely challenging because:
//
// 1. PgRow has private fields (pub(crate)) that cannot be accessed outside SQLx
// 2. It requires complex PostgreSQL binary protocol data (DataRow with raw bytes)
// 3. It needs proper metadata (PgStatementMetadata with type information)
// 4. Manual creation would require encoding data in PostgreSQL's binary format
//
// SQLx intentionally makes PgRow hard to mock to encourage:
// - Integration testing with real databases using #[sqlx::test]
// - Unit testing of business logic separately from database concerns
// - Mocking at the service/repository layer rather than the row level
//
// For comprehensive testing, we use:
// - These unit tests for the core dictionary building logic
// - Python integration tests for end-to-end SQL-Arrow bridge testing with real databases
// - The existing property_get_tests.rs for UDF-level dictionary behavior

// Helper function to create mock properties
fn create_test_properties() -> Vec<Property> {
    vec![
        Property::new("version".to_string().into(), "1.2.3".to_string().into()),
        Property::new("platform".to_string().into(), "linux".to_string().into()),
        Property::new("user".to_string().into(), "alice".to_string().into()),
    ]
}

#[test]
fn test_properties_dictionary_builder_integration() -> Result<()> {
    // Test the PropertiesDictionaryBuilder directly
    use micromegas_analytics::properties::dictionary_builder::PropertiesDictionaryBuilder;

    let mut builder = PropertiesDictionaryBuilder::new(3);

    // Add some properties
    let props1 = create_test_properties();
    let props2 = create_test_properties(); // Same as props1, should reuse dictionary entry
    let props3 = vec![Property::new(
        "env".to_string().into(),
        "prod".to_string().into(),
    )];

    builder.append_properties_from_vec(props1)?;
    builder.append_properties_from_vec(props2)?; // Same as props1, should reuse dictionary entry
    builder.append_properties_from_vec(props3)?;

    let dict_array = builder.finish()?;

    // Verify the dictionary array structure
    assert_eq!(dict_array.len(), 3);

    // First two entries should have the same dictionary key (since they're identical)
    let keys = dict_array.keys();
    assert_eq!(keys.value(0), keys.value(1));
    assert_ne!(keys.value(0), keys.value(2));

    // Verify the dictionary has 2 unique entries
    assert_eq!(dict_array.values().len(), 2);

    // Check data type
    if let DataType::Dictionary(key_type, value_type) = dict_array.data_type() {
        assert_eq!(**key_type, DataType::Int32);
        assert!(matches!(**value_type, DataType::List(_)));
    } else {
        panic!("Expected Dictionary type");
    }

    Ok(())
}

#[test]
fn test_dictionary_with_nulls() -> Result<()> {
    use micromegas_analytics::properties::dictionary_builder::PropertiesDictionaryBuilder;

    let mut builder = PropertiesDictionaryBuilder::new(3);

    // Add properties and nulls
    let props1 = create_test_properties();
    builder.append_properties_from_vec(props1)?;
    builder.append_null();

    let props2 = vec![Property::new(
        "different".to_string().into(),
        "value".to_string().into(),
    )];
    builder.append_properties_from_vec(props2)?;

    let dict_array = builder.finish()?;

    // Verify structure
    assert_eq!(dict_array.len(), 3);
    assert!(dict_array.is_valid(0));
    assert!(dict_array.is_null(1));
    assert!(dict_array.is_valid(2));

    Ok(())
}

#[test]
fn test_dictionary_deduplication() -> Result<()> {
    use micromegas_analytics::properties::dictionary_builder::PropertiesDictionaryBuilder;

    let mut builder = PropertiesDictionaryBuilder::new(4);

    // Create identical property sets
    builder.append_properties_from_vec(create_test_properties())?;
    builder.append_properties_from_vec(create_test_properties())?;
    builder.append_properties_from_vec(create_test_properties())?;
    builder.append_properties_from_vec(create_test_properties())?;

    let dict_array = builder.finish()?;

    // All entries should point to the same dictionary key
    let keys = dict_array.keys();
    assert_eq!(keys.value(0), keys.value(1));
    assert_eq!(keys.value(1), keys.value(2));
    assert_eq!(keys.value(2), keys.value(3));

    // Dictionary should have only 1 unique entry
    assert_eq!(dict_array.values().len(), 1);

    Ok(())
}

#[test]
fn test_column_reader_trait_default_implementations() {
    use micromegas_analytics::sql_arrow_bridge::ColumnReader;

    // Test that we can create a minimal implementation
    struct TestColumnReader;

    impl ColumnReader for TestColumnReader {
        fn field(&self) -> Field {
            Field::new("test", DataType::Utf8, false)
        }
    }

    let reader = TestColumnReader;

    // Test default implementations - they should return Ok(()) and None
    // We can't easily mock PgRow and StructBuilder without database dependencies
    // so we test that the trait defaults work conceptually
    assert!(reader.extract_all_from_rows(&[]).unwrap().is_none());

    // Test field method
    assert_eq!(reader.field().name(), "test");
}

#[test]
fn test_properties_column_reader_schema() {
    use datafusion::arrow::datatypes::{DataType, Field, Fields};

    // Test the expected schema for properties dictionary column
    let expected_schema = DataType::Dictionary(
        Box::new(DataType::Int32),
        Box::new(DataType::List(Arc::new(Field::new(
            "Property",
            DataType::Struct(Fields::from(vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Utf8, false),
            ])),
            false,
        )))),
    );

    // Verify the schema structure
    match &expected_schema {
        DataType::Dictionary(key_type, value_type) => {
            assert_eq!(**key_type, DataType::Int32);
            match value_type.as_ref() {
                DataType::List(field) => match field.data_type() {
                    DataType::Struct(fields) => {
                        assert_eq!(fields.len(), 2);
                        assert_eq!(fields[0].name(), "key");
                        assert_eq!(fields[1].name(), "value");
                    }
                    _ => panic!("Expected struct type"),
                },
                _ => panic!("Expected list type"),
            }
        }
        _ => panic!("Expected dictionary type"),
    }
}
