use object_store::{path::Path, ObjectStore};
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct DataLakeConnection {
    pub db_pool: PgPool,
    pub blob_store: Arc<Box<dyn ObjectStore>>,
    pub blob_store_root: Path,
}

impl DataLakeConnection {
    pub fn new(
        db_pool: PgPool,
        blob_store: Arc<Box<dyn ObjectStore>>,
        blob_store_root: Path,
    ) -> Self {
        Self {
            db_pool,
            blob_store,
            blob_store_root,
        }
    }
}
