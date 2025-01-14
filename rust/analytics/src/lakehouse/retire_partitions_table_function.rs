use anyhow::Context;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::Session;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::common::plan_err;
use datafusion::datasource::TableType;
use datafusion::error::DataFusionError;
use datafusion::execution::context::ExecutionProps;
use datafusion::logical_expr::simplify::SimplifyContext;
use datafusion::optimizer::simplify_expressions::ExprSimplifier;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::Expr;
use datafusion::scalar::ScalarValue;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::error;
use std::any::Any;
use std::sync::Arc;

use crate::async_log_stream::LogExecPlan;
use crate::response_writer::LogSender;
use crate::response_writer::Logger;

use super::write_partition::retire_partitions;

#[derive(Debug)]
pub struct RetirePartitionsTableFunction {
    lake: Arc<DataLakeConnection>,
}

impl RetirePartitionsTableFunction {
    pub fn new(lake: Arc<DataLakeConnection>) -> Self {
        Self { lake }
    }
}

fn simplify_exp(expr: &Expr) -> datafusion::error::Result<Expr> {
    let execution_props = ExecutionProps::new();
    let info = SimplifyContext::new(&execution_props);
    ExprSimplifier::new(info).simplify(expr.clone())
}

fn exp_to_string(expr: &Expr) -> datafusion::error::Result<String> {
    match simplify_exp(expr)? {
        Expr::Literal(ScalarValue::Utf8(Some(string))) => Ok(string),
        other => {
            plan_err!("can't convert {other:?} to string")
        }
    }
}

fn exp_to_timestamp(expr: &Expr) -> datafusion::error::Result<DateTime<Utc>> {
    match simplify_exp(expr)? {
        Expr::Literal(ScalarValue::Utf8(Some(string))) => {
            let ts = chrono::DateTime::parse_from_rfc3339(&string)
                .map_err(|e| DataFusionError::External(e.into()))?;
            Ok(ts.into())
        }
        Expr::Literal(ScalarValue::TimestampNanosecond(Some(ns), timezone)) => {
            if let Some(tz) = timezone {
                if *tz != *"+00:00" {
                    return plan_err!("Timestamp should be in UTC");
                }
            }
            Ok(DateTime::from_timestamp_nanos(ns))
        }
        other => {
            plan_err!("can't convert {other:?} to timestamp")
        }
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

        Ok(Arc::new(RetirePartitionsTableProvider {
            log_stream: Arc::new(LogExecPlan::new(Box::new(spawner))),
        }))
    }
}

#[derive(Debug)]
pub struct RetirePartitionsTableProvider {
    pub log_stream: Arc<LogExecPlan>,
}

#[async_trait]
impl TableProvider for RetirePartitionsTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.log_stream.schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        _projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        _limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        Ok(self.log_stream.clone())
    }
}
