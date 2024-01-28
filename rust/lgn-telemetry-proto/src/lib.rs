//! telemetry protocols

#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::wildcard_imports,
    clippy::similar_names,
    clippy::use_self,
    clippy::return_self_not_must_use
)]

use std::io::{Read, Write};

use anyhow::{Context, Result};

pub mod telemetry {
    tonic::include_proto!("telemetry");
}

pub mod health {
    tonic::include_proto!("health");
}

pub mod ingestion {
    tonic::include_proto!("ingestion");
}

pub mod analytics {
    tonic::include_proto!("analytics");
}

pub fn compress(src: &[u8]) -> Result<Vec<u8>> {
    let mut compressed = Vec::new();
    let mut encoder = lz4::EncoderBuilder::new()
        .level(10)
        .build(&mut compressed)
        .with_context(|| "allocating lz4 encoder")?;
    let _size = encoder
        .write(src)
        .with_context(|| "writing to lz4 encoder")?;
    let (_writer, res) = encoder.finish();
    res.with_context(|| "closing lz4 encoder")?;
    Ok(compressed)
}

pub fn decompress(compressed: &[u8]) -> Result<Vec<u8>> {
    let mut decompressed = Vec::new();
    let mut decoder = lz4::Decoder::new(compressed).with_context(|| "allocating lz4 decoder")?;
    let _size = decoder
        .read_to_end(&mut decompressed)
        .with_context(|| "reading lz4-compressed buffer")?;
    let (_reader, res) = decoder.finish();
    res?;
    Ok(decompressed)
}
