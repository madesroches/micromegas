use super::{
    blocks_view::BlocksView,
    dataframe_time_bounds::{DataFrameTimeBounds, NamedColumnsTimeBounds},
    jit_partitions::{
        BlockOrder, JitPartitionConfig, blocks_insert_time_range, generate_stream_jit_partitions,
        group_contiguous_block_chains, is_jit_partition_up_to_date,
    },
    lakehouse_context::LakehouseContext,
    partition_cache::PartitionCache,
    partition_source_data::{SourceDataBlocksInMemory, hash_to_object_count},
    partitioned_execution_plan::{OrderingBounds, ScanOrdering},
    view::{PartitionSpec, ScanSortColumn, View, ViewMetadata},
    view_factory::{ViewFactory, ViewMaker},
};
use crate::{
    call_tree::make_call_tree,
    dfext::typed_column::typed_column_by_name,
    lakehouse::write_partition::{PartitionRowSet, RetireMatch, write_partition_from_rows},
    metadata::{find_process_with_latest_timing, find_stream_from_view},
    response_writer::ResponseWriter,
    span_table::{SpanRecordBuilder, get_spans_schema},
    time::{ConvertTicks, TimeRange, datetime_to_scalar, make_time_converter_from_latest_timing},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::arrow::array::TimestampNanosecondArray;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::logical_expr::{BinaryExpr, Expr, Operator};
use datafusion::{arrow::datatypes::Schema, logical_expr::expr_fn::col};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::{blob_storage::BlobStorage, types::block::BlockMetadata};
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use uuid::Uuid;

const VIEW_SET_NAME: &str = "thread_spans";
const SCHEMA_VERSION: u8 = 3;
lazy_static::lazy_static! {
    static ref MIN_TIME_COLUMN: Arc<String> = Arc::new( String::from("begin"));
    static ref MAX_TIME_COLUMN: Arc<String> = Arc::new( String::from("end"));
}

/// A `ViewMaker` for creating `ThreadSpansView` instances.
#[derive(Debug)]
pub struct ThreadSpansViewMaker {
    view_factory: Arc<ViewFactory>,
}

impl ThreadSpansViewMaker {
    pub fn new(view_factory: Arc<ViewFactory>) -> Self {
        Self { view_factory }
    }
}

impl ViewMaker for ThreadSpansViewMaker {
    fn make_view(&self, stream_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(ThreadSpansView::new(
            stream_id,
            self.view_factory.clone(),
        )?))
    }

    fn get_schema_hash(&self) -> Vec<u8> {
        vec![SCHEMA_VERSION]
    }

    fn get_schema(&self) -> Arc<Schema> {
        Arc::new(get_spans_schema())
    }
}

/// A view of thread spans.
#[derive(Debug)]
pub struct ThreadSpansView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    stream_id: sqlx::types::Uuid,
    view_factory: Arc<ViewFactory>,
}

impl ThreadSpansView {
    pub fn new(view_instance_id: &str, view_factory: Arc<ViewFactory>) -> Result<Self> {
        if view_instance_id == "global" {
            anyhow::bail!("the global view is not implemented for thread spans");
        }

        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(String::from(view_instance_id)),
            stream_id: Uuid::parse_str(view_instance_id).with_context(|| "Uuid::parse_str")?,
            view_factory,
        })
    }
}

#[span_fn]
async fn append_call_tree(
    record_builder: &mut SpanRecordBuilder,
    convert_ticks: &ConvertTicks,
    blocks: &[BlockMetadata],
    blob_storage: Arc<BlobStorage>,
    stream: &crate::metadata::StreamMetadata,
) -> Result<()> {
    let call_tree = make_call_tree(
        blocks,
        convert_ticks.delta_ticks_to_ns(blocks[0].begin_ticks),
        convert_ticks.delta_ticks_to_ns(blocks[blocks.len() - 1].end_ticks),
        None,
        blob_storage,
        convert_ticks.clone(),
        stream,
    )
    .await
    .with_context(|| "make_call_tree")?;
    record_builder
        .append_call_tree(&call_tree)
        .with_context(|| "adding call tree to span record builder")?;
    Ok(())
}

