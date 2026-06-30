use anyhow::{Context, Result};
use micromegas_object_cache::CacheClientStore;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::{info, warn};
use object_store::ObjectStore;
use sqlx::PgPool;
use std::sync::Arc;

/// A connection to the data lake, including a database pool and a blob storage client.
#[derive(Debug, Clone)]
pub struct DataLakeConnection {
    pub db_pool: PgPool,
    pub blob_storage: Arc<BlobStorage>,
}

impl DataLakeConnection {
    pub fn new(db_pool: PgPool, blob_storage: Arc<BlobStorage>) -> Self {
        Self {
            db_pool,
            blob_storage,
        }
    }
}

pub(crate) fn make_cache_layer() -> impl FnOnce(Arc<dyn ObjectStore>) -> Arc<dyn ObjectStore> {
    move |direct: Arc<dyn ObjectStore>| {
        let cache_url = std::env::var("MICROMEGAS_OBJECT_CACHE_URL").ok();
        let api_key = std::env::var("MICROMEGAS_OBJECT_CACHE_API_KEY").ok();
        if let Some(url) = cache_url {
            if api_key.is_none() {
                warn!(
                    "MICROMEGAS_OBJECT_CACHE_URL is set ({url}) but MICROMEGAS_OBJECT_CACHE_API_KEY is missing: the object cache is disabled and requests will go directly to the store"
                );
                return direct;
            }
            Arc::new(CacheClientStore::new(url, api_key, direct)) as Arc<dyn ObjectStore>
        } else {
            direct
        }
    }
}

/// Connects to the data lake.
pub async fn connect_to_data_lake(
    db_uri: &str,
    object_store_url: &str,
) -> Result<DataLakeConnection> {
    info!("connecting to blob storage");
    let blob_storage = Arc::new(
        BlobStorage::connect_with_layer(object_store_url, make_cache_layer())
            .with_context(|| "connecting to blob storage")?,
    );
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(db_uri)
        .await
        .with_context(|| String::from("Connecting to telemetry database"))?;
    Ok(DataLakeConnection::new(pool, blob_storage))
}
