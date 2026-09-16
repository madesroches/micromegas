use super::{
    dataframe_time_bounds::DataFrameTimeBounds,
    partitioned_execution_plan::{assert_ordering_satisfied, assert_single_partition},
    view::{PartitionSpec, ScanSortColumn, ViewMetadata},
    write_partition::write_partition_from_rows,
};
use crate::{
    dfext::typed_column::typed_column_by_name,
    lakehouse::write_partition::{PartitionRowSet, RetireMatch},
    record_batch_transformer::RecordBatchTransformer,
    response_writer::Logger,
    time::TimeRange,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use datafusion::{
    arrow::{
        array::{Int64Array, RecordBatch},
        datatypes::Schema,
    },
    execution::{SendableRecordBatchStream, TaskContext},
    physical_plan::{ExecutionPlan, execute_stream},
    prelude::*,
};
use futures::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// A `PartitionSpec` implementation for SQL-defined partitions.
pub struct SqlPartitionSpec {
    ctx: SessionContext,
    transformer: Arc<dyn RecordBatchTransformer>,
    compute_time_bounds: Arc<dyn DataFrameTimeBounds>,
    schema: Arc<Schema>,
    extract_query: String,
    view_metadata: ViewMetadata,
    insert_range: TimeRange,
    record_count: i64,
    /// The sort guarantee to record on the fresh partition this extract query writes, if any (see
    /// `SqlBatchView::with_merge_sort_order`). When set, `write` applies it to the extract
    /// query's `DataFrame` as a `DataFrame::sort` before executing it (see
    /// `plan_sorted_extract`), so the guarantee it records always matches what was actually
    /// written.
    sort_order: Option<Vec<String>>,
}

impl SqlPartitionSpec {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        ctx: SessionContext,
        transformer: Arc<dyn RecordBatchTransformer>,
        compute_time_bounds: Arc<dyn DataFrameTimeBounds>,
        schema: Arc<Schema>,
        extract_query: String,
        view_metadata: ViewMetadata,
        insert_range: TimeRange,
        record_count: i64,
        sort_order: Option<Vec<String>>,
    ) -> Self {
        Self {
            ctx,
            transformer,
            compute_time_bounds,
            schema,
            extract_query,
            view_metadata,
            insert_range,
            record_count,
            sort_order,
        }
    }

    /// Converts the declared `sort_order` (if any) to `ScanSortColumn`s and delegates to
    /// `plan_sorted_extract`, which applies it to the extract query's `DataFrame` before building
    /// the physical plan -- the same apply-then-plan discipline as
    /// `QueryMerger::execute_sorted_merge` -- then executes the plan it returns. The undeclared
    /// path (`sort_order: None`) plans and executes the query unsorted.
    async fn execute_extract_query(&self, df: DataFrame) -> Result<SendableRecordBatchStream> {
        let columns: Option<Vec<ScanSortColumn>> = self.sort_order.as_ref().map(|sort_order| {
            sort_order
                .iter()
                .map(|c| ScanSortColumn {
                    column: Arc::new(c.clone()),
                    descending: false,
                })
                .collect()
        });
        let subject = format!("extract query for {}", self.view_metadata.view_set_name);
        let (plan, task_ctx) =
            plan_sorted_extract(df, columns.as_deref(), &subject, self.insert_range).await?;
        execute_stream(plan, task_ctx).with_context(|| "executing extract query plan")
    }
}

/// Applies `sort_order` (if declared) to `df` as a `DataFrame::sort` over its `ScanSortColumn`s,
/// builds the physical plan, and -- only when a sort order was declared -- runs
/// `assert_single_partition` and `assert_ordering_satisfied` against it, mirroring
/// `QueryMerger::execute_sorted_merge`'s apply-then-plan sequence (`merge.rs:227-270`; its four
/// merge-specific optimizer settings have no counterpart here). With `sort_order: None` the plan
/// is built unsorted and neither assertion runs. `subject` names the query for the assertions'
/// error messages (e.g. `format!("extract query for {view}")`); `insert_range` is the range being
/// written. Returns the physical plan and the `TaskContext` the caller needs to execute it --
/// callers are left to `execute_stream` it themselves, since only they know what to do with the
/// resulting stream.
pub async fn plan_sorted_extract(
    df: DataFrame,
    sort_order: Option<&[ScanSortColumn]>,
    subject: &str,
    insert_range: TimeRange,
) -> Result<(Arc<dyn ExecutionPlan>, Arc<TaskContext>)> {
    let Some(columns) = sort_order else {
        let task_ctx = Arc::new(df.task_ctx());
        let plan = df
            .create_physical_plan()
            .await
            .with_context(|| "creating physical plan for extract query")?;
        return Ok((plan, task_ctx));
    };

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
        .with_context(|| "creating physical plan for extract query")?;

    assert_single_partition(
        &plan,
        subject,
        insert_range,
        "a declared sort_order requires a single-partition, globally-ordered output.",
    )?;

    assert_ordering_satisfied(
        &plan,
        columns,
        "extract-query",
        subject,
        insert_range,
        &format!(
            "the declared sort_order {columns:?} even after applying it as a DataFrame::sort; \
             this indicates the physical plan silently dropped or reordered the applied sort, \
             not an author mistake."
        ),
    )?;

    Ok((plan, task_ctx))
}

