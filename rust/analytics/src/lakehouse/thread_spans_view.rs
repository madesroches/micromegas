use super::{
    view::{PartitionSpec, View},
    view_factory::ViewMaker,
};
use crate::span_table::get_spans_schema;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::arrow::datatypes::Schema;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

const VIEW_SET_NAME: &str = "thread_spans";

pub struct ThreadSpansViewMaker {}

impl ViewMaker for ThreadSpansViewMaker {
    fn make_view(&self, stream_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(ThreadSpansView::new(stream_id)?))
    }
}

pub struct ThreadSpansView {
    view_set_name: Arc<String>,
    stream_id: Arc<String>,
}

impl ThreadSpansView {
    pub fn new(stream_id: &str) -> Result<Self> {
        if stream_id == "global" {
            anyhow::bail!("the global view is not implemented for thread spans");
        }

        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            stream_id: Arc::new(String::from(stream_id)),
        })
    }
}

#[async_trait]
impl View for ThreadSpansView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.stream_id.clone()
    }

    async fn make_partition_spec(
        &self,
        _pool: &sqlx::PgPool,
        _begin_insert: DateTime<Utc>,
        _end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>> {
        anyhow::bail!("not implemented")
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![0]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(get_spans_schema())
    }

    async fn jit_update(
        &self,
        _lake: Arc<DataLakeConnection>,
        _begin_insert: DateTime<Utc>,
        _end_insert: DateTime<Utc>,
    ) -> Result<()> {
        anyhow::bail!("not implemented");
    }
}
