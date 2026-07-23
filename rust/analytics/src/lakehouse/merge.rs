use super::{
    lakehouse_context::LakehouseContext,
    partition::Partition,
    partition_cache::PartitionCache,
    partition_source_data::hash_to_object_count,
    partitioned_execution_plan::OrderingBounds,
    partitioned_table_provider::PartitionedTableProvider,
    query::make_session_context,
    session_configurator::SessionConfigurator,
    view::{ScanSortColumn, View},
    view_factory::ViewFactory,
    write_partition::{PartitionRowSet, write_partition_from_rows},
};
use crate::{response_writer::Logger, time::TimeRange};
use anyhow::{Context, Result};
use async_trait::async_trait;
use datafusion::{
    arrow::datatypes::Schema,
    execution::SendableRecordBatchStream,
    physical_plan::{displayable, execute_stream},
    prelude::*,
    sql::TableReference,
};
use futures::stream::StreamExt;
use micromegas_tracing::prelude::*;
use std::fmt::Debug;
use std::sync::Arc;
use xxhash_rust::xxh32::xxh32;

/// The outcome of running a merge query.
pub struct MergeQueryResult {
    /// The merged rows.
    pub stream: SendableRecordBatchStream,
    /// Whether the merger's declared scan ordering (if any) was honored by the physical plan
    /// without falling back to a buffering `Sort`/`SortPreservingMerge` node. Always `true` when
    /// no ordering was declared to DataFusion in the first place -- it is only ever computed
    /// dynamically by an ordering-declaring `QueryMerger`. This drives only a memory-regression
    /// warning; it never gates the recorded `sort_order` (see `View::get_merged_partition_sort_order`).
    pub ordering_honored: bool,
}

/// A trait for merging partitions.
#[async_trait]
pub trait PartitionMerger: Send + Sync + Debug {
    /// Executes the merge query.
    async fn execute_merge_query(
        &self,
        lakehouse: Arc<LakehouseContext>,
        partitions_to_merge: Arc<Vec<Partition>>,
        partitions_all_views: Arc<PartitionCache>,
        insert_range: TimeRange,
    ) -> Result<MergeQueryResult>;
}

/// A `PartitionMerger` that executes a SQL query to merge partitions.
#[derive(Debug)]
pub struct QueryMerger {
    view_factory: Arc<ViewFactory>,
    session_configurator: Arc<dyn SessionConfigurator>,
    file_schema: Arc<Schema>,
    query: Arc<String>,
    merge_scan_ordering: Vec<ScanSortColumn>,
}

impl QueryMerger {
    pub fn new(
        view_factory: Arc<ViewFactory>,
        session_configurator: Arc<dyn SessionConfigurator>,
        file_schema: Arc<Schema>,
        query: Arc<String>,
    ) -> Self {
        Self {
            view_factory,
            session_configurator,
            file_schema,
            query,
            merge_scan_ordering: vec![],
        }
    }

    /// Declares an ordering the merge's source scan already satisfies (see
    /// `PartitionedTableProvider::with_ordering`), letting DataFusion elide the merge query's
    /// `Sort` node instead of buffering. Default: empty (no declared ordering, matching today's
    /// behavior for every existing caller).
    pub fn with_merge_scan_ordering(mut self, ordering: Vec<ScanSortColumn>) -> Self {
        self.merge_scan_ordering = ordering;
        self
    }
}

#[async_trait]
impl PartitionMerger for QueryMerger {
    async fn execute_merge_query(
        &self,
        lakehouse: Arc<LakehouseContext>,
        partitions_to_merge: Arc<Vec<Partition>>,
        partitions_all_views: Arc<PartitionCache>,
        insert_range: TimeRange,
    ) -> Result<MergeQueryResult> {
        let reader_factory = lakehouse.reader_factory().clone();
        let ctx = make_session_context(
            lakehouse.clone(),
            partitions_all_views,
            Some(insert_range),
            self.view_factory.clone(),
            self.session_configurator.clone(),
        )
        .await?;
        let src_table = PartitionedTableProvider::with_ordering(
            self.file_schema.clone(),
            reader_factory,
            partitions_to_merge,
            self.merge_scan_ordering.clone(),
            OrderingBounds::InsertTime,
        );
        ctx.register_table(
            TableReference::Bare {
                table: "source".into(),
            },
            Arc::new(src_table),
        )?;

        if self.merge_scan_ordering.is_empty() {
            let stream = ctx
                .sql(&self.query)
                .await?
                .execute_stream()
                .await
                .with_context(|| "merged_df.execute_stream")?;
            return Ok(MergeQueryResult {
                stream,
                ordering_honored: true,
            });
        }

        // Ordering-declared merge: force the source scan into a single sequential file group
        // (Design §1 point 3) so the declared ordering can be elided instead of re-sorted, then
        // build the physical plan once, inspect it, and execute that exact plan -- never
        // planning or building twice.
        ctx.state_ref()
            .write()
            .config_mut()
            .options_mut()
            .optimizer
            .repartition_file_scans = false;

        let df = ctx.sql(&self.query).await?;
        let task_ctx = Arc::new(df.task_ctx());
        let plan = df
            .create_physical_plan()
            .await
            .with_context(|| "creating physical plan for merge query")?;

        let partition_count = plan.properties().output_partitioning().partition_count();
        if partition_count != 1 {
            anyhow::bail!(
                "merge query {:?} (insert_range=[{}, {}]) produced a {partition_count}-partition \
                 physical plan; execute_stream requires a single-partition plan. This likely means \
                 repartition_file_scans did not take effect.",
                self.query,
                insert_range.begin.to_rfc3339(),
                insert_range.end.to_rfc3339()
            );
        }

        let plan_str = displayable(plan.as_ref()).indent(true).to_string();
        let ordering_honored =
            !plan_str.contains("SortExec") && !plan_str.contains("SortPreservingMergeExec");
        if !ordering_honored {
            warn!(
                "merge query {:?} (insert_range=[{}, {}]) did not elide its declared ordering -- \
                 the merge will still produce a correctly ordered result, but it will buffer in \
                 memory instead of streaming. Plan:\n{plan_str}",
                self.query,
                insert_range.begin.to_rfc3339(),
                insert_range.end.to_rfc3339()
            );
        }

        let stream =
            execute_stream(plan, task_ctx).with_context(|| "executing merge query plan")?;
        Ok(MergeQueryResult {
            stream,
            ordering_honored,
        })
    }
}

