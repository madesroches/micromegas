use super::{
    blocks_view::BlocksView,
    dataframe_time_bounds::{DataFrameTimeBounds, NamedColumnsTimeBounds},
    jit_partitions::{
        BlockOrder, JitPartitionConfig, blocks_insert_time_range, generate_process_jit_partitions,
        group_contiguous_block_chains, is_jit_partition_up_to_date,
    },
    lakehouse_context::LakehouseContext,
    partition_cache::PartitionCache,
    partition_source_data::{SourceDataBlocksInMemory, hash_to_object_count},
    view::{PartitionSpec, View, ViewMetadata},
    view_factory::{ViewFactory, ViewMaker},
};
use crate::{
    lakehouse::write_partition::{PartitionRowSet, RetireMatch, write_partition_from_rows},
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
const SCHEMA_VERSION: u8 = 2;
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
    same_run_ranges: &[TimeRange],
) -> Result<()> {
    let nb_events = hash_to_object_count(&spec.block_ids_hash)? as usize;
    info!("nb_events: {nb_events}");
    if spec.blocks.is_empty() {
        anyhow::bail!("empty partition spec");
    }
    // NetSpansView is grouped under BlockOrder::EventTime (see jit_update below), so spec.blocks
    // is event-time ordered, not insert-time ordered -- blocks_insert_time_range computes the real
    // min/max rather than reading list endpoints.
    let insert_range =
        blocks_insert_time_range(&spec.blocks).with_context(|| "blocks_insert_time_range")?;

    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    let join_handle = spawn_with_context(write_partition_from_rows(
        lake.clone(),
        view_meta,
        schema,
        insert_range,
        spec.block_ids_hash.clone(),
        None,
        RetireMatch::Overlap,
        same_run_ranges.to_vec(),
        rx,
        null_response_writer,
    ));

    let build_result: Result<Option<PartitionRowSet>> = async {
        let mut record_builder = NetSpanRecordBuilder::with_capacity(nb_events / 2);
        let stream = spec.blocks[0].stream.clone();
        for b in &spec.blocks {
            anyhow::ensure!(
                b.stream.stream_id == stream.stream_id,
                "net_spans partition contains multiple streams ({} and {}); expected one per process",
                stream.stream_id,
                b.stream.stream_id,
            );
        }
        // One net span tree per unbroken chain of blocks; see `group_contiguous_block_chains`.
        for chain in group_contiguous_block_chains(&spec.blocks) {
            append_net_span_tree(
                &mut record_builder,
                convert_ticks,
                &chain,
                lake.blob_storage.clone(),
                &stream,
                process_id.clone(),
            )
            .await?;
        }
        // Real min/max over the blocks, not list endpoints: fallback only, since
        // record_builder.get_time_range() normally supplies the actual row bounds below.
        // `spec.blocks` is non-empty (checked above), so min/max are always `Some` -- surface a
        // violation of that as an error rather than silently substituting tick 0, matching
        // thread_spans_view's equivalent computation.
        let rows_time_range = match record_builder.get_time_range() {
            Some(range) => range,
            None => {
                let min_ticks = spec
                    .blocks
                    .iter()
                    .map(|b| b.block.begin_ticks)
                    .min()
                    .with_context(|| "empty partition spec")?;
                let max_ticks = spec
                    .blocks
                    .iter()
                    .map(|b| b.block.end_ticks)
                    .max()
                    .with_context(|| "empty partition spec")?;
                TimeRange::new(
                    convert_ticks.delta_ticks_to_time(min_ticks),
                    convert_ticks.delta_ticks_to_time(max_ticks),
                )
            }
        };
        let nb_rows = record_builder.len();
        let rows = record_builder
            .finish()
            .with_context(|| "record_builder.finish()")?;
        info!("writing {nb_rows} rows");
        if nb_rows > 0 {
            Ok(Some(PartitionRowSet {
                rows_time_range,
                rows,
                max_sort_key_time: None,
            }))
        } else {
            Ok(None)
        }
    }
    .await;

    match build_result {
        Ok(Some(row_set)) => {
            tx.send(Ok(row_set)).await?;
            drop(tx);
            join_handle.await??;
            Ok(())
        }
        Ok(None) => {
            drop(tx);
            join_handle.await??;
            Ok(())
        }
        Err(e) => {
            warn!(
                "aborting net-spans partition write for block {:?}: {e:?}",
                spec.block_ids_hash
            );
            let _ = tx
                .send(Err(anyhow::anyhow!("net-spans build aborted")))
                .await;
            drop(tx);
            match join_handle.await {
                Ok(Ok(())) => {}
                Ok(Err(writer_err)) => {
                    debug!("net-spans writer task error during abort: {writer_err:?}");
                }
                Err(join_err) => {
                    warn!("net-spans writer task panicked during abort: {join_err:?}");
                }
            }
            Err(e)
        }
    }
}

/// Rebuilds the partition if it's missing or out of date.
///
/// `same_run_ranges` accumulates the insert ranges this `jit_update` run has already handled --
/// written, or found already up to date -- earlier in its loop; see `ThreadSpansView`'s
/// `update_partition` and `RetireMatch::Overlap`'s docs for why.
#[span_fn]
async fn update_partition(
    lake: Arc<DataLakeConnection>,
    view_meta: ViewMetadata,
    schema: Arc<Schema>,
    convert_ticks: &ConvertTicks,
    spec: &SourceDataBlocksInMemory,
    process_id: Arc<String>,
    same_run_ranges: &mut Vec<TimeRange>,
) -> Result<()> {
    let insert_range =
        blocks_insert_time_range(&spec.blocks).with_context(|| "blocks_insert_time_range")?;
    if is_jit_partition_up_to_date(
        &lake.db_pool,
        view_meta.clone(),
        spec,
        BlockOrder::EventTime,
    )
    .await?
    {
        same_run_ranges.push(insert_range);
        return Ok(());
    }
    write_partition(
        lake,
        view_meta,
        schema,
        convert_ticks,
        spec,
        process_id,
        same_run_ranges.as_slice(),
    )
    .await
    .with_context(|| "write_partition")?;
    same_run_ranges.push(insert_range);
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
        // NetSpansView builds cross-block net span trees, so its JIT partitions must be
        // event-time ordered, not insert-time ordered -- see BlockOrder::EventTime's docs. (Unlike
        // ThreadSpansView, NetSpansView declares no ScanOrdering::Concatenated today, so it does
        // not also need a monotonicity check -- see thread_spans_view.rs's
        // ensure_begin_non_decreasing -- and its PartitionRowSet always carries
        // max_sort_key_time: None. If it ever does declare that ordering, it must first add an
        // ensure_begin_non_decreasing equivalent and populate max_sort_key_time, per
        // tasks/completed/thread_spans_segment_boundary_overlap_plan.md.)
        let config = JitPartitionConfig {
            block_order: BlockOrder::EventTime,
            ..Default::default()
        };
        let all_partitions = generate_process_jit_partitions(
            &config,
            lakehouse.clone(),
            &blocks_view,
            &query_range,
            process.clone(),
            NET_STREAM_TAG,
        )
        .await
        .with_context(|| "generate_process_jit_partitions")?;

        let process_id_str = Arc::new(self.process_id.to_string());
        // Accumulates this run's own already-handled insert ranges across the loop, so a later
        // partition's retire step never retires an earlier one from this same run -- see
        // `update_partition`'s and `RetireMatch::Overlap`'s docs.
        let mut same_run_ranges: Vec<TimeRange> = Vec::new();
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
                &mut same_run_ranges,
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
