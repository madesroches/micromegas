//! Legion Blob Storage
//!
//! read & write binary files that could be in a local folder or in cloud storage

// crate-specific lint exceptions:
//#![allow()]

mod aws_s3_blob_storage;
mod error;
mod local_blob_storage;
mod lz4_blob_storage_adapter;

pub use aws_s3_blob_storage::{AwsS3BlobStorage, AwsS3Url};
pub use error::{Error, Result};
pub use local_blob_storage::LocalBlobStorage;
pub use lz4_blob_storage_adapter::Lz4BlobStorageAdapter;
use tracing::prelude::*;

use async_trait::async_trait;
use std::{path::Path, pin::Pin};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub type BoxedAsyncRead = Pin<Box<dyn AsyncRead + Send>>;
pub type BoxedAsyncWrite = Pin<Box<dyn AsyncWrite + Send>>;

#[derive(Debug)]
pub struct BlobStats {
    pub size: u64,
}

/// A trait for blob storage backends that implement efficient sequential reads
/// and writes.
#[async_trait]
pub trait StreamingBlobStorage: Send + Sync {
    async fn blob_exists(&self, name: &str) -> Result<bool> {
        self.get_blob_info(name).await.map(|info| info.is_some())
    }

    /// Read information about a blob.
    ///
    /// If the blob does not exist, Ok(None) is returned.
    async fn get_blob_info(&self, name: &str) -> Result<Option<BlobStats>>;

    /// Reads a blob from the storage.
    ///
    /// If no such blob exists, Error::NoSuchBlob is returned.
    ///
    /// In any other case, an error is returned.
    async fn get_blob_reader(&self, name: &str) -> Result<BoxedAsyncRead>;

    /// Writes a blob to the storage.
    ///
    /// If the blob already exists, None is returned and no further action is
    /// required.
    ///
    /// In any other case, an error is returned.
    async fn get_blob_writer(&self, name: &str) -> Result<Option<BoxedAsyncWrite>>;

    /// Reads the the full contents of a blob from the storage.
    async fn read_blob(&self, name: &str) -> Result<Vec<u8>> {
        async_span_scope!("StreamingBlobStorage::read_blob");
        let mut reader = self.get_blob_reader(name).await?;
        let mut contents = Vec::new();

        reader.read_to_end(&mut contents).await.map_err(|e| {
            Error::forward_with_context(e, format!("could not read blob: {}", name))
        })?;

        Ok(contents)
    }

    /// Writes the full contents of a blob to the storage.
    async fn write_blob(&self, name: &str, content: &[u8]) -> Result<()> {
        async_span_scope!("StreamingBlobStorage::write_blob");
        let writer = self.get_blob_writer(name).await?;
        if let Some(mut writer) = writer {
            writer.write_all(content).await.map_err(|e| {
                Error::forward_with_context(e, format!("could not write blob: {}", name))
            })?;
            writer.shutdown().await.map_err(|e| {
                Error::forward_with_context(e, format!("could not shutdown writer: {}", name))
            })?;
        }
        Ok(())
    }

    /// Download a blob from the storage and persist it to disk at the specified
    /// location.
    async fn download_blob(&self, path: &Path, name: &str) -> Result<()> {
        async_span_scope!("StreamingBlobStorage::download_blob");
        let mut reader = self.get_blob_reader(name).await?;
        let mut writer = tokio::fs::File::create(path).await.map_err(|e| {
            Error::forward_with_context(
                e,
                format!("could not create destination file: {}", path.display()),
            )
        })?;

        tokio::io::copy(&mut reader, &mut writer)
            .await
            .map_err(|e| {
                Error::forward_with_context(e, format!("could not copy blob data: {}", name))
            })?;

        Ok(())
    }

    /// Deletes a blob from the storage.
    ///
    /// If the blob does not exist, no error is returned.
    async fn delete_blob(&self, name: &str) -> Result<()>;
}

#[async_trait]
pub trait BlobStorage: Send + Sync {
    async fn blob_exists(&self, name: &str) -> Result<bool> {
        self.get_blob_info(name).await.map(|info| info.is_some())
    }

    /// Read information about a blob.
    ///
    /// If the blob does not exist, Ok(None) is returned.
    async fn get_blob_info(&self, name: &str) -> Result<Option<BlobStats>>;

    /// Reads the the full contents of a blob from the storage.
    async fn read_blob(&self, name: &str) -> Result<Vec<u8>>;

    /// Writes the full contents of a blob to the storage.
    /// warning: nothing prevents a reader from accessing a partially written blob.
    async fn write_blob(&self, name: &str, content: &[u8]) -> Result<()>;

    /// Download a blob from the storage and persist it to disk at the specified
    /// location.
    async fn download_blob(&self, path: &Path, name: &str) -> Result<()>;

    /// Deletes a blob from the storage.
    ///
    /// If the blob does not exist, no error is returned.
    async fn delete_blob(&self, name: &str) -> Result<()>;
}

/// Blanket implementation for all blob streaming storage backends.
#[async_trait]
impl<T: StreamingBlobStorage> BlobStorage for T {
    #[span_fn]
    async fn get_blob_info(&self, name: &str) -> Result<Option<BlobStats>> {
        StreamingBlobStorage::get_blob_info(self, name).await
    }

    /// Reads the the full contents of a blob from the storage.
    async fn read_blob(&self, name: &str) -> Result<Vec<u8>> {
        StreamingBlobStorage::read_blob(self, name).await
    }

    /// Writes the full contents of a blob to the storage.
    async fn write_blob(&self, name: &str, content: &[u8]) -> Result<()> {
        StreamingBlobStorage::write_blob(self, name, content).await
    }

    /// Download a blob from the storage and persist it to disk at the specified
    /// location.
    async fn download_blob(&self, path: &Path, name: &str) -> Result<()> {
        StreamingBlobStorage::download_blob(self, path, name).await
    }

    async fn delete_blob(&self, name: &str) -> Result<()> {
        StreamingBlobStorage::delete_blob(self, name).await
    }
}
