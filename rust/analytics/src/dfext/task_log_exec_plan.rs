use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::datatypes::TimeUnit;
use datafusion::common::Statistics;
use datafusion::common::internal_err;
use datafusion::error::DataFusionError;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::execution::TaskContext;
use datafusion::physical_expr::EquivalenceProperties;
use datafusion::physical_plan::DisplayAs;
use datafusion::physical_plan::DisplayFormatType;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::Partitioning;
use datafusion::physical_plan::PlanProperties;
use datafusion::physical_plan::execution_plan::Boundedness;
use datafusion::physical_plan::execution_plan::EmissionType;
use std::any::Any;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::async_log_stream::AsyncLogStream;

/// A type alias for a function that spawns a log message receiver.
pub type TaskSpawner =
    dyn FnOnce() -> mpsc::Receiver<(chrono::DateTime<chrono::Utc>, String)> + Sync + Send;

/// An `ExecutionPlan` that provides a stream of log messages.
pub struct TaskLogExecPlan {
    schema: SchemaRef,
    cache: PlanProperties,
    spawner: std::sync::Mutex<Option<Box<TaskSpawner>>>,
}

impl DisplayAs for TaskLogExecPlan {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default
            | DisplayFormatType::Verbose
            | DisplayFormatType::TreeRender => {
                write!(f, "TaskLogExecPlan")
            }
        }
    }
}

impl std::fmt::Debug for TaskLogExecPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TaskLogExecPlan")
    }
}

impl TaskLogExecPlan {
    pub fn new(spawner: Box<TaskSpawner>) -> Self {
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "time",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new("msg", DataType::Utf8, false),
        ]));

        let cache = PlanProperties::new(
            EquivalenceProperties::new(Arc::clone(&schema)),
            Partitioning::RoundRobinBatch(1),
            EmissionType::Incremental,
            Boundedness::Unbounded {
                requires_infinite_memory: false,
            },
        );

        Self {
            schema,
            cache,
            spawner: std::sync::Mutex::new(Some(spawner)),
        }
    }
}

impl ExecutionPlan for TaskLogExecPlan {
    fn name(&self) -> &'static str {
        "LogExecPlan"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn properties(&self) -> &PlanProperties {
        &self.cache
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        if children.is_empty() {
            Ok(self)
        } else {
            internal_err!("Children cannot be replaced in LogExecPlan")
        }
    }

    fn execute(
        &self,
        partition: usize,
        _context: Arc<TaskContext>,
    ) -> datafusion::error::Result<SendableRecordBatchStream> {
        if partition >= 1 {
            return internal_err!("Invalid partition {partition} for LogExecPlan");
        }

        let mut spawner = self.spawner.lock().map_err(|_| {
            DataFusionError::Execution("Error locking mutex in LogExecPlan".to_owned())
        })?;
        if let Some(fun) = spawner.take() {
            drop(spawner);
            Ok(Box::pin(AsyncLogStream::new(self.schema.clone(), fun())))
        } else {
            internal_err!("Spawner already taken in LogExecPlan")
        }
    }

    fn statistics(&self) -> datafusion::error::Result<Statistics> {
        Ok(Statistics::new_unknown(&self.schema))
    }
}
