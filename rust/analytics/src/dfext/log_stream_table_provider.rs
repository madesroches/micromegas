use super::task_log_exec_plan::TaskLogExecPlan;
use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::Session;
use datafusion::catalog::TableProvider;
use datafusion::datasource::TableType;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::limit::GlobalLimitExec;
use datafusion::physical_plan::projection::ProjectionExec;
use datafusion::prelude::Expr;
use std::sync::Arc;

/// A DataFusion `TableProvider` for a log stream.
#[derive(Debug)]
pub struct LogStreamTableProvider {
    /// The underlying log stream execution plan.
    pub log_stream: Arc<TaskLogExecPlan>,
}

#[async_trait]
impl TableProvider for LogStreamTableProvider {
    fn schema(&self) -> SchemaRef {
        self.log_stream.schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        let mut plan: Arc<dyn ExecutionPlan> = self.log_stream.clone();
        // DataFusion trusts the scan to apply the pushed-down projection: returning the full
        // (time, msg) schema against a narrower projection fails physical planning (e.g.
        // `SELECT msg FROM retire_partitions(...)` or `SELECT msg AS rule_id FROM
        // deny_queries(...)`). Same idiom as `process_spans_table_function.rs`.
        if let Some(projection) = projection {
            let schema = plan.schema();
            let projected_exprs: Vec<(Arc<dyn datafusion::physical_expr::PhysicalExpr>, String)> =
                projection
                    .iter()
                    .map(|&i| {
                        let name = schema.field(i).name().clone();
                        let expr = Arc::new(datafusion::physical_expr::expressions::Column::new(
                            &name, i,
                        ))
                            as Arc<dyn datafusion::physical_expr::PhysicalExpr>;
                        (expr, name)
                    })
                    .collect();
            plan = Arc::new(ProjectionExec::try_new(projected_exprs, plan)?);
        }
        // DataFusion likewise trusts us to apply the limit - if we ignore it, too many rows
        // will be returned to the client.
        if let Some(fetch) = limit {
            plan = Arc::new(GlobalLimitExec::new(plan, 0, Some(fetch)));
        }
        Ok(plan)
    }
}
