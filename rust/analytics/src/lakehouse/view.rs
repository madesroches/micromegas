use super::partition_source_data::PartitionSourceData;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;

#[async_trait]
pub trait View {
    fn get_table_set_name(&self) -> Arc<String>;
    fn get_table_instance_id(&self) -> Arc<String>;
    async fn fetch_source_data(
        &self,
        pool: &sqlx::PgPool,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<PartitionSourceData>;
}
