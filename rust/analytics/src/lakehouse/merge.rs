use super::{
    lakehouse_context::LakehouseContext,
    partition::Partition,
    partition_cache::{PartitionCache, QueryPartitionProvider},
    partition_source_data::hash_to_object_count,
    partitioned_execution_plan::{
        ScanOrdering, assert_ordering_satisfied, assert_single_partition,
    },
    partitioned_table_provider::PartitionedTableProvider,
    query::make_session_context,
    read_scope::CallerContext,
    session_configurator::SessionConfigurator,
    view::{ScanSortColumn, View},
    view_factory::ViewFactory,
    write_partition::{PartitionRowSet, RetireMatch, write_partition_from_rows},
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
use std::time::Instant;
use xxhash_rust::xxh32::xxh32;

/// A merge is a bounded, streaming rewrite of a fixed set of files whose consumer is a single
/// writer task (`create_merged_partition` -> `write_partition_from_rows`), not a query that
/// benefits from scan parallelism. Left at DataFusion's default (`true`), `EnforceDistribution`
/// splits the source scan into `target_partitions` byte-range file groups and `execute_stream`
/// coalesces them, so the reader working set is multiplied by the host's core count for no
/// throughput the writer can absorb. Forcing one sequential file group here fixes that for the
/// `Unordered` and `Concatenated` shapes, which both start from a single file group spanning every
/// input partition: left unsplit, one reader works through that group's files one at a time instead
/// of `target_partitions` concurrent byte-range readers. `PerFile` already builds one file group per
/// input partition, and this setting is load-bearing there too: left at `true`,
/// `repartition_file_groups` (via `repartition_preserving_order`) splits each of those k
/// per-partition groups into `target_partitions` byte-range groups whenever k < `target_partitions`,
/// so this setting is what keeps `PerFile` at one reader per input file for its k-way ordered merge.
/// Downstream parallelism is untouched: a merge query with a GROUP BY still gets its round-robin
/// fan-out above the scan.
pub async fn make_merge_session_context(
    lakehouse: Arc<LakehouseContext>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
    configurator: Arc<dyn SessionConfigurator>,
    caller: CallerContext,
) -> Result<SessionContext> {
    let ctx = make_session_context(
        lakehouse,
        part_provider,
        query_range,
        view_factory,
        configurator,
        caller,
    )
    .await?;
    ctx.state_ref()
        .write()
        .config_mut()
        .options_mut()
        .optimizer
        .repartition_file_scans = false;
    Ok(ctx)
}

/// The outcome of running a merge query.
pub struct MergeQueryResult {
    /// The merged rows.
    pub stream: SendableRecordBatchStream,
    /// Whether the merger's declared scan ordering (if any) was honored by the physical plan
    /// without falling back to a buffering `Sort` node. Always `true` for the concatenate
    /// strategy's undeclared shape (`ScanOrdering::Unordered`) -- there is no declared ordering to
    /// defeat -- and only ever computed dynamically for the two strategies that declare one. This
    /// drives only a memory-regression warning; it never gates the recorded `sort_order` (see
    /// `View::get_merged_partition_sort_order`).
    ///
    /// For the sort-merge strategy (`ScanOrdering::PerFile`), a surviving `SortPreservingMergeExec`
    /// is the *expected* operator, not a regression -- only a surviving `SortExec` means the
    /// streaming shape was lost. For the concatenate strategy's declared shape
    /// (`ScanOrdering::Concatenated`), either operator surviving is a regression.
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
    merge_scan_ordering: ScanOrdering,
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
            merge_scan_ordering: ScanOrdering::Unordered,
        }
    }

    /// Declares an ordering the merge's source scan already satisfies (see
    /// `PartitionedTableProvider::with_scan_ordering`), letting DataFusion elide the merge query's
    /// `Sort` node instead of buffering. Default: `Unordered` (matching today's behavior for every
    /// existing caller).
    pub fn with_merge_scan_ordering(mut self, ordering: ScanOrdering) -> Self {
        self.merge_scan_ordering = ordering;
        self
    }

    /// The concatenating merge path: one sequential reader over one file group, covering both
    /// `ScanOrdering::Unordered` and `ScanOrdering::Concatenated` -- they build and execute the
    /// identical scan (Design §2 of `tasks/1491_merge_scan_memory_plan.md`), differing only in
    /// whether `declared` is `true`, i.e. whether the resulting order is declared to DataFusion.
    /// `repartition_file_scans = false` is applied to every merge by the caller's
    /// `make_merge_session_context` session, not here; this method just builds the physical plan
    /// once, inspects it, and executes that exact plan -- never planning or building twice.
    /// `declared` gates `assert_single_partition` and the `SortExec`/`SortPreservingMergeExec`
    /// plan-string check, both of which exist to protect a declared ordering: with nothing
    /// declared there is nothing to protect, and `ordering_honored` is unconditionally `true`,
    /// matching `MergeQueryResult::ordering_honored`'s documented default for an undeclared
    /// ordering.
    async fn execute_concatenated_merge(
        &self,
        ctx: &SessionContext,
        declared: bool,
        insert_range: TimeRange,
    ) -> Result<MergeQueryResult> {
        let df = ctx.sql(&self.query).await?;
        let task_ctx = Arc::new(df.task_ctx());
        let plan = df
            .create_physical_plan()
            .await
            .with_context(|| "creating physical plan for merge query")?;

        if declared {
            assert_single_partition(
                &plan,
                &format!("merge query {:?}", self.query),
                insert_range,
                "executing it would coalesce partitions and destroy the declared ordering. This \
                 likely means repartition_file_scans did not take effect.",
            )?;
        }

        let ordering_honored = if declared {
            let plan_str = displayable(plan.as_ref()).indent(true).to_string();
            let honored =
                !plan_str.contains("SortExec") && !plan_str.contains("SortPreservingMergeExec");
            if !honored {
                warn!(
                    "merge query {:?} (insert_range=[{}, {}]) did not elide its declared \
                     ordering -- the merge will still produce a correctly ordered result, but \
                     it will buffer in memory instead of streaming. Plan:\n{plan_str}",
                    self.query,
                    insert_range.begin.to_rfc3339(),
                    insert_range.end.to_rfc3339()
                );
            }
            honored
        } else {
            true
        };

        let stream =
            execute_stream(plan, task_ctx).with_context(|| "executing merge query plan")?;
        Ok(MergeQueryResult {
            stream,
            ordering_honored,
        })
    }

    /// The `PerFile` k-way merge path (Design §2 of
    /// `tasks/completed/1392_kway_merge_sorted_partitions_plan.md`): four optimizer settings keep the merge
    /// a bounded-memory streaming k-way merge instead of fanning out to `target_partitions` --
    /// `repartition_file_scans = false` is the fifth, applied to every merge by the caller's
    /// `make_merge_session_context` session rather than set here (Design §1 of
    /// `tasks/1491_merge_scan_memory_plan.md`) -- an unconditional logical-plan `DataFrame::sort`
    /// makes the declared ordering a property of the query plan rather than of an optimizer
    /// preference (so a wrong or missing merge-query sort is not representable), and three checks
    /// -- two hard, one warn-only -- run against the physical plan before it is executed.
    async fn execute_sorted_merge(
        &self,
        ctx: &SessionContext,
        columns: &[ScanSortColumn],
        insert_range: TimeRange,
    ) -> Result<MergeQueryResult> {
        {
            let state = ctx.state_ref();
            let mut state = state.write();
            let optimizer = &mut state.config_mut().options_mut().optimizer;
            optimizer.enable_round_robin_repartition = false;
            optimizer.repartition_aggregations = false;
            optimizer.prefer_existing_sort = true;
            optimizer.repartition_joins = false;
        }

        let df = ctx.sql(&self.query).await?;
        let df = df.sort(
            columns
                .iter()
                .map(|c| {
                    Expr::Column(datafusion::common::Column::new_unqualified(
                        c.column.as_str(),
                    ))
                    .sort(!c.descending, c.descending)
                })
                .collect(),
        )?;
        let task_ctx = Arc::new(df.task_ctx());
        let plan = df
            .create_physical_plan()
            .await
            .with_context(|| "creating physical plan for per-file merge query")?;

        // Check 1 (hard): execute_stream would otherwise coalesce partitions and destroy order.
        assert_single_partition(
            &plan,
            &format!("merge query {:?}", self.query),
            insert_range,
            "executing it would coalesce partitions and destroy the declared ordering. This \
             likely means repartition_aggregations or enable_round_robin_repartition did not \
             take effect, or the merge query is missing a collapsing operator (e.g. a \
             SortPreservingMergeExec) to bring the per-file partitions back to one.",
        )?;

        // Check 2 (hard): the plan's output ordering must satisfy the declared columns -- a
        // defensive assertion that the sort_order about to be recorded is truthful. This has no
        // author-reachable failure path under this design (the sort above is applied
        // unconditionally): it is a backstop against a future DataFusion regression, not against
        // author error.
        assert_ordering_satisfied(
            &plan,
            columns,
            "per-file merge",
            &format!("merge query {:?}", self.query),
            insert_range,
            &format!(
                "the declared per-file merge columns {columns:?}; refusing to record a false \
                 sort_order guarantee."
            ),
        )?;

        // Check 3 (warn-only): a SortPreservingMergeExec is the *expected* operator here -- only
        // a surviving SortExec signals the streaming shape regressed (memory, not correctness --
        // check 2 already proved the output order).
        let plan_str = displayable(plan.as_ref()).indent(true).to_string();
        let ordering_honored = !plan_str.contains("SortExec");
        if !ordering_honored {
            warn!(
                "merge query {:?} (insert_range=[{}, {}]) did not elide its declared per-file \
                 ordering -- the merge will still produce a correctly ordered result, but it will \
                 buffer in memory instead of streaming. Plan:\n{plan_str}",
                self.query,
                insert_range.begin.to_rfc3339(),
                insert_range.end.to_rfc3339()
            );
        }

        let stream = execute_stream(plan, task_ctx)
            .with_context(|| "executing per-file merge query plan")?;
        Ok(MergeQueryResult {
            stream,
            ordering_honored,
        })
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
        let ctx = make_merge_session_context(
            lakehouse.clone(),
            partitions_all_views,
            Some(insert_range),
            self.view_factory.clone(),
            self.session_configurator.clone(),
            CallerContext::maintenance(),
        )
        .await?;
        let src_table = PartitionedTableProvider::with_scan_ordering(
            self.file_schema.clone(),
            reader_factory,
            partitions_to_merge,
            self.merge_scan_ordering.clone(),
        );
        ctx.register_table(
            TableReference::Bare {
                table: "source".into(),
            },
            Arc::new(src_table),
        )?;

        match &self.merge_scan_ordering {
            // Sort-merge: k ordered readers, collapsed by a SortPreservingMergeExec.
            ScanOrdering::PerFile { columns } => {
                self.execute_sorted_merge(&ctx, columns, insert_range).await
            }
            // Concatenate: one sequential reader over one file group. `Unordered` and
            // `Concatenated` are the same strategy -- they differ only in whether the resulting
            // order is declared, which is what gates the checks inside. Spelled out rather than a
            // wildcard so a future `ScanOrdering` variant fails to compile here instead of
            // silently taking this arm.
            other @ (ScanOrdering::Unordered | ScanOrdering::Concatenated { .. }) => {
                self.execute_concatenated_merge(
                    &ctx,
                    other.declares_concatenated_ordering(),
                    insert_range,
                )
                .await
            }
        }
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
    let merge_start = Instant::now();
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
    // A defeated ordering elision (ordering_honored: false) is already warned about, with the
    // offending plan, inside QueryMerger::execute_merge_query.
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
        RetireMatch::Containment,
        Vec::new(),
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
            tx.send(Ok(PartitionRowSet::new(event_time_range, rb, None)))
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
            logger
                .write_log_entry(format!(
                    "{desc}: merge completed in {:.3}s",
                    merge_start.elapsed().as_secs_f64()
                ))
                .await
                .with_context(|| "write_log_entry")?;
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
