/// Test verifying metadata serialization/deserialization round-trip
///
/// This test ensures that serialize_parquet_metadata() correctly extracts
/// the FileMetaData portion from ParquetMetaDataWriter's output, allowing
/// successful deserialization with ParquetMetaDataReader::decode_metadata().
#[test]
fn test_parquet_metadata_format_extraction() {
    use datafusion::arrow::array::Int32Array;
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;
    use datafusion::parquet::arrow::ArrowWriter;
    use micromegas_analytics::arrow_utils::{parse_parquet_metadata, serialize_parquet_metadata};
    use std::sync::Arc;

    // Create test data
    let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int32, false)]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5]))],
    )
    .unwrap();

    // Write parquet file and get metadata
    let mut parquet_buffer = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut parquet_buffer, schema, None).unwrap();
    writer.write(&batch).unwrap();
    let metadata = writer.close().unwrap();

    assert_eq!(metadata.file_metadata().num_rows(), 5);

    // Serialize using our function
    let serialized = serialize_parquet_metadata(&metadata).unwrap();

    // Deserialize and verify round-trip
    let decoded = parse_parquet_metadata(&serialized).unwrap();
    assert_eq!(decoded.file_metadata().num_rows(), 5);
}
