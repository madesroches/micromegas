use object_store::{path::Path, ObjectStore};
use std::sync::Arc;

#[derive(Clone)]
pub struct DataLakeConnection {
    pub db_pool: sqlx::any::AnyPool,
    pub blob_store: Arc<Box<dyn ObjectStore>>,
    pub blob_store_root: Path,
}

impl DataLakeConnection {
    pub fn new(
        db_pool: sqlx::AnyPool,
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
