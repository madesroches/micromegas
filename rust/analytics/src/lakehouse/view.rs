use crate::response_writer::ResponseWriter;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::arrow::datatypes::Schema;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

#[async_trait]
pub trait PartitionSpec: Send + Sync {
    fn get_source_data_hash(&self) -> Vec<u8>;
    async fn write(&self, lake: Arc<DataLakeConnection>, writer: Arc<ResponseWriter>)
        -> Result<()>;
}

#[async_trait]
pub trait View: Send + Sync {
    fn get_view_set_name(&self) -> Arc<String>;
    fn get_view_instance_id(&self) -> Arc<String>;
    async fn make_partition_spec(
        &self,
        pool: &sqlx::PgPool,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>>;
    fn get_file_schema_hash(&self) -> Vec<u8>;
    fn get_file_schema(&self) -> Arc<Schema>;
}
