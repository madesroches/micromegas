use anyhow::{Context, Result};
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::info;
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

/// Connects to the data lake.
pub async fn connect_to_data_lake(
    db_uri: &str,
    object_store_url: &str,
) -> Result<DataLakeConnection> {
    info!("connecting to blob storage");
    let blob_storage = Arc::new(
        BlobStorage::connect(object_store_url).with_context(|| "connecting to blob storage")?,
    );
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(db_uri)
        .await
        .with_context(|| String::from("Connecting to telemetry database"))?;
    Ok(DataLakeConnection::new(pool, blob_storage))
}
