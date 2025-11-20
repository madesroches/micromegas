use anyhow::{Context, Result};
use bytes::Bytes;
use datafusion::parquet::file::metadata::ParquetMetaDataReader;
use datafusion::{
    arrow::{
        array::{ListBuilder, StructBuilder, as_struct_array},
        record_batch::RecordBatch,
    },
    parquet::file::metadata::ParquetMetaData,
};

/// Creates an empty record batch with an empty schema.
pub fn make_empty_record_batch() -> RecordBatch {
    let mut list_builder = ListBuilder::new(StructBuilder::from_fields([], 0));
    let array = list_builder.finish();
    as_struct_array(array.values()).into()
}

/// Parses Parquet metadata from a byte slice.
pub fn parse_parquet_metadata(bytes: &Bytes) -> Result<ParquetMetaData> {
    ParquetMetaDataReader::decode_metadata(bytes).with_context(|| "parsing ParquetMetaData")
}

/// Serializes Parquet metadata to a byte slice.
///
/// This uses `ParquetMetaDataWriter` to serialize the metadata, then extracts
/// just the FileMetaData portion that `decode_metadata()` expects.
///
/// ## Background
/// `ParquetMetaDataWriter` outputs: [Page Indexes][FileMetaData][Length][PAR1]
/// But `decode_metadata()` expects just the raw FileMetaData thrift bytes.
/// We extract the FileMetaData portion using the footer length field.
pub fn serialize_parquet_metadata(pmd: &ParquetMetaData) -> Result<bytes::Bytes> {
    use datafusion::parquet::file::metadata::ParquetMetaDataWriter;

    // Serialize the full footer format
    let mut buffer = Vec::new();
    let md_writer = ParquetMetaDataWriter::new(&mut buffer, pmd);
    md_writer
        .finish()
        .with_context(|| "serializing parquet metadata")?;

    let serialized = bytes::Bytes::from(buffer);

    // Extract just the FileMetaData portion using Parquet footer format
    // The footer structure is: [...][FileMetaData][metadata_len: u32][magic: u32]
    const FOOTER_SIZE: usize = 8; // 4 bytes for length + 4 bytes for PAR1 magic
    const LENGTH_SIZE: usize = 4;

    if serialized.len() < FOOTER_SIZE {
        anyhow::bail!("Serialized metadata too small: {} bytes", serialized.len());
    }

    // Read the FileMetaData length from the footer
    let length_offset = serialized.len() - FOOTER_SIZE;
    let footer_len_bytes = &serialized[length_offset..length_offset + LENGTH_SIZE];
    let metadata_len = u32::from_le_bytes(
        footer_len_bytes
            .try_into()
            .with_context(|| "reading footer length")?,
    ) as usize;

    // Calculate where FileMetaData starts
    let footer_start = serialized
        .len()
        .checked_sub(FOOTER_SIZE + metadata_len)
        .with_context(|| {
            format!(
                "Invalid footer length: {} (total size: {})",
                metadata_len,
                serialized.len()
            )
        })?;

    // Extract just the FileMetaData bytes (excluding page indexes and footer suffix)
    let file_metadata_bytes = serialized.slice(footer_start..length_offset);

    Ok(file_metadata_bytes)
}
