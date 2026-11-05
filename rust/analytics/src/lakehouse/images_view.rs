use super::{
    batch_update::PartitionCreationStrategy,
    block_partition_spec::{BlockProcessor, BlockProcessorMap},
    dataframe_time_bounds::{DataFrameTimeBounds, NamedColumnsTimeBounds},
    image_block_processor::ImageBlockProcessor,
    jit_partitions::update_insert_time_jit_view,
    lakehouse_context::LakehouseContext,
    partition_cache::PartitionCache,
    view::{PartitionSpec, View},
    view_factory::ViewMaker,
};
use crate::{
    images_table::images_table_schema,
    metadata::find_process,
    time::{TimeRange, datetime_to_scalar},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::Schema,
    logical_expr::{Between, Expr, col},
};
use micromegas_ingestion::web_ingestion_service::FORMAT_TRANSIT;
use micromegas_tracing::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

const VIEW_SET_NAME: &str = "images";
const SCHEMA_VERSION: u8 = 2;

lazy_static::lazy_static! {
    static ref TIME_COLUMN: Arc<String> = Arc::new(String::from("time"));
}

fn image_processors() -> Arc<BlockProcessorMap> {
    let mut m: BlockProcessorMap = HashMap::new();
    m.insert(
        FORMAT_TRANSIT,
        Arc::new(ImageBlockProcessor {}) as Arc<dyn BlockProcessor>,
    );
    Arc::new(m)
}

#[derive(Debug)]
pub struct ImagesViewMaker {}

impl ViewMaker for ImagesViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(ImagesView::new(view_instance_id)?))
    }

    fn get_schema_hash(&self) -> Vec<u8> {
        vec![SCHEMA_VERSION]
    }

    fn get_schema(&self) -> Arc<Schema> {
        Arc::new(images_table_schema())
    }
}

#[derive(Clone, Debug)]
pub struct ImagesView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    process_id: sqlx::types::Uuid,
}

impl ImagesView {
    pub fn new(view_instance_id: &str) -> Result<Self> {
        if view_instance_id == "global" {
            anyhow::bail!("ImagesView does not support global view access");
        }
        let process_id = Uuid::parse_str(view_instance_id).with_context(|| "Uuid::parse_str")?;
        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(view_instance_id.into()),
            process_id,
        })
    }
}

#[async_trait]
impl View for ImagesView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        _lakehouse: Arc<LakehouseContext>,
        _existing_partitions: Arc<PartitionCache>,
        _insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>> {
        anyhow::bail!("ImagesView does not support batch partition specs")
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![SCHEMA_VERSION]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(images_table_schema())
    }

    #[span_fn]
    async fn jit_update(
        &self,
        lakehouse: Arc<LakehouseContext>,
        query_range: Option<TimeRange>,
    ) -> Result<()> {
        let process = Arc::new(
            find_process(
                &lakehouse.lake().db_pool,
                &self.process_id,
                &lakehouse.default_audience(),
            )
            .await
            .with_context(|| "find_process")?,
        );
        let query_range =
            query_range.unwrap_or_else(|| TimeRange::new(process.start_time, chrono::Utc::now()));
        update_insert_time_jit_view(
            lakehouse,
            Arc::new(self.clone()),
            process,
            query_range,
            "image",
            image_processors(),
        )
        .await
    }

    fn make_time_filter(&self, begin: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Expr>> {
        Ok(vec![Expr::Between(Between::new(
            col("time").into(),
            false,
            Expr::Literal(datetime_to_scalar(begin), None).into(),
            Expr::Literal(datetime_to_scalar(end), None).into(),
        ))])
    }

    fn get_time_bounds(&self) -> Arc<dyn DataFrameTimeBounds> {
        Arc::new(NamedColumnsTimeBounds::new(
            TIME_COLUMN.clone(),
            TIME_COLUMN.clone(),
        ))
    }

    fn get_update_group(&self) -> Option<i32> {
        None
    }

    fn get_max_partition_time_delta(&self, _strategy: &PartitionCreationStrategy) -> TimeDelta {
        TimeDelta::hours(1)
    }
}
