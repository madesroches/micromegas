use anyhow::{Context, Result};
use bytes::Bytes;
use datafusion::parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};
use micromegas_tracing::prelude::*;
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
    debug!(
        "parse_legacy_and_upgrade: starting with {} bytes, num_rows={}",
        metadata_bytes.len(),
        num_rows
    );

    // Parse with old thrift API
    let mut transport = thrift::transport::TBufferChannel::with_capacity(metadata_bytes.len(), 0);
    transport.set_readable_bytes(metadata_bytes);
    let mut protocol = TCompactInputProtocol::new(transport);

    debug!("parse_legacy_and_upgrade: reading thrift metadata");
    let mut thrift_meta = ThriftFileMetaData::read_from_in_protocol(&mut protocol)
        .context("parsing legacy metadata with thrift")?;

    debug!(
        "parse_legacy_and_upgrade: thrift_meta.num_rows={}",
        thrift_meta.num_rows
    );

    // Inject num_rows if missing or zero
    if thrift_meta.num_rows == 0 {
        debug!("parse_legacy_and_upgrade: injecting num_rows={}", num_rows);
        thrift_meta.num_rows = num_rows;
    }

    // Re-serialize with thrift (now has num_rows)
    debug!("parse_legacy_and_upgrade: re-serializing with thrift");
    let mut out_transport = thrift::transport::TBufferChannel::with_capacity(0, 8192);
    let mut out_protocol = TCompactOutputProtocol::new(&mut out_transport);
    thrift_meta
        .write_to_out_protocol(&mut out_protocol)
        .context("serializing corrected thrift metadata")?;
    out_protocol.flush()?;

    let corrected_bytes = out_transport.write_bytes();
    debug!(
        "parse_legacy_and_upgrade: re-serialized to {} bytes",
        corrected_bytes.len()
    );

    // Parse with Arrow 57.0 (should work now)
    debug!("parse_legacy_and_upgrade: parsing with Arrow 57.0");
    let result = ParquetMetaDataReader::decode_metadata(&Bytes::copy_from_slice(&corrected_bytes))
        .context("re-parsing with Arrow 57.0")?;

    debug!("parse_legacy_and_upgrade: success!");
    Ok(result)
}
