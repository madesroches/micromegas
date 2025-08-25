use super::{
    batch_update::PartitionCreationStrategy,
    blocks_view::BlocksView,
    dataframe_time_bounds::{DataFrameTimeBounds, NamedColumnsTimeBounds},
    jit_partitions::{JitPartitionConfig, write_partition_from_blocks},
    partition_cache::PartitionCache,
    view::{PartitionSpec, View, ViewMetadata},
    view_factory::{ViewFactory, ViewMaker},
};
use crate::{
    async_events_table::async_events_table_schema,
    lakehouse::jit_partitions::{generate_jit_partitions, is_jit_partition_up_to_date},
    metadata::{find_process_with_latest_timing, list_process_streams_tagged},
    time::{TimeRange, datetime_to_scalar, make_time_converter_from_latest_timing},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::Schema,
    execution::runtime_env::RuntimeEnv,
    logical_expr::{Between, Expr, col},
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;
use uuid::Uuid;

use super::async_events_block_processor::AsyncEventsBlockProcessor;

const VIEW_SET_NAME: &str = "async_events";
lazy_static::lazy_static! {
    static ref TIME_COLUMN: Arc<String> = Arc::new(String::from("time"));
}

/// A `ViewMaker` for creating `AsyncEventsView` instances.
#[derive(Debug)]
pub struct AsyncEventsViewMaker {}

impl ViewMaker for AsyncEventsViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(AsyncEventsView::new(view_instance_id)?))
    }
}

/// A view of async span events.
#[derive(Debug)]
pub struct AsyncEventsView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    process_id: Option<sqlx::types::Uuid>,
}

impl AsyncEventsView {
    pub fn new(view_instance_id: &str) -> Result<Self> {
        if view_instance_id == "global" {
            anyhow::bail!("AsyncEventsView does not support global view access");
        }

        let process_id =
            Some(Uuid::parse_str(view_instance_id).with_context(|| "Uuid::parse_str")?);

        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(view_instance_id.into()),
            process_id,
        })
    }
}

#[async_trait]
impl View for AsyncEventsView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        _runtime: Arc<RuntimeEnv>,
        _lake: Arc<DataLakeConnection>,
        _existing_partitions: Arc<PartitionCache>,
        _insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>> {
        anyhow::bail!("AsyncEventsView does not support batch partition specs");
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![1] // Updated to version 1 to include depth field
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(async_events_table_schema())
    }

    async fn jit_update(
        &self,
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        query_range: Option<TimeRange>,
    ) -> Result<()> {
        // Create a minimal view factory with just the blocks view needed for processes view
        let blocks_view = Arc::new(BlocksView::new()?);
        let minimal_view_factory = Arc::new(ViewFactory::new(vec![blocks_view]));

        let (process, last_block_end_ticks, last_block_end_time) = find_process_with_latest_timing(
            runtime.clone(),
            lake.clone(),
            minimal_view_factory,
            &self
                .process_id
                .with_context(|| "getting a view's process_id")?,
        )
        .await
        .with_context(|| "find_process_with_latest_timing")?;

        let process = Arc::new(process);
        let query_range =
            query_range.unwrap_or_else(|| TimeRange::new(process.start_time, chrono::Utc::now()));

        // Create a consistent ConvertTicks using the latest timing information
        let convert_ticks = Arc::new(
            make_time_converter_from_latest_timing(
                &process,
                last_block_end_ticks,
                last_block_end_time,
            )
            .with_context(|| "make_time_converter_from_latest_timing")?,
        );

        // Use all thread streams since async events are recorded in thread streams
        let streams = list_process_streams_tagged(&lake.db_pool, process.process_id, "cpu")
            .await
            .with_context(|| "list_process_streams_tagged")?;
        let mut all_partitions = vec![];
        let blocks_view = BlocksView::new()?;
        for stream in streams {
            let mut partitions = generate_jit_partitions(
                &JitPartitionConfig::default(),
                runtime.clone(),
                lake.clone(),
                &blocks_view,
                &query_range,
                Arc::new(stream),
                process.clone(),
            )
            .await
            .with_context(|| "generate_jit_partitions")?;
            all_partitions.append(&mut partitions);
        }
        let view_meta = ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
        };

        for part in all_partitions {
            if !is_jit_partition_up_to_date(&lake.db_pool, view_meta.clone(), &part).await? {
                write_partition_from_blocks(
                    lake.clone(),
                    view_meta.clone(),
                    self.get_file_schema(),
                    part,
                    Arc::new(AsyncEventsBlockProcessor::new(convert_ticks.clone())),
                )
                .await
                .with_context(|| "write_partition_from_blocks")?;
            }
        }
        Ok(())
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
        None // Process-specific views don't use update groups
    }

    fn get_max_partition_time_delta(&self, _strategy: &PartitionCreationStrategy) -> TimeDelta {
        TimeDelta::hours(1)
    }
}