/// Verifies that `batch`'s `begin` column is non-decreasing, naming `stream_id` and the offending
/// row/values in both the returned error and (unlike a plain `anyhow::ensure!`) an `error!` log
/// line emitted immediately before it: nothing on `ensure_begin_non_decreasing`'s callers' error
/// propagation path (`MaterializedView::scan` -> DataFusion planning -> the flight SQL service)
/// logs a planning-time error at error level, so the check logs itself instead of relying on that
/// propagation (see the plan's Design §5 for the full trace).
///
/// `pub`, not inlined into `write_partition`, so `rust/analytics/tests/` (an external integration
/// crate that can only reach `pub` items) can call it directly with a hand-built batch.
pub fn ensure_begin_non_decreasing(stream_id: &str, batch: &RecordBatch) -> Result<()> {
    let begins: &TimestampNanosecondArray = typed_column_by_name(batch, "begin")?;
    let mut previous: Option<i64> = None;
    for i in 0..begins.len() {
        let begin = begins.value(i);
        if let Some(prev) = previous
            && begin < prev
        {
            let msg = format!(
                "thread_spans stream {stream_id}: begin regressed at row {i}: {begin} < {prev}"
            );
            error!("{msg}");
            anyhow::bail!(msg);
        }
        previous = Some(begin);
    }
    Ok(())
}

/// Writes a partition from a set of blocks.
#[span_fn]
async fn write_partition(
    lake: Arc<DataLakeConnection>,
    view_meta: ViewMetadata,
    schema: Arc<Schema>,
    convert_ticks: &ConvertTicks,
    spec: &SourceDataBlocksInMemory,
    same_run_ranges: &[TimeRange],
) -> Result<()> {
    let nb_events = hash_to_object_count(&spec.block_ids_hash)? as usize;
    info!("nb_events: {nb_events}");
    if spec.blocks.is_empty() {
        anyhow::bail!("empty partition spec");
    }
    // JIT partitions here are grouped under BlockOrder::EventTime (see jit_update below), so
    // spec.blocks is event-time ordered, not insert-time ordered -- blocks_insert_time_range computes
    // the real min/max rather than reading list endpoints.
    let stream_id = spec.blocks[0].stream.stream_id.to_string();
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

    let build_result: Result<PartitionRowSet> = async {
        let mut record_builder = SpanRecordBuilder::with_capacity(nb_events / 2);
        // One call tree per unbroken chain of blocks, so a span opened in one block and closed in
        // the next is reconstructed whole; see `group_contiguous_block_chains`.
        for chain in group_contiguous_block_chains(&spec.blocks) {
            append_call_tree(
                &mut record_builder,
                convert_ticks,
                &chain,
                lake.blob_storage.clone(),
                &spec.blocks[0].stream,
            )
            .await?;
        }
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
        let min_time_row = convert_ticks.delta_ticks_to_time(min_ticks);
        let max_time_row = convert_ticks.delta_ticks_to_time(max_ticks);
        let rows = record_builder
            .finish()
            .with_context(|| "record_builder.finish()")?;
        ensure_begin_non_decreasing(&stream_id, &rows)
            .with_context(|| "ensure_begin_non_decreasing")?;
        info!("writing {} rows", rows.num_rows());
        // The true max `begin` is simply the last row's value: rows are verified
        // non-decreasing on `begin` above, and one SpanRecordBuilder accumulates every chain
        // into exactly one RecordBatch (see SpanRecordBuilder::finish), so "the last row" is the
        // partition-global max with no per-chain scoping hole. Zero rows is reachable in
        // practice (e.g. every event filtered out by the chain range, or a block carrying only
        // async events), not just theoretically -- guard it rather than indexing an empty column.
        let max_sort_key_time = if rows.num_rows() == 0 {
            None
        } else {
            let begins: &TimestampNanosecondArray = typed_column_by_name(&rows, "begin")?;
            Some(DateTime::from_timestamp_nanos(
                begins.value(begins.len() - 1),
            ))
        };
        Ok(PartitionRowSet {
            rows_time_range: TimeRange::new(min_time_row, max_time_row),
            rows,
            max_sort_key_time,
        })
    }
    .await;

    match build_result {
        Ok(row_set) => {
            tx.send(Ok(row_set)).await?;
            drop(tx);
            join_handle.await??;
            Ok(())
        }
        Err(e) => {
            warn!(
                "aborting thread-spans partition write for block {:?}: {e:?}",
                spec.block_ids_hash
            );
            let _ = tx
                .send(Err(anyhow::anyhow!("thread-spans build aborted")))
                .await;
            drop(tx);
            match join_handle.await {
                Ok(Ok(())) => {}
                Ok(Err(writer_err)) => {
                    debug!("thread-spans writer task error during abort: {writer_err:?}");
                }
                Err(join_err) => {
                    warn!("thread-spans writer task panicked during abort: {join_err:?}");
                }
            }
            Err(e)
        }
    }
}
/// Rebuilds the partition if it's missing or out of date.
///
/// `same_run_ranges` accumulates the insert ranges every partition this `jit_update` run has already
/// handled -- written, or found already up to date -- earlier in its loop. This call reads it (to
/// pass to `retire_partitions` via `RetireMatch::Overlap`, protecting those earlier partitions from
/// this write) and appends `spec`'s own range before returning.
///
/// `pub` only so `thread_spans_ordering_db_test.rs` can write a single JIT partition directly, with
/// exact boundaries `jit_update`'s loop wouldn't expose; `rust/analytics/tests/` compiles as an
/// external crate and can only reach `pub` items. Not intended as API.
/// (`net_spans_view::update_partition` stays private because its tests drive `retire_partitions`
/// directly instead -- there is no Rust producer of `net` streams to push blocks through.)
#[span_fn]
pub async fn update_partition(
    lake: Arc<DataLakeConnection>,
    view_meta: ViewMetadata,
    schema: Arc<Schema>,
    convert_ticks: &ConvertTicks,
    spec: &SourceDataBlocksInMemory,
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
        same_run_ranges.as_slice(),
    )
    .await
    .with_context(|| "write_partition")?;
    same_run_ranges.push(insert_range);

    Ok(())
}

