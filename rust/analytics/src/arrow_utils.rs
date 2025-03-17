use anyhow::{Context, Result};
use bytes::BufMut;
use bytes::{Bytes, BytesMut};
use datafusion::parquet::file::metadata::ParquetMetaDataReader;
use datafusion::{
    arrow::{
        array::{as_struct_array, ListBuilder, StructBuilder},
        record_batch::RecordBatch,
    },
    parquet::file::metadata::ParquetMetaData,
    parquet::file::metadata::ParquetMetaDataWriter,
};

/// make_empty_record_batch returns an empty record batch with an empty schema
pub fn make_empty_record_batch() -> RecordBatch {
    let mut list_builder = ListBuilder::new(StructBuilder::from_fields([], 0));
    let array = list_builder.finish();
    as_struct_array(array.values()).into()
}

pub fn parse_parquet_metadata(bytes: &Bytes) -> Result<ParquetMetaData> {
    ParquetMetaDataReader::decode_metadata(bytes).with_context(|| "parsing ParquetMetaData")
}

pub fn serialize_parquet_metadata(pmd: &ParquetMetaData) -> Result<bytes::Bytes> {
    let mut buffer_writer = BytesMut::new().writer();
    let md_writer = ParquetMetaDataWriter::new(&mut buffer_writer, pmd);
    md_writer
        .finish()
        .with_context(|| "closing ParquetMetaDataWriter")?;
    Ok(buffer_writer.into_inner().freeze())
}
