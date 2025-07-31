use anyhow::Context;
use chrono::DateTime;
use chrono::Utc;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::common::plan_err;
use datafusion::prelude::Expr;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::error;
use std::sync::Arc;

use crate::dfext::expressions::exp_to_string;
use crate::dfext::expressions::exp_to_timestamp;
use crate::dfext::log_stream_table_provider::LogStreamTableProvider;
use crate::dfext::task_log_exec_plan::TaskLogExecPlan;
use crate::response_writer::LogSender;
use crate::response_writer::Logger;

use super::write_partition::retire_partitions;

/// A DataFusion `TableFunctionImpl` for retiring lakehouse partitions.
#[derive(Debug)]
pub struct RetirePartitionsTableFunction {
    lake: Arc<DataLakeConnection>,
}

impl RetirePartitionsTableFunction {
    pub fn new(lake: Arc<DataLakeConnection>) -> Self {
        Self { lake }
    }
}

async fn retire_partitions_impl(
    lake: Arc<DataLakeConnection>,
    view_set_name: &str,
    view_instance_id: &str,
    begin_insert_time: DateTime<Utc>,
    end_insert_time: DateTime<Utc>,
    logger: Arc<dyn Logger>,
) -> anyhow::Result<()> {
    let mut tr = lake.db_pool.begin().await?;
    retire_partitions(
        &mut tr,
        view_set_name,
        view_instance_id,
        begin_insert_time,
        end_insert_time,
        logger,
    )
    .await?;
    tr.commit().await.with_context(|| "commit")?;
    Ok(())
}

impl TableFunctionImpl for RetirePartitionsTableFunction {
    fn call(&self, args: &[Expr]) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        // an alternative would be to use coerce & create_physical_expr
        let Some(view_set_name) = args.first().map(exp_to_string).transpose()? else {
            return plan_err!("Missing first argument, expected view_set_name: String");
        };
        let Some(view_instance_id) = args.get(1).map(exp_to_string).transpose()? else {
            return plan_err!("Missing 2nd argument, expected view_instance_id: String");
        };
        let Some(begin) = args.get(2).map(exp_to_timestamp).transpose()? else {
            return plan_err!("Missing 3rd argument, expected a UTC nanoseconds timestamp");
        };
        let Some(end) = args.get(3).map(exp_to_timestamp).transpose()? else {
            return plan_err!("Missing 4th argument, expected a UTC nanoseconds timestamp");
        };

        let lake = self.lake.clone();

        let spawner = move || {
            let (tx, rx) = tokio::sync::mpsc::channel(100);
            let logger = Arc::new(LogSender::new(tx));
            tokio::spawn(async move {
                if let Err(e) = retire_partitions_impl(
                    lake,
                    &view_set_name,
                    &view_instance_id,
                    begin,
                    end,
                    logger,
                )
                .await
                .with_context(|| "retire_partitions_impl")
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
