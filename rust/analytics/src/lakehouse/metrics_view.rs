use crate::metrics_table::metrics_table_schema;

use super::{
    block_partition_spec::BlockPartitionSpec,
    metrics_block_processor::MetricsBlockProcessor,
    partition_source_data::fetch_partition_source_data,
    view::{PartitionSpec, View},
    view_factory::ViewMaker,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::arrow::datatypes::Schema;
use std::sync::Arc;

const VIEW_SET_NAME: &str = "measures";
const VIEW_INSTANCE_ID: &str = "global";

pub struct MetricsViewMaker {}

impl ViewMaker for MetricsViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(MetricsView::new(view_instance_id)?))
    }
}

pub struct MetricsView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
}

impl MetricsView {
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
impl View for MetricsView {
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
        let source_data = fetch_partition_source_data(pool, begin_insert, end_insert, "metrics")
            .await
            .with_context(|| "fetch_partition_source_data")?;
        Ok(Arc::new(BlockPartitionSpec {
            view_set_name: self.view_set_name.clone(),
            view_instance_id: self.view_instance_id.clone(),
            begin_insert,
            end_insert,
            file_schema: self.get_file_schema(),
            file_schema_hash: self.get_file_schema_hash(),
            source_data,
            block_processor: Arc::new(MetricsBlockProcessor {}),
        }))
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![0]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(metrics_table_schema())
    }
}
