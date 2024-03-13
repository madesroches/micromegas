use std::sync::Arc;

use anyhow::Result;
use lgn_blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;

pub struct DiskCache {
    storage: Arc<dyn BlobStorage>,
}

impl DiskCache {
    pub fn new(storage: Arc<dyn BlobStorage>) -> Self {
        Self { storage }
    }

    pub async fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        if !self.storage.blob_exists(name).await? {
            return Ok(None);
        }
        info!("reading {}", name);
        let buffer = self.storage.read_blob(name).await?;
        Ok(Some(buffer))
    }

    pub async fn put(&self, name: &str, buffer: Vec<u8>) -> Result<()> {
        if !self.storage.blob_exists(name).await? {
            info!("writing {}", name);
            self.storage.write_blob(name, &buffer).await?;
            info!("writing {} completed", name);
        }
        Ok(())
    }

    pub async fn get_cached_object<T>(&self, name: &str) -> Option<T>
    where
        T: prost::Message + Default,
    {
        match self.get(name).await {
            Err(e) => {
                error!("Error reading {} from cache: {}", name, e);
                None
            }
            Ok(Some(buffer)) => match T::decode(&*buffer) {
                Ok(obj) => Some(obj),
                Err(e) => {
                    error!("Error reading {} from cache: {}", name, e);
                    None
                }
            },
            Ok(None) => None,
        }
    }

    pub async fn get_or_put<FOBJ, T>(&self, name: &str, f: FOBJ) -> Result<T>
    where
        FOBJ: std::future::Future<Output = Result<T>>,
        T: Default + prost::Message,
    {
        if let Some(obj) = self.get_cached_object::<T>(name).await {
            return Ok(obj);
        }
        let obj = f.await?;
        if let Err(e) = self.put(name, obj.encode_to_vec()).await {
            error!("Error writing {} to cache: {}", name, e);
        }
        Ok(obj)
    }
}
