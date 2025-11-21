use anyhow::{Context, Result};
use bytes::Bytes;
use datafusion::parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};
use micromegas_tracing::prelude::*;
#[allow(deprecated)]
use parquet::format::FileMetaData as ThriftFileMetaData;
use parquet::thrift::TSerializable;
use thrift::protocol::{TCompactInputProtocol, TCompactOutputProtocol, TOutputProtocol};

/// Parse legacy metadata (Arrow 56.0) and convert to new format (Arrow 57.0)
///
/// This function handles the migration from Arrow 56.0 to 57.0 by:
/// 1. Parsing legacy metadata using the deprecated thrift API
/// 2. Injecting the required `num_rows` field if missing or zero
/// 3. Re-serializing with thrift to produce corrected bytes
/// 4. Parsing with Arrow 57.0's standard parser
#[allow(deprecated)]
pub fn parse_legacy_and_upgrade(metadata_bytes: &[u8], num_rows: i64) -> Result<ParquetMetaData> {
    // Parse with old thrift API
    let mut transport = thrift::transport::TBufferChannel::with_capacity(metadata_bytes.len(), 0);
    transport.set_readable_bytes(metadata_bytes);
    let mut protocol = TCompactInputProtocol::new(transport);
    let mut thrift_meta = ThriftFileMetaData::read_from_in_protocol(&mut protocol)
        .context("parsing legacy metadata with thrift")?;
    // Inject num_rows if missing or zero
    if thrift_meta.num_rows == 0 {
        trace!("injecting num_rows={} into legacy metadata", num_rows);
        thrift_meta.num_rows = num_rows;
    }
    // Re-serialize with thrift (now has num_rows)
    let mut out_transport = thrift::transport::TBufferChannel::with_capacity(0, 8192);
    let mut out_protocol = TCompactOutputProtocol::new(&mut out_transport);
    thrift_meta
        .write_to_out_protocol(&mut out_protocol)
        .context("serializing corrected thrift metadata")?;
    out_protocol.flush()?;
    let corrected_bytes = out_transport.write_bytes();
    // Parse with Arrow 57.0 (should work now)
    ParquetMetaDataReader::decode_metadata(&Bytes::copy_from_slice(&corrected_bytes))
        .context("re-parsing with Arrow 57.0")
}
