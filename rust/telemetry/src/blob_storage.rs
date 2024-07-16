use anyhow::Result;
use futures::stream;
use futures::StreamExt;
use futures::TryStreamExt;
use object_store::{path::Path, ObjectStore};
use std::sync::Arc;

#[derive(Debug)]
pub struct BlobStorage {
    blob_store: Arc<dyn ObjectStore>,
    blob_store_root: Path,
}

impl BlobStorage {
    pub fn new(blob_store: Arc<dyn ObjectStore>, blob_store_root: Path) -> Self {
        Self {
            blob_store,
            blob_store_root,
        }
    }

    pub fn connect(object_store_url: &str) -> Result<Self> {
        let (blob_store, blob_store_root) =
            object_store::parse_url(&url::Url::parse(object_store_url)?)?;
        Ok(Self {
            blob_store: blob_store.into(),
            blob_store_root,
        })
    }

    pub async fn put(&self, obj_path: &str, buffer: bytes::Bytes) -> Result<()> {
        let full_path = Path::from(format!("{}/{obj_path}", self.blob_store_root));
        self.blob_store.put(&full_path, buffer).await?;
        Ok(())
    }

    pub async fn read_blob(&self, obj_path: &str) -> Result<bytes::Bytes> {
        let full_path = Path::from(format!("{}/{obj_path}", self.blob_store_root));
        let get_result = self.blob_store.get(&full_path).await?;
        Ok(get_result.bytes().await?)
    }

    pub async fn delete(&self, obj_path: &str) -> Result<()> {
        let full_path = Path::from(format!("{}/{obj_path}", self.blob_store_root));
        self.blob_store.delete(&full_path).await?;
        Ok(())
    }

    pub async fn delete_batch(&self, objects: &[String]) -> Result<()> {
        let path_stream = stream::iter(
            objects
                .iter()
                .map(|obj_path| Path::from(format!("{}/{obj_path}", self.blob_store_root)))
                .map(Ok),
        );
        self.blob_store
            .delete_stream(Box::pin(path_stream))
            .map(|res| {
                if let Err(e) = res {
                    match e {
                        object_store::Error::NotFound { path: _, source: _ } => Ok(()),
                        ref _other_error => Err(e),
                    }
                } else {
                    Ok(())
                }
            })
            .try_collect()
            .await?;
        Ok(())
    }
}