impl std::fmt::Debug for SqlPartitionSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SqlPartitionSpec")
    }
}

#[async_trait]
impl PartitionSpec for SqlPartitionSpec {
    fn is_empty(&self) -> bool {
        self.record_count < 1
    }

    fn get_source_data_hash(&self) -> Vec<u8> {
        self.record_count.to_le_bytes().to_vec()
    }

    async fn write(&self, lake: Arc<DataLakeConnection>, logger: Arc<dyn Logger>) -> Result<()> {
        // Allow empty record_count - write_partition_from_rows will create
        // an empty partition record if no data is sent through the channel
        let desc = format!(
            "[{}, {}] {} {}",
            self.view_metadata.view_set_name,
            self.view_metadata.view_instance_id,
            self.insert_range.begin.to_rfc3339(),
            self.insert_range.end.to_rfc3339()
        );
        logger.write_log_entry(format!("writing {desc}")).await?;
        let df = self.ctx.sql(&self.extract_query).await?;
        let mut stream = self.execute_extract_query(df).await?;

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let join_handle = spawn_with_context(write_partition_from_rows(
            lake.clone(),
            self.view_metadata.clone(),
            self.schema.clone(),
            self.insert_range,
            self.get_source_data_hash(),
            self.sort_order.clone(),
            RetireMatch::Containment,
            Vec::new(),
            rx,
            logger.clone(),
        ));

        while let Some(rb_res) = stream.next().await {
            let rb = self.transformer.transform(rb_res?).await?;
            let event_time_range = self
                .compute_time_bounds
                .get_time_bounds(self.ctx.read_batch(rb.clone())?)
                .await?;
            tx.send(Ok(PartitionRowSet::new(event_time_range, rb, None)))
                .await?;
        }
        drop(tx);
        join_handle.await??;
        Ok(())
    }
}

/// Fetches a `SqlPartitionSpec` by executing a count query and an extract query. `sort_order`
/// declares the ordering the resulting partition's rows are guaranteed to satisfy (see
/// `SqlBatchView::with_merge_sort_order`); pass `None` for views that make no such guarantee.
#[expect(clippy::too_many_arguments)]
pub async fn fetch_sql_partition_spec(
    ctx: SessionContext,
    transformer: Arc<dyn RecordBatchTransformer>,
    compute_time_bounds: Arc<dyn DataFrameTimeBounds>,
    schema: Arc<Schema>,
    count_src_sql: String,
    extract_query: String,
    view_metadata: ViewMetadata,
    insert_range: TimeRange,
    sort_order: Option<Vec<String>>,
) -> Result<SqlPartitionSpec> {
    let df = ctx.sql(&count_src_sql).await?;
    let batches: Vec<RecordBatch> = df.collect().await?;
    if batches.len() != 1 {
        anyhow::bail!("fetch_sql_partition_spec: query should return a single batch");
    }
    let rb = &batches[0];
    let count_column: &Int64Array = typed_column_by_name(rb, "count")?;
    if count_column.len() != 1 {
        anyhow::bail!("fetch_sql_partition_spec: query should return a single row");
    }
    let count = count_column.value(0);
    if count > 0 {
        trace!(
            "fetch_sql_partition_spec for view {}, count={count}",
            &*view_metadata.view_set_name
        );
    }
    Ok(SqlPartitionSpec::new(
        ctx,
        transformer,
        compute_time_bounds,
        schema,
        extract_query,
        view_metadata,
        insert_range,
        count,
        sort_order,
    ))
}
