/// Tests for metadata compatibility between Arrow 56.0 and 57.0 formats
use datafusion::arrow::array::Int32Array;
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::parquet::arrow::ArrowWriter;
use micromegas_analytics::arrow_utils::serialize_parquet_metadata;
use micromegas_analytics::lakehouse::metadata_compat::parse_legacy_and_upgrade;
use std::sync::Arc;

/// Test that parse_legacy_and_upgrade can handle new Arrow 57.0 metadata
/// (metadata that already has num_rows set correctly)
#[test]
fn test_legacy_parser_handles_new_format() {
    // Create test data with 5 rows
    let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int32, false)]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5]))],
    )
    .expect("create batch");
    // Write parquet and get metadata (this is Arrow 57.0 format)
    let mut parquet_buffer = Vec::new();
    let mut writer =
        ArrowWriter::try_new(&mut parquet_buffer, schema, None).expect("create writer");
    writer.write(&batch).expect("write batch");
    let metadata = writer.close().expect("close writer");
    assert_eq!(metadata.file_metadata().num_rows(), 5);
    // Serialize the metadata (Arrow 57.0 format with num_rows=5)
    let serialized = serialize_parquet_metadata(&metadata).expect("serialize metadata");
    // Parse with legacy parser - should handle it correctly
    // Pass num_rows=5 (matches what's in metadata)
    let parsed = parse_legacy_and_upgrade(&serialized, 5).expect("parse with legacy parser");
    assert_eq!(parsed.file_metadata().num_rows(), 5);
}

/// Test that parse_legacy_and_upgrade correctly handles the case where
/// metadata has num_rows=0 and we need to inject the correct value
/// This simulates the Arrow 56.0 format where num_rows could be 0
#[test]
#[allow(deprecated)]
fn test_legacy_parser_injects_num_rows_when_zero() {
    use parquet::format::FileMetaData as ThriftFileMetaData;
    use parquet::thrift::TSerializable;
    use thrift::protocol::{TCompactOutputProtocol, TOutputProtocol};
    // Create test data to get proper schema metadata
    let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int32, false)]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5]))],
    )
    .expect("create batch");
    let mut parquet_buffer = Vec::new();
    let mut writer =
        ArrowWriter::try_new(&mut parquet_buffer, schema, None).expect("create writer");
    writer.write(&batch).expect("write batch");
    let metadata = writer.close().expect("close writer");
    // Serialize and parse to get thrift metadata
    let serialized = serialize_parquet_metadata(&metadata).expect("serialize metadata");
    let mut transport = thrift::transport::TBufferChannel::with_capacity(serialized.len(), 0);
    transport.set_readable_bytes(&serialized);
    let mut protocol = thrift::protocol::TCompactInputProtocol::new(transport);
    let mut thrift_meta =
        ThriftFileMetaData::read_from_in_protocol(&mut protocol).expect("read thrift");
    // Manually set num_rows to 0 to simulate Arrow 56.0 format
    thrift_meta.num_rows = 0;
    // Re-serialize with num_rows=0 - use Vec<u8> which auto-grows as needed
    let mut zero_num_rows_bytes: Vec<u8> = Vec::new();
    let mut out_protocol = TCompactOutputProtocol::new(&mut zero_num_rows_bytes);
    thrift_meta
        .write_to_out_protocol(&mut out_protocol)
        .expect("write thrift");
    out_protocol.flush().expect("flush");
    // Parse with legacy parser - should inject num_rows=5
    let parsed =
        parse_legacy_and_upgrade(&zero_num_rows_bytes, 5).expect("parse with legacy parser");
    assert_eq!(parsed.file_metadata().num_rows(), 5);
}

/// Test that parse_legacy_and_upgrade doesn't overwrite correct num_rows
/// This validates that when metadata already has num_rows > 0, we don't replace it
#[test]
fn test_legacy_parser_preserves_existing_num_rows() {
    // Create test data with 5 rows
    let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int32, false)]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5]))],
    )
    .expect("create batch");
    // Write parquet and get metadata (num_rows will be 5)
    let mut parquet_buffer = Vec::new();
    let mut writer =
        ArrowWriter::try_new(&mut parquet_buffer, schema, None).expect("create writer");
    writer.write(&batch).expect("write batch");
    let metadata = writer.close().expect("close writer");
    let serialized = serialize_parquet_metadata(&metadata).expect("serialize metadata");
    // Pass a different num_rows value (999) - should be ignored since metadata has 5
    let parsed = parse_legacy_and_upgrade(&serialized, 999).expect("parse with legacy parser");
    // Should still be 5, not 999
    assert_eq!(parsed.file_metadata().num_rows(), 5);
}
