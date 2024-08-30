use super::{
    block_partition_spec::BlockPartitionSpec,
    log_block_processor::LogBlockProcessor,
    partition_source_data::fetch_partition_source_data,
    view::{PartitionSpec, View, ViewMetadata},
    view_factory::ViewMaker,
};
use crate::log_entries_table::log_table_schema;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::datatypes::Schema, catalog::TableProvider, execution::context::SessionContext,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

const VIEW_SET_NAME: &str = "log_entries";
const VIEW_INSTANCE_ID: &str = "global";

pub struct LogViewMaker {}

impl ViewMaker for LogViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(LogView::new(view_instance_id)?))
    }
}

pub struct LogView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
}

impl LogView {
    pub fn new(view_instance_id: &str) -> Result<Self> {
        if view_instance_id != "global" {
            anyhow::bail!("only the global view instance is implemented");
        }

        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(String::from(VIEW_INSTANCE_ID)),
        })
    }
}

#[async_trait]
impl View for LogView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
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
        Ok(Arc::new(BlockPartitionSpec {
            view_metadata: ViewMetadata {
                view_set_name: self.view_set_name.clone(),
                view_instance_id: self.view_instance_id.clone(),
                file_schema: self.get_file_schema(),
                file_schema_hash: self.get_file_schema_hash(),
            },
            begin_insert,
            end_insert,
            source_data,
            block_processor: Arc::new(LogBlockProcessor {}),
        }))
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![0]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(log_table_schema())
    }

    async fn jit_update(
        &self,
        _lake: Arc<DataLakeConnection>,
        _begin_insert: DateTime<Utc>,
        _end_insert: DateTime<Utc>,
    ) -> Result<()> {
        if *self.view_instance_id == "global" {
            // this view instance is updated using the deamon
            return Ok(());
        }
        anyhow::bail!("not implemented");
    }

    async fn make_filtering_table_provider(
        &self,
        ctx: &SessionContext,
        full_table_name: &str,
        begin: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Arc<dyn TableProvider>> {
        let row_filter = ctx
            .sql(&format!(
                "SELECT * from {full_table_name} WHERE time BETWEEN '{}' AND '{}';",
                begin.to_rfc3339(),
                end.to_rfc3339(),
            ))
            .await?;
        Ok(row_filter.into_view())
    }
}