fn partition_set_stats(
    view: Arc<dyn View>,
    filtered_partitions: &[Partition],
) -> Result<(i64, i64)> {
    let mut sum_size: i64 = 0;
    let mut source_hash: i64 = 0;
    let latest_file_schema_hash = view.get_file_schema_hash();
    for p in filtered_partitions {
        // for some time all the hashes will actually be the number of events in the source data
        // when views have different hash algos, we should delegate to the view the creation of the merged hash
        source_hash = if p.source_data_hash.len() == std::mem::size_of::<i64>() {
            source_hash + hash_to_object_count(&p.source_data_hash)?
        } else {
            //previous hash algo
            xxh32(&p.source_data_hash, source_hash as u32).into()
        };

        sum_size += p.file_size;

        if p.view_metadata.file_schema_hash != latest_file_schema_hash {
            anyhow::bail!(
                "incompatible file schema with [{},{}]",
                p.begin_insert_time().to_rfc3339(),
                p.end_insert_time().to_rfc3339()
            );
        }
    }
    Ok((sum_size, source_hash))
}

/// Creates a merged partition from a set of existing partitions.
pub async fn create_merged_partition(
    partitions_to_merge: Arc<PartitionCache>,
    partitions_all_views: Arc<PartitionCache>,
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    insert_range: TimeRange,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let view_set_name = &view.get_view_set_name();
    let view_instance_id = &view.get_view_instance_id();
    let desc = format!(
        "[{}, {}] {view_set_name} {view_instance_id}",
        insert_range.begin.to_rfc3339(),
        insert_range.end.to_rfc3339()
    );
    // we are not looking for intersecting partitions, but only those that fit completely in the range
    // otherwise we'd get duplicated records
    let mut filtered_partitions = partitions_to_merge
        .filter_inside_range(view_set_name, view_instance_id, insert_range)
        .partitions;
    if filtered_partitions.len() != partitions_to_merge.len() {
        warn!("partitions_to_merge was not filtered properly");
    }
    if filtered_partitions.len() < 2 {
        logger
            .write_log_entry(format!("{desc}: not enough partitions to merge"))
            .await
            .with_context(|| "writing log")?;
        return Ok(());
    }
    let (sum_size, source_hash) = partition_set_stats(view.clone(), &filtered_partitions)
        .with_context(|| "partition_set_stats")?;
    logger
        .write_log_entry(format!(
            "{desc}: merging {} partitions sum_size={sum_size}",
            filtered_partitions.len()
        ))
        .await
        .with_context(|| "write_log_entry")?;
    filtered_partitions.sort_by_key(|p| p.begin_insert_time());
    // Computed before merge_partitions runs: a pure function of the input slice alone (Design §4).
    let merged_sort_order = view.get_merged_partition_sort_order(&filtered_partitions);
    let merge_result = view
        .merge_partitions(
            lakehouse.clone(),
            Arc::new(filtered_partitions),
            partitions_all_views,
            insert_range,
        )
        .await
        .with_context(|| "view.merge_partitions")?;
    if !merge_result.ordering_honored {
        warn!(
            "{desc}: merge did not honor its declared scan ordering; memory bound not honored for this merge (result is still correctly ordered)"
        );
    }
    let mut merged_stream = merge_result.stream;
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let view_copy = view.clone();
    let lake = lakehouse.lake().clone();
    let join_handle = spawn_with_context(write_partition_from_rows(
        lake,
        view_copy.get_meta(),
        view_copy.get_file_schema(),
        insert_range,
        source_hash.to_le_bytes().to_vec(),
        merged_sort_order,
        false,
        rx,
        logger.clone(),
    ));
    let compute_time_bounds = view.get_time_bounds();
    let ctx =
        SessionContext::new_with_config_rt(SessionConfig::default(), lakehouse.runtime().clone());
    let stream_result: Result<()> = async {
        while let Some(rb_res) = merged_stream.next().await {
            let rb = rb_res.with_context(|| "receiving record_batch from stream")?;
            let event_time_range = compute_time_bounds
                .get_time_bounds(ctx.read_batch(rb.clone()).with_context(|| "read_batch")?)
                .await?;
            tx.send(Ok(PartitionRowSet::new(event_time_range, rb)))
                .await
                .with_context(|| "sending partition row set")?;
        }
        Ok(())
    }
    .await;

    match stream_result {
        Ok(()) => {
            drop(tx);
            join_handle.await??;
            Ok(())
        }
        Err(e) => {
            warn!("aborting merge partition write for {desc}: {e:?}");
            let _ = tx.send(Err(anyhow::anyhow!("merge stream aborted"))).await;
            drop(tx);
            match join_handle.await {
                Ok(Ok(())) => {}
                Ok(Err(writer_err)) => {
                    debug!("merge writer task error during abort: {writer_err:?}");
                }
                Err(join_err) => {
                    warn!("merge writer task panicked during abort: {join_err:?}");
                }
            }
            Err(e)
        }
    }
}