#[async_trait]
impl View for ThreadSpansView {
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
        anyhow::bail!("not implemented")
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![SCHEMA_VERSION]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(get_spans_schema())
    }

    #[span_fn]
    async fn jit_update(
        &self,
        lakehouse: Arc<LakehouseContext>,
        query_range: Option<TimeRange>,
    ) -> Result<()> {
        let Some(query_range) = query_range else {
            anyhow::bail!("query range mandatory for thread spans view");
        };
        let stream = Arc::new(
            find_stream_from_view(
                lakehouse.clone(),
                self.view_factory.clone(),
                &self.stream_id,
                None,
            )
            .await
            .with_context(|| "find_stream_from_view")?,
        );
        let (process, last_block_end_ticks, last_block_end_time) = find_process_with_latest_timing(
            lakehouse.clone(),
            self.view_factory.clone(),
            &stream.process_id,
            None,
        )
        .await
        .with_context(|| "find_process_with_latest_timing")?;
        let process = Arc::new(process);
        let convert_ticks = make_time_converter_from_latest_timing(
            &process,
            last_block_end_ticks,
            last_block_end_time,
        )
        .with_context(|| "make_time_converter_from_latest_timing")?;
        let blocks_view = BlocksView::new()?;
        // ThreadSpansView builds cross-block call trees and declares ScanOrdering::Concatenated
        // over `begin` (see get_scan_output_ordering below), so its JIT partitions must be
        // event-time ordered, not insert-time ordered -- see BlockOrder::EventTime's docs.
        let config = JitPartitionConfig {
            block_order: BlockOrder::EventTime,
            ..Default::default()
        };
        let partitions = generate_stream_jit_partitions(
            &config,
            lakehouse.clone(),
            &blocks_view,
            &query_range,
            stream.clone(),
            process.clone(),
        )
        .await
        .with_context(|| "generate_stream_jit_partitions")?;
        // Accumulates this run's own already-handled insert ranges across the loop, so a later
        // partition's retire step never retires an earlier one from this same run -- see
        // `update_partition`'s and `RetireMatch::Overlap`'s docs.
        let mut same_run_ranges: Vec<TimeRange> = Vec::new();
        for part in &partitions {
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
                col("begin").into(),
                Operator::LtEq,
                Expr::Literal(datetime_to_scalar(end), None).into(),
            )),
            Expr::BinaryExpr(BinaryExpr::new(
                col("end").into(),
                Operator::GtEq,
                Expr::Literal(datetime_to_scalar(begin), None).into(),
            )),
        ])
    }

    fn get_time_bounds(&self) -> Arc<dyn DataFrameTimeBounds> {
        Arc::new(NamedColumnsTimeBounds::new(
            MIN_TIME_COLUMN.clone(),
            MAX_TIME_COLUMN.clone(),
        ))
    }

    fn get_update_group(&self) -> Option<i32> {
        None
    }

    fn get_scan_output_ordering(&self) -> ScanOrdering {
        ScanOrdering::Concatenated {
            columns: vec![ScanSortColumn {
                column: MIN_TIME_COLUMN.clone(),
                descending: false,
            }],
            bounds: OrderingBounds::EventTime,
        }
    }
}
