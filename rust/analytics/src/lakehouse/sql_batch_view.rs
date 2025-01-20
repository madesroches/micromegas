use super::{
    partition_cache::{NullPartitionProvider, QueryPartitionProvider},
    query::make_session_context,
    view::{PartitionSpec, View},
    view_factory::ViewFactory,
};
use crate::time::TimeRange;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{arrow::datatypes::Schema, prelude::*, sql::TableReference};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::hash::Hash;
use std::hash::Hasher;
use std::{hash::DefaultHasher, sync::Arc};

#[derive(Debug)]
pub struct SqlBatchView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    event_time_column: Arc<String>, // could be more general - for filtering
    src_query: Arc<String>,
    transform_query: Arc<String>,
    merge_partitions_query: Arc<String>,
    schema: Arc<Schema>,
}

impl SqlBatchView {
    pub async fn new(
        view_set_name: Arc<String>,
        view_instance_id: Arc<String>,
        event_time_column: Arc<String>,
        src_query: Arc<String>,
        transform_query: Arc<String>,
        merge_partitions_query: Arc<String>,
        lake: Arc<DataLakeConnection>,
        view_factory: Arc<ViewFactory>,
    ) -> Result<Self> {
        let null_part_provider = Arc::new(NullPartitionProvider {});
        let ctx = make_session_context(lake, null_part_provider, None, view_factory)
            .await
            .with_context(|| "make_session_context")?;
        let src_df = ctx.sql(&src_query).await?;
        let src_view = src_df.into_view();
        ctx.register_table(
            TableReference::Bare {
                table: "source".into(),
            },
            src_view,
        )?;

        let transformed_df = ctx.sql(&transform_query).await?;
        let schema = transformed_df.schema().inner().clone();

        Ok(Self {
            view_set_name,
            view_instance_id,
            event_time_column,
            src_query,
            transform_query,
            merge_partitions_query,
            schema,
        })
    }
}

#[async_trait]
impl View for SqlBatchView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        _lake: Arc<DataLakeConnection>,
        _part_provider: Arc<dyn QueryPartitionProvider>,
        _begin_insert: DateTime<Utc>,
        _end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>> {
        todo!();
    }
    fn get_file_schema_hash(&self) -> Vec<u8> {
        let mut hasher = DefaultHasher::new();
        self.schema.hash(&mut hasher);
        hasher.finish().to_le_bytes().to_vec()
    }
    fn get_file_schema(&self) -> Arc<Schema> {
        self.schema.clone()
    }
    async fn jit_update(
        &self,
        _lake: Arc<DataLakeConnection>,
        _query_range: Option<TimeRange>,
    ) -> Result<()> {
        anyhow::bail!("jit_update not supported on SqlBatchView");
    }
    fn make_time_filter(&self, _begin: DateTime<Utc>, _end: DateTime<Utc>) -> Result<Vec<Expr>> {
        todo!();
    }
}
