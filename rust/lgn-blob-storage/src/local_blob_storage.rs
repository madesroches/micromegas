use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::prelude::*;
use std::path::PathBuf;
use tokio::fs;

use super::{BlobStats, BoxedAsyncRead, BoxedAsyncWrite, StreamingBlobStorage};

pub struct LocalBlobStorage(PathBuf);

impl LocalBlobStorage {
    /// returns a ``LocalBlobStorage`` instance ready to be used
    ///
    /// # Errors
    /// Could fail to create the directory if it does not already exist
    #[span_fn]
    pub async fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)
            .await
            .with_context(|| {
                format!(
                    "could not create local blobs directory at: {}",
                    root.display()
                )
            })
            .map(|_| Self(root))
    }

    #[span_fn]
    async fn get_blob_reader_file(&self, hash: &str) -> super::Result<fs::File> {
        let blob_path = self.0.join(hash);

        match fs::File::open(&blob_path).await {
            Ok(file) => Ok(file),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Err(super::Error::NoSuchBlob(hash.to_string()))
                } else {
                    Err(super::Error::forward_with_context(
                        e,
                        format!("could not open blob file: {}", blob_path.display()),
                    ))
                }
            }
        }
    }
}

#[async_trait]
impl StreamingBlobStorage for LocalBlobStorage {
    async fn get_blob_info(&self, hash: &str) -> super::Result<Option<BlobStats>> {
        async_span_scope!("LocalBlobStorage::get_blob_info");
        match self.get_blob_reader_file(hash).await {
            Ok(file) => match file.metadata().await {
                Ok(metadata) => Ok(Some(BlobStats {
                    size: metadata.len(),
                })),
                Err(e) => Err(super::Error::forward_with_context(
                    e,
                    format!("could not get metadata for blob: {}", hash),
                )),
            },
            Err(e) => match e {
                super::Error::NoSuchBlob(_) => Ok(None),
                super::Error::Other { .. } => Err(e),
            },
        }
    }

    async fn get_blob_reader(&self, hash: &str) -> super::Result<BoxedAsyncRead> {
        async_span_scope!("LocalBlobStorage::get_blob_reader");
        let file = self.get_blob_reader_file(hash).await?;

        Ok(Box::pin(file))
    }

    async fn get_blob_writer(&self, hash: &str) -> super::Result<Option<BoxedAsyncWrite>> {
        async_span_scope!("LocalBlobStorage::get_blob_writer");
        let blob_path = self.0.join(hash);

        if fs::metadata(&blob_path).await.is_ok() {
            return Ok(None);
        }

        // Nothing prevents a reader from accessing a partially written blob.
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&blob_path)
            .await
            .map_err(|e| {
                super::Error::forward_with_context(
                    e,
                    format!("could not create blob file: {}", blob_path.display()),
                )
            })?;

        Ok(Some(Box::pin(file)))
    }

    async fn delete_blob(&self, name: &str) -> super::Result<()> {
        async_span_scope!("LocalBlobStorage::delete_blob");
        let blob_path = self.0.join(name);

        match fs::remove_file(&blob_path).await {
            Ok(_) => Ok(()),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(super::Error::forward_with_context(
                        e,
                        format!("could not delete blob file: {}", blob_path.display()),
                    ))
                }
            }
        }
    }
}
