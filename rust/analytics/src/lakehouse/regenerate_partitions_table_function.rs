use super::batch_update::regenerate_partition_range;
use super::lakehouse_context::LakehouseContext;
use super::partition_cache::PartitionCache;
use super::view_factory::ViewFactory;
use crate::dfext::expressions::exp_to_i64;
use crate::dfext::expressions::exp_to_string;
use crate::dfext::expressions::exp_to_timestamp;
use crate::dfext::log_stream_table_provider::LogStreamTableProvider;
use crate::dfext::task_log_exec_plan::TaskLogExecPlan;
use crate::response_writer::LogSender;
use crate::response_writer::Logger;
use crate::time::TimeRange;
use anyhow::Context;
use chrono::TimeDelta;
use datafusion::catalog::TableFunctionArgs;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::common::plan_err;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// A DataFusion `TableFunctionImpl` for force-regenerating lakehouse partitions directly from
/// source data, bypassing the "already up to date" freshness check `materialize_partitions` stops
/// at. See `tasks/blocks_view_ordered_merges_plan.md`'s Design §3.
#[derive(Debug)]
pub struct RegeneratePartitionsTableFunction {
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
}

impl RegeneratePartitionsTableFunction {
    pub fn new(lakehouse: Arc<LakehouseContext>, view_factory: Arc<ViewFactory>) -> Self {
        Self {
            lakehouse,
            view_factory,
        }
    }
}

#[span_fn]
async fn regenerate_partitions_impl(
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    view_name: &str,
    insert_range: TimeRange,
    partition_time_delta: TimeDelta,
    logger: Arc<dyn Logger>,
) -> anyhow::Result<()> {
    let view = view_factory
        .get_global_view(view_name)
        .with_context(|| format!("can't find view {view_name}"))?;

    let existing_partitions_all_views = Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lakehouse.lake().db_pool, insert_range)
            .await?,
    );

    regenerate_partition_range(
        existing_partitions_all_views,
        lakehouse,
        view,
        insert_range,
        partition_time_delta,
        logger,
    )
    .await?;
    Ok(())
}

impl TableFunctionImpl for RegeneratePartitionsTableFunction {
    fn call_with_args(
        &self,
        args: TableFunctionArgs,
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        let args = args.exprs();
        // an alternative would be to use coerce & create_physical_expr
        let Some(view_set_name) = args.first().map(exp_to_string).transpose()? else {
            return plan_err!("Missing first argument, expected view_set_name: String");
        };
        let Some(begin) = args.get(1).map(exp_to_timestamp).transpose()? else {
            return plan_err!("Missing 2nd argument, expected a UTC nanoseconds timestamp");
        };
        let Some(end) = args.get(2).map(exp_to_timestamp).transpose()? else {
            return plan_err!("Missing 3rd argument, expected a UTC nanoseconds timestamp");
        };
        let Some(delta) = args.get(3).map(exp_to_i64).transpose()? else {
            return plan_err!("Missing 4th argument, expected a number of seconds(i64)");
        };

        let lakehouse = self.lakehouse.clone();
        let view_factory = self.view_factory.clone();

        let spawner = move || {
            let (tx, rx) = tokio::sync::mpsc::channel(100);
            // Keep a clone of the raw sender alongside the LogSender wrapping it: ordinary
            // progress lines flow through the logger as Ok((time, msg)), matching
            // materialize_partitions's behavior, but a regenerate_partition_range failure must
            // surface as a query-level error, not just one more log row -- so it is sent as a
            // single Err item through the raw sender instead of only being logged.
            let raw_tx = tx.clone();
            let logger = Arc::new(LogSender::new(tx));
            spawn_with_context(async move {
                if let Err(e) = regenerate_partitions_impl(
                    lakehouse,
                    view_factory,
                    &view_set_name,
                    TimeRange::new(begin, end),
                    TimeDelta::seconds(delta),
                    logger.clone(),
                )
                .await
                .with_context(|| "regenerate_partitions_impl")
                {
                    let msg = format!("{e:?}");
                    error!("{msg}");
                    let _ = raw_tx.send(Err(msg)).await;
                }
            });
            rx
        };

        Ok(Arc::new(LogStreamTableProvider {
            log_stream: Arc::new(TaskLogExecPlan::new(Box::new(spawner))),
        }))
    }
}
