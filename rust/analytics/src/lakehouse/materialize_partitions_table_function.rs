use super::batch_update::materialize_partition_range;
use super::partition_cache::PartitionCache;
use super::view_factory::ViewFactory;
use crate::dfext::expressions::exp_to_i64;
use crate::dfext::expressions::exp_to_string;
use crate::dfext::expressions::exp_to_timestamp;
use crate::dfext::log_stream_table_provider::LogStreamTableProvider;
use crate::dfext::task_log_exec_plan::TaskLogExecPlan;
use crate::response_writer::LogSender;
use crate::response_writer::Logger;
use anyhow::Context;
use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::common::plan_err;
use datafusion::prelude::Expr;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::error;
use std::sync::Arc;

#[derive(Debug)]
pub struct MaterializePartitionsTableFunction {
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
}

impl MaterializePartitionsTableFunction {
    pub fn new(lake: Arc<DataLakeConnection>, view_factory: Arc<ViewFactory>) -> Self {
        Self { lake, view_factory }
    }
}

#[allow(clippy::too_many_arguments)]
async fn materialize_partitions_impl(
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
    view_name: &str,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    partition_time_delta: TimeDelta,
    logger: Arc<dyn Logger>,
) -> anyhow::Result<()> {
    let view = view_factory
        .get_global_view(view_name)
        .with_context(|| format!("can't find view {view_name}"))?;

    let existing_partitions =
        Arc::new(PartitionCache::fetch_overlapping_insert_range(&lake.db_pool, begin, end).await?);

    materialize_partition_range(
        existing_partitions,
        lake.clone(),
        view,
        begin,
        end,
        partition_time_delta,
        logger,
    )
    .await?;
    Ok(())
}

impl TableFunctionImpl for MaterializePartitionsTableFunction {
    fn call(&self, args: &[Expr]) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        // an alternative would be to use coerce & create_physical_expr
        let Some(view_set_name) = args.first().map(exp_to_string).transpose()? else {
            return plan_err!("Missing first argument, expected view_set_name: String");
        };
        let Some(begin) = args.get(1).map(exp_to_timestamp).transpose()? else {
            return plan_err!("Missing 3rd argument, expected a UTC nanoseconds timestamp");
        };
        let Some(end) = args.get(2).map(exp_to_timestamp).transpose()? else {
            return plan_err!("Missing 4th argument, expected a UTC nanoseconds timestamp");
        };
        let Some(delta) = args.get(3).map(exp_to_i64).transpose()? else {
            return plan_err!("Missing 5th argument, expected a number of seconds(i64)");
        };

        let lake = self.lake.clone();
        let view_factory = self.view_factory.clone();

        let spawner = move || {
            let (tx, rx) = tokio::sync::mpsc::channel(100);
            let logger = Arc::new(LogSender::new(tx));
            tokio::spawn(async move {
                if let Err(e) = materialize_partitions_impl(
                    lake,
                    view_factory,
                    &view_set_name,
                    begin,
                    end,
                    TimeDelta::seconds(delta),
                    logger,
                )
                .await
                .with_context(|| "materialize_partitions_impl")
                {
                    error!("{e:?}");
                }
            });
            rx
        };

        Ok(Arc::new(LogStreamTableProvider {
            log_stream: Arc::new(TaskLogExecPlan::new(Box::new(spawner))),
        }))
    }
}
