use super::task_log_exec_plan::TaskLogExecPlan;
use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::Session;
use datafusion::catalog::TableProvider;
use datafusion::datasource::TableType;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::limit::GlobalLimitExec;
use datafusion::prelude::Expr;
use std::any::Any;
use std::sync::Arc;

/// A DataFusion `TableProvider` for a log stream.
#[derive(Debug)]
pub struct LogStreamTableProvider {
    /// The underlying log stream execution plan.
    pub log_stream: Arc<TaskLogExecPlan>,
}

#[async_trait]
impl TableProvider for LogStreamTableProvider {
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
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        // Wrap the execution plan in a GlobalLimitExec if a limit is provided.
        // DataFusion trusts us to apply the limit - if we ignore it, too many rows
        // will be returned to the client.
        let plan: Arc<dyn ExecutionPlan> = self.log_stream.clone();
        if let Some(fetch) = limit {
            Ok(Arc::new(GlobalLimitExec::new(plan, 0, Some(fetch))))
        } else {
            Ok(plan)
        }
    }
}
