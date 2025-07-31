use super::task_log_exec_plan::TaskLogExecPlan;
use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::Session;
use datafusion::catalog::TableProvider;
use datafusion::datasource::TableType;
use datafusion::physical_plan::ExecutionPlan;
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
        _limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        Ok(self.log_stream.clone())
    }
}
