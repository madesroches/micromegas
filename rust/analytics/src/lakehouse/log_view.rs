use super::{
    log_partition_spec::LogPartitionSpec,
    partition_source_data::fetch_partition_source_data,
    view::{PartitionSpec, View},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;

pub const TABLE_SET_NAME: &str = "log_entries";
pub const TABLE_INSTANCE_ID: &str = "global";

pub struct LogView {
    table_set_name: Arc<String>,
    table_instance_id: Arc<String>,
}

impl LogView {
    pub fn new() -> Self {
        Self {
            table_set_name: Arc::new(String::from(TABLE_SET_NAME)),
            table_instance_id: Arc::new(String::from(TABLE_INSTANCE_ID)),
        }
    }
}

impl Default for LogView {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl View for LogView {
    fn get_table_set_name(&self) -> Arc<String> {
        self.table_set_name.clone()
    }

    fn get_table_instance_id(&self) -> Arc<String> {
        self.table_instance_id.clone()
    }

    async fn make_partition_spec(
        &self,
        pool: &sqlx::PgPool,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>> {
        let source_data = fetch_partition_source_data(pool, begin_insert, end_insert, "log")
            .await
            .with_context(|| "fetch_partition_source_data")?;
        Ok(Arc::new(LogPartitionSpec {
            begin_insert,
            end_insert,
            file_schema_hash: self.get_file_schema_hash(),
            source_data,
        }))
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![0]
    }
}
