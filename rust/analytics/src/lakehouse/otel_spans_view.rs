//! Per-process JIT view of OTel spans.
//!
//! `otel_spans` mirrors the `AsyncEventsView` pattern (no batch path; JIT-only;
//! materialized per-process) but the time-conversion plumbing is simpler: OTLP
//! timestamps are absolute nanoseconds (`tsc_frequency = 1_000_000_000`), so we
//! use the plain `find_process` flow instead of the latest-timing variant.
//!
//! Cross-process trace traversal (`WHERE trace_id = X` across services) is a
//! documented v1 limitation — users supply the `process_id` to
//! `view_instance('otel_spans', '<uuid>')` or UNION across multiple instances.

use super::{
    batch_update::PartitionCreationStrategy,
    block_partition_spec::{BlockProcessor, BlockProcessorMap},
    blocks_view::BlocksView,
    dataframe_time_bounds::{DataFrameTimeBounds, NamedColumnsTimeBounds},
    jit_partitions::{
        JitPartitionConfig, generate_process_jit_partitions, is_jit_partition_up_to_date,
        write_partition_from_blocks,
    },
    lakehouse_context::LakehouseContext,
    otel_spans_block_processor::OtelSpansBlockProcessor,
    otel_spans_table::otel_spans_table_schema,
    partition_cache::PartitionCache,
    view::{PartitionSpec, View, ViewMetadata},
    view_factory::ViewMaker,
};
use crate::{
    metadata::find_process,
    time::{TimeRange, datetime_to_scalar},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::Schema,
    logical_expr::{BinaryExpr, Expr, Operator, col},
};
use micromegas_tracing::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

const VIEW_SET_NAME: &str = "otel_spans";
const SCHEMA_VERSION: u8 = 1;
lazy_static::lazy_static! {
    static ref TIME_COLUMN: Arc<String> = Arc::new(String::from("start_time"));
    static ref END_TIME_COLUMN: Arc<String> = Arc::new(String::from("end_time"));
}

/// `ViewMaker` for `OtelSpansView`. Per-process only — `"global"` is rejected.
#[derive(Debug)]
pub struct OtelSpansViewMaker {}

impl ViewMaker for OtelSpansViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(OtelSpansView::new(view_instance_id)?))
    }

    fn get_schema_hash(&self) -> Vec<u8> {
        vec![SCHEMA_VERSION]
    }

    fn get_schema(&self) -> Arc<Schema> {
        Arc::new(otel_spans_table_schema())
    }
}

#[derive(Debug)]
pub struct OtelSpansView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    process_id: sqlx::types::Uuid,
}

impl OtelSpansView {
    pub fn new(view_instance_id: &str) -> Result<Self> {
        if view_instance_id == "global" {
            anyhow::bail!("OtelSpansView does not support global view access");
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
impl View for OtelSpansView {
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
        anyhow::bail!("OtelSpansView does not support batch partition specs");
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![SCHEMA_VERSION]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(otel_spans_table_schema())
    }

    #[span_fn]
    async fn jit_update(
        &self,
        lakehouse: Arc<LakehouseContext>,
        query_range: Option<TimeRange>,
    ) -> Result<()> {
        let process = Arc::new(
            find_process(&lakehouse.lake().db_pool, &self.process_id)
                .await
                .with_context(|| "find_process")?,
        );
        let query_range =
            query_range.unwrap_or_else(|| TimeRange::new(process.start_time, chrono::Utc::now()));

        let blocks_view = BlocksView::new()?;
        let all_partitions = generate_process_jit_partitions(
            &JitPartitionConfig::default(),
            lakehouse.clone(),
            &blocks_view,
            &query_range,
            process.clone(),
            "trace",
        )
        .await
        .with_context(|| "generate_process_jit_partitions")?;
        let view_meta = ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
        };

        let mut block_processors: BlockProcessorMap = HashMap::new();
        block_processors.insert(
            crate::lakehouse::format::FORMAT_OTLP_TRACES,
            Arc::new(OtelSpansBlockProcessor {}) as Arc<dyn BlockProcessor>,
        );

        for part in all_partitions {
            if !is_jit_partition_up_to_date(&lakehouse.lake().db_pool, view_meta.clone(), &part)
                .await?
            {
                write_partition_from_blocks(
                    lakehouse.lake().clone(),
                    view_meta.clone(),
                    self.get_file_schema(),
                    part,
                    block_processors.clone(),
                )
                .await
                .with_context(|| "write_partition_from_blocks")?;
            }
        }
        Ok(())
    }

    fn make_time_filter(&self, begin: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Expr>> {
        // Dual-bound (start_time <= end AND end_time >= begin) so spans whose
        // duration crosses the query window — i.e. start before `begin` or end
        // after `end` — are still included. A `start_time BETWEEN begin AND end`
        // filter would silently drop long-running spans that overlap the window.
        Ok(vec![
            Expr::BinaryExpr(BinaryExpr::new(
                col("start_time").into(),
                Operator::LtEq,
                Expr::Literal(datetime_to_scalar(end), None).into(),
            )),
            Expr::BinaryExpr(BinaryExpr::new(
                col("end_time").into(),
                Operator::GtEq,
                Expr::Literal(datetime_to_scalar(begin), None).into(),
            )),
        ])
    }

    fn get_time_bounds(&self) -> Arc<dyn DataFrameTimeBounds> {
        Arc::new(NamedColumnsTimeBounds::new(
            TIME_COLUMN.clone(),
            END_TIME_COLUMN.clone(),
        ))
    }

    fn get_update_group(&self) -> Option<i32> {
        None // process-specific JIT view; no scheduled updates
    }

    fn get_max_partition_time_delta(&self, _strategy: &PartitionCreationStrategy) -> TimeDelta {
        TimeDelta::hours(1)
    }
}
