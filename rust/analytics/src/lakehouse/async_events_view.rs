use super::{
    batch_update::PartitionCreationStrategy,
    block_partition_spec::{BlockProcessor, BlockProcessorMap},
    dataframe_time_bounds::{DataFrameTimeBounds, NamedColumnsTimeBounds},
    jit_partitions::update_insert_time_jit_view,
    lakehouse_context::LakehouseContext,
    partition_cache::PartitionCache,
    view::{PartitionSpec, View},
    view_factory::{ViewFactory, ViewMaker},
};
use crate::{
    async_events_table::async_events_table_schema,
    metadata::find_process_with_latest_timing,
    time::{TimeRange, datetime_to_scalar, make_time_converter_from_latest_timing},
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

use super::async_events_block_processor::AsyncEventsBlockProcessor;

const VIEW_SET_NAME: &str = "async_events";
const SCHEMA_VERSION: u8 = 4; // Version 4: added properties; version 3: widened dictionary keys to Int32
lazy_static::lazy_static! {
    static ref TIME_COLUMN: Arc<String> = Arc::new(String::from("time"));
}

/// A `ViewMaker` for creating `AsyncEventsView` instances.
#[derive(Debug)]
pub struct AsyncEventsViewMaker {
    view_factory: Arc<ViewFactory>,
}

impl AsyncEventsViewMaker {
    pub fn new(view_factory: Arc<ViewFactory>) -> Self {
        Self { view_factory }
    }
}

impl ViewMaker for AsyncEventsViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(AsyncEventsView::new(
            view_instance_id,
            self.view_factory.clone(),
        )?))
    }

    fn get_schema_hash(&self) -> Vec<u8> {
        vec![SCHEMA_VERSION]
    }

    fn get_schema(&self) -> Arc<Schema> {
        Arc::new(async_events_table_schema())
    }
}

/// A view of async span events.
#[derive(Clone, Debug)]
pub struct AsyncEventsView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    process_id: Option<sqlx::types::Uuid>,
    view_factory: Arc<ViewFactory>,
}

impl AsyncEventsView {
    pub fn new(view_instance_id: &str, view_factory: Arc<ViewFactory>) -> Result<Self> {
        if view_instance_id == "global" {
            anyhow::bail!("AsyncEventsView does not support global view access");
        }

        let process_id =
            Some(Uuid::parse_str(view_instance_id).with_context(|| "Uuid::parse_str")?);

        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(view_instance_id.into()),
            process_id,
            view_factory,
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
        _lakehouse: Arc<LakehouseContext>,
        _existing_partitions: Arc<PartitionCache>,
        _insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>> {
        anyhow::bail!("AsyncEventsView does not support batch partition specs");
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![SCHEMA_VERSION]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(async_events_table_schema())
    }

    #[span_fn]
    async fn jit_update(
        &self,
        lakehouse: Arc<LakehouseContext>,
        query_range: Option<TimeRange>,
    ) -> Result<()> {
        let (process, last_block_end_ticks, last_block_end_time) = find_process_with_latest_timing(
            lakehouse.clone(),
            self.view_factory.clone(),
            &self
                .process_id
                .with_context(|| "getting a view's process_id")?,
            query_range,
        )
        .await
        .with_context(|| "find_process_with_latest_timing")?;

        let process = Arc::new(process);
        let query_range =
            query_range.unwrap_or_else(|| TimeRange::new(process.start_time, last_block_end_time));

        // Create a consistent ConvertTicks using the latest timing information
        let convert_ticks = Arc::new(
            make_time_converter_from_latest_timing(
                &process,
                last_block_end_ticks,
                last_block_end_time,
            )
            .with_context(|| "make_time_converter_from_latest_timing")?,
        );

        // async_events only sources native transit streams (tagged "cpu");
        // OTel spans go to the dedicated otel_spans view.
        let mut block_processors: BlockProcessorMap = HashMap::new();
        block_processors.insert(
            FORMAT_TRANSIT,
            Arc::new(AsyncEventsBlockProcessor::new(convert_ticks.clone()))
                as Arc<dyn BlockProcessor>,
        );
        update_insert_time_jit_view(
            lakehouse,
            Arc::new(self.clone()),
            process,
            query_range,
            "cpu",
            Arc::new(block_processors),
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
        None // Process-specific views don't use update groups
    }

    fn get_max_partition_time_delta(&self, _strategy: &PartitionCreationStrategy) -> TimeDelta {
        TimeDelta::hours(1)
    }
}
