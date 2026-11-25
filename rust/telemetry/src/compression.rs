use anyhow::{Context, Result};
use std::io::Read;
use std::io::Write;

/// Compresses a byte slice using LZ4.
///
/// The compression level is set to 10 for a good balance between speed and compression ratio.
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

/// Decompresses a LZ4-compressed byte slice.
///
/// This function will allocate a new `Vec<u8>` to hold the decompressed data.
/// Fails if the decompressed data is larger than `max_decompressed_bytes`, so a
/// small hostile payload can't expand into an unbounded allocation.
pub fn decompress(compressed: &[u8], max_decompressed_bytes: usize) -> Result<Vec<u8>> {
    let mut decompressed = Vec::new();
    let decoder = lz4::Decoder::new(compressed).with_context(|| "allocating lz4 decoder")?;
    // `take` truncates silently; reading one byte past the cap makes the overflow detectable.
    let mut limited = decoder.take((max_decompressed_bytes as u64).saturating_add(1));
    limited
        .read_to_end(&mut decompressed)
        .with_context(|| "reading lz4-compressed buffer")?;
    if decompressed.len() > max_decompressed_bytes {
        anyhow::bail!("decompressed lz4 buffer exceeds {max_decompressed_bytes} bytes");
    }
    let (_reader, res) = limited.into_inner().finish();
    res?;
    Ok(decompressed)
}
