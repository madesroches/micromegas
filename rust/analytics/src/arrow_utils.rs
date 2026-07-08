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
use micromegas_tracing::prelude::*;

/// Creates an empty record batch with an empty schema.
pub fn make_empty_record_batch() -> RecordBatch {
    let mut list_builder = ListBuilder::new(StructBuilder::from_fields([], 0));
    let array = list_builder.finish();
    as_struct_array(array.values()).into()
}

/// Parses Parquet metadata from a byte slice.
#[span_fn]
pub fn parse_parquet_metadata(bytes: &Bytes) -> Result<ParquetMetaData> {
    ParquetMetaDataReader::decode_metadata(bytes).with_context(|| "parsing ParquetMetaData")
}
