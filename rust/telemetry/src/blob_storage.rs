use anyhow::Result;
use futures::stream;
use futures::StreamExt;
use object_store::prefix::PrefixStore;
use object_store::{path::Path, ObjectStore};
use std::sync::Arc;

#[derive(Debug)]
pub struct BlobStorage {
    blob_store: Arc<dyn ObjectStore>,
}

impl BlobStorage {
    pub fn new(blob_store: Arc<dyn ObjectStore>, blob_store_root: Path) -> Self {
        Self {
            blob_store: Arc::new(PrefixStore::new(blob_store, blob_store_root)),
        }
    }

    pub fn connect(object_store_url: &str) -> Result<Self> {
        let (blob_store, blob_store_root) =
            object_store::parse_url(&url::Url::parse(object_store_url)?)?;
        Ok(Self {
            blob_store: Arc::new(PrefixStore::new(blob_store, blob_store_root)),
        })
    }

    pub fn inner(&self) -> Arc<dyn ObjectStore> {
        self.blob_store.clone()
    }

    pub async fn put(&self, obj_path: &str, buffer: bytes::Bytes) -> Result<()> {
        self.blob_store
            .put(&Path::from(obj_path), buffer.into())
            .await?;
        Ok(())
    }

    pub async fn read_blob(&self, obj_path: &str) -> Result<bytes::Bytes> {
        let get_result = self.blob_store.get(&Path::from(obj_path)).await?;
        Ok(get_result.bytes().await?)
    }

    pub async fn delete(&self, obj_path: &str) -> Result<()> {
        self.blob_store.delete(&Path::from(obj_path)).await?;
        Ok(())
    }

    pub async fn delete_batch(&self, objects: &[String]) -> Result<()> {
        let path_stream = stream::iter(
            objects
                .iter()
                .map(|obj_path| Path::from(obj_path.as_str()))
                .map(Ok),
        );
        let mut stream = self.blob_store.delete_stream(Box::pin(path_stream));
        while let Some(res) = stream.next().await {
            if let Err(e) = res {
                match e {
                    object_store::Error::NotFound { path: _, source: _ } => Ok(()),
                    ref _other_error => Err(e),
                }?
            }
        }
        Ok(())
    }
}
