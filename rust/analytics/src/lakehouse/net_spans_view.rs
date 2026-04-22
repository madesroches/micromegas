use super::{
    blocks_view::BlocksView,
    dataframe_time_bounds::{DataFrameTimeBounds, NamedColumnsTimeBounds},
    jit_partitions::{
        JitPartitionConfig, generate_process_jit_partitions, is_jit_partition_up_to_date,
    },
    lakehouse_context::LakehouseContext,
    partition_cache::PartitionCache,
    partition_source_data::{SourceDataBlocksInMemory, hash_to_object_count},
    view::{PartitionSpec, View, ViewMetadata},
    view_factory::{ViewFactory, ViewMaker},
};
use crate::{
    lakehouse::write_partition::{PartitionRowSet, write_partition_from_rows},
    metadata::{StreamMetadata, find_process_with_latest_timing},
    net_span_tree::make_net_span_tree,
    net_spans_table::{NetSpanRecordBuilder, net_spans_table_schema},
    response_writer::ResponseWriter,
    time::{ConvertTicks, TimeRange, datetime_to_scalar, make_time_converter_from_latest_timing},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::logical_expr::{BinaryExpr, Expr, Operator};
use datafusion::{arrow::datatypes::Schema, logical_expr::expr_fn::col};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::{blob_storage::BlobStorage, types::block::BlockMetadata};
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use uuid::Uuid;

const VIEW_SET_NAME: &str = "net_spans";
const SCHEMA_VERSION: u8 = 0;
const NET_STREAM_TAG: &str = "net";

lazy_static::lazy_static! {
    static ref BEGIN_TIME_COLUMN: Arc<String> = Arc::new(String::from("begin_time"));
    static ref END_TIME_COLUMN: Arc<String> = Arc::new(String::from("end_time"));
}

/// A `ViewMaker` for creating `NetSpansView` instances.
#[derive(Debug)]
pub struct NetSpansViewMaker {
    view_factory: Arc<ViewFactory>,
}

impl NetSpansViewMaker {
    pub fn new(view_factory: Arc<ViewFactory>) -> Self {
        Self { view_factory }
    }
}

impl ViewMaker for NetSpansViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(NetSpansView::new(
            view_instance_id,
            self.view_factory.clone(),
        )?))
    }

    fn get_schema_hash(&self) -> Vec<u8> {
        vec![SCHEMA_VERSION]
    }

    fn get_schema(&self) -> Arc<Schema> {
        Arc::new(net_spans_table_schema())
    }
}

/// A view of network bandwidth spans (Connection / Object / Property / RPC).
#[derive(Debug)]
pub struct NetSpansView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    process_id: sqlx::types::Uuid,
    view_factory: Arc<ViewFactory>,
}

impl NetSpansView {
    pub fn new(view_instance_id: &str, view_factory: Arc<ViewFactory>) -> Result<Self> {
        if view_instance_id == "global" {
            anyhow::bail!("NetSpansView does not support global view access");
        }
        let process_id = Uuid::parse_str(view_instance_id).with_context(|| "Uuid::parse_str")?;
        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(view_instance_id.into()),
            process_id,
            view_factory,
        })
    }
}

#[span_fn]
async fn append_net_span_tree(
    record_builder: &mut NetSpanRecordBuilder,
    convert_ticks: &ConvertTicks,
    blocks: &[BlockMetadata],
    blob_storage: Arc<BlobStorage>,
    stream: &StreamMetadata,
    process_id: Arc<String>,
) -> Result<()> {
    make_net_span_tree(
        blocks,
        record_builder,
        blob_storage,
        stream,
        process_id,
        convert_ticks.clone(),
    )
    .await
    .with_context(|| "make_net_span_tree")
}

