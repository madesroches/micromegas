use super::{
    partition_source_data::{fetch_partition_source_data, PartitionSourceData},
    view::View,
};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;

pub struct LogView {
    table_set_name: Arc<String>,
    table_instance_id: Arc<String>,
}

impl LogView {
    pub fn new() -> Self {
        Self {
            table_set_name: Arc::new(String::from("log_entries")),
            table_instance_id: Arc::new(String::from("global")),
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

    async fn fetch_source_data(
        &self,
        pool: &sqlx::PgPool,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<PartitionSourceData> {
        fetch_partition_source_data(pool, begin_insert, end_insert, "log").await
    }
}
