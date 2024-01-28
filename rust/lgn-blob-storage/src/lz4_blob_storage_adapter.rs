use std::{
    io::{BufReader, BufWriter, Write},
    path::Path,
};

use async_trait::async_trait;
use tracing::prelude::*;

use super::{BlobStats, BlobStorage, Result};

/// A LZ4-compressed blob storage adapter.
pub struct Lz4BlobStorageAdapter<B: BlobStorage> {
    inner: B,
}

impl<B: BlobStorage> Lz4BlobStorageAdapter<B> {
    pub fn new(inner: B) -> Self {
        Self { inner }
    }

    /// Reads the the full contents of a blob from the storage to the specified writer.
    #[span_fn]
    async fn decompress_blob_to(&self, hash: &str, w: &mut impl Write) -> Result<()> {
        let compressed_data = self.inner.read_blob(hash).await?;
        let reader = BufReader::new(compressed_data.as_slice());
        let mut decoder = match lz4::Decoder::new(reader) {
            Ok(decoder) => decoder,
            Err(e) => {
                return Err(super::Error::forward_with_context(
                    e,
                    format!("could not create LZ4 decoder for blob: {}", hash),
                ))
            }
        };

        if let Err(e) = std::io::copy(&mut decoder, w) {
            return Err(super::Error::forward_with_context(
                e,
                format!("could not LZ4-decode blob: {}", hash),
            ));
        }

        Ok(())
    }
}

#[async_trait]
impl<B: BlobStorage> BlobStorage for Lz4BlobStorageAdapter<B> {
    async fn get_blob_info(&self, hash: &str) -> super::Result<Option<BlobStats>> {
        self.inner.get_blob_info(hash).await
    }

    /// Reads the the full contents of a blob from the storage.
    async fn read_blob(&self, hash: &str) -> Result<Vec<u8>> {
        async_span_scope!("Lz4BlobStorageAdapter::read_blob");
        let mut data = Vec::new();

        if let Some(stats) = self.get_blob_info(hash).await? {
            data.reserve(stats.size as usize);
        }

        self.decompress_blob_to(hash, &mut data).await?;

        Ok(data)
    }

    /// Writes the full contents of a blob to the storage.
    async fn write_blob(&self, hash: &str, contents: &[u8]) -> Result<()> {
        async_span_scope!("Lz4BlobStorageAdapter::write_blob");
        let mut compressed_data = Vec::with_capacity(contents.len());

        let mut encoder = match lz4::EncoderBuilder::new()
            .level(10)
            .build(BufWriter::new(&mut compressed_data))
        {
            Ok(encoder) => encoder,
            Err(e) => {
                return Err(super::Error::forward_with_context(
                    e,
                    format!("could not create LZ4 encoder for blob: {}", hash),
                ))
            }
        };

        if let Err(e) = encoder.write(contents) {
            return Err(super::Error::forward_with_context(
                e,
                format!("could not LZ4-encode blob: {}", hash),
            ));
        }

        if let Err(e) = encoder.finish().1 {
            return Err(super::Error::forward_with_context(
                e,
                format!("could not finish LZ4-encoding blob: {}", hash),
            ));
        }

        self.inner.write_blob(hash, &compressed_data).await
    }

    /// Download a blob from the storage and persist it to disk at the specified
    /// location.
    async fn download_blob(&self, path: &Path, hash: &str) -> Result<()> {
        async_span_scope!("Lz4BlobStorageAdapter::download_blob");
        let mut output_file = match std::fs::File::create(path) {
            Ok(file) => file,
            Err(e) => {
                return Err(super::Error::forward_with_context(
                    e,
                    format!("could not create file: {}", path.display()),
                ))
            }
        };

        self.decompress_blob_to(hash, &mut output_file).await?;

        Ok(())
    }

    async fn delete_blob(&self, name: &str) -> super::Result<()> {
        async_span_scope!("Lz4BlobStorageAdapter::delete_blob");
        self.inner.delete_blob(name).await
    }
}
