use anyhow::{Context, Result};
use std::io::Read;
use std::io::Write;

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