/// Writes a partition from a set of blocks.
#[span_fn]
async fn write_partition(
    lake: Arc<DataLakeConnection>,
    view_meta: ViewMetadata,
    schema: Arc<Schema>,
    convert_ticks: &ConvertTicks,
    spec: &SourceDataBlocksInMemory,
    process_id: Arc<String>,
) -> Result<()> {
    let nb_events = hash_to_object_count(&spec.block_ids_hash)? as usize;
    info!("nb_events: {nb_events}");
    let mut record_builder = NetSpanRecordBuilder::with_capacity(nb_events / 2);
    if spec.blocks.is_empty() {
        anyhow::bail!("empty partition spec");
    }
    let min_insert_time = spec.blocks[0].block.insert_time;
    let max_insert_time = spec.blocks[spec.blocks.len() - 1].block.insert_time;

    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    let join_handle = spawn_with_context(write_partition_from_rows(
        lake.clone(),
        view_meta,
        schema,
        TimeRange::new(min_insert_time, max_insert_time),
        spec.block_ids_hash.clone(),
        rx,
        null_response_writer,
    ));

    // A process has exactly one net stream (the Unreal NetTraceWriter creates a
    // single NetStream tagged "net"), so all blocks here share the same stream.
    // Validate the invariant so a future regression surfaces early instead of
    // silently tagging rows with the wrong stream_id.
    let stream = spec.blocks[0].stream.clone();
    for b in &spec.blocks {
        anyhow::ensure!(
            b.stream.stream_id == stream.stream_id,
            "net_spans partition contains multiple streams ({} and {}); expected one per process",
            stream.stream_id,
            b.stream.stream_id,
        );
    }

    // Split on time gaps: blocks are contiguous in ticks by construction (the
    // flush point's `Now` is shared between the closing and new block), so a
    // gap means a block was lost — don't stitch the span stack across it.
    let mut blocks_to_process: Vec<BlockMetadata> = vec![];
    let mut last_end: Option<i64> = None;
    for block in &spec.blocks {
        let contiguous = last_end
            .map(|e| block.block.begin_ticks == e)
            .unwrap_or(true);
        if !contiguous {
            append_net_span_tree(
                &mut record_builder,
                convert_ticks,
                &blocks_to_process,
                lake.blob_storage.clone(),
                &stream,
                process_id.clone(),
            )
            .await?;
            blocks_to_process = vec![];
        }
        blocks_to_process.push(block.block.clone());
        last_end = Some(block.block.end_ticks);
    }
    if !blocks_to_process.is_empty() {
        append_net_span_tree(
            &mut record_builder,
            convert_ticks,
            &blocks_to_process,
            lake.blob_storage.clone(),
            &stream,
            process_id.clone(),
        )
        .await?;
    }

    let min_time_row = convert_ticks.delta_ticks_to_time(spec.blocks[0].block.begin_ticks);
    let max_time_row =
        convert_ticks.delta_ticks_to_time(spec.blocks[spec.blocks.len() - 1].block.end_ticks);
    let rows_time_range = record_builder
        .get_time_range()
        .unwrap_or(TimeRange::new(min_time_row, max_time_row));
    let nb_rows = record_builder.len();
    let rows = record_builder
        .finish()
        .with_context(|| "record_builder.finish()")?;
    info!("writing {} rows", nb_rows);
    if nb_rows > 0 {
        tx.send(PartitionRowSet {
            rows_time_range,
            rows,
        })
        .await?;
    }
    drop(tx);
    join_handle.await??;
    Ok(())
}

/// Rebuilds the partition if it's missing or out of date.
#[span_fn]
async fn update_partition(
    lake: Arc<DataLakeConnection>,
    view_meta: ViewMetadata,
    schema: Arc<Schema>,
    convert_ticks: &ConvertTicks,
    spec: &SourceDataBlocksInMemory,
    process_id: Arc<String>,
) -> Result<()> {
    if is_jit_partition_up_to_date(&lake.db_pool, view_meta.clone(), spec).await? {
        return Ok(());
    }
    write_partition(lake, view_meta, schema, convert_ticks, spec, process_id)
        .await
        .with_context(|| "write_partition")?;
    Ok(())
}

#[async_trait]
impl View for NetSpansView {
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
        anyhow::bail!("NetSpansView does not support batch partition specs")
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![SCHEMA_VERSION]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(net_spans_table_schema())
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
            &self.process_id,
            query_range,
        )
        .await
        .with_context(|| "find_process_with_latest_timing")?;

        let process = Arc::new(process);
        let query_range =
            query_range.unwrap_or_else(|| TimeRange::new(process.start_time, last_block_end_time));

        let convert_ticks = make_time_converter_from_latest_timing(
            &process,
            last_block_end_ticks,
            last_block_end_time,
        )
        .with_context(|| "make_time_converter_from_latest_timing")?;

        let blocks_view = BlocksView::new()?;
        let all_partitions = generate_process_jit_partitions(
            &JitPartitionConfig::default(),
            lakehouse.clone(),
            &blocks_view,
            &query_range,
            process.clone(),
            NET_STREAM_TAG,
        )
        .await
        .with_context(|| "generate_process_jit_partitions")?;

        let process_id_str = Arc::new(self.process_id.to_string());
        for part in &all_partitions {
            update_partition(
                lakehouse.lake().clone(),
                ViewMetadata {
                    view_set_name: self.get_view_set_name(),
                    view_instance_id: self.get_view_instance_id(),
                    file_schema_hash: self.get_file_schema_hash(),
                },
                self.get_file_schema(),
                &convert_ticks,
                part,
                process_id_str.clone(),
            )
            .await
            .with_context(|| "update_partition")?;
        }
        Ok(())
    }

    fn make_time_filter(&self, begin: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Expr>> {
        Ok(vec![
            Expr::BinaryExpr(BinaryExpr::new(
                col("begin_time").into(),
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
            BEGIN_TIME_COLUMN.clone(),
            END_TIME_COLUMN.clone(),
        ))
    }

    fn get_update_group(&self) -> Option<i32> {
        None
    }
}
