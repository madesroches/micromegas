use std::sync::Arc;

use lgn_blob_storage::BlobStorage;

#[derive(Clone)]
pub struct DataLakeConnection {
    pub db_pool: sqlx::any::AnyPool,
    pub blob_storage: Arc<dyn BlobStorage>,
}

impl DataLakeConnection {
    pub fn new(db_pool: sqlx::AnyPool, blob_storage: Arc<dyn BlobStorage>) -> Self {
        Self {
            db_pool,
            blob_storage,
        }
    }
}
