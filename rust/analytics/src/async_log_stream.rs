use datafusion::{
    arrow::{
        array::{PrimitiveBuilder, RecordBatch, StringBuilder},
        datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit, TimestampNanosecondType},
    },
    common::{internal_err, Result, Statistics},
    error::DataFusionError,
    execution::{RecordBatchStream, SendableRecordBatchStream, TaskContext},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        execution_plan::{Boundedness, EmissionType},
        DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
    },
};
use futures::Stream;
use std::{
    any::Any,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::mpsc;

pub type TaskSpawner =
    dyn FnOnce() -> mpsc::Receiver<(chrono::DateTime<chrono::Utc>, String)> + Sync + Send;

pub struct LogExecPlan {
    schema: SchemaRef,
    cache: PlanProperties,
    spawner: std::sync::Mutex<Option<Box<TaskSpawner>>>,
}

impl DisplayAs for LogExecPlan {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => {
                write!(f, "LogExecPlan")
            }
        }
    }
}

impl std::fmt::Debug for LogExecPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LogExecPlan")
    }
}

impl LogExecPlan {
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

impl ExecutionPlan for LogExecPlan {
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
    ) -> Result<Arc<dyn ExecutionPlan>> {
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
    ) -> Result<SendableRecordBatchStream> {
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

    fn statistics(&self) -> Result<Statistics> {
        Ok(Statistics::new_unknown(&self.schema))
    }
}

pub struct AsyncLogStream {
    schema: SchemaRef,
    rx: mpsc::Receiver<(chrono::DateTime<chrono::Utc>, String)>,
}

impl AsyncLogStream {
    pub fn new(
        schema: SchemaRef,
        rx: mpsc::Receiver<(chrono::DateTime<chrono::Utc>, String)>,
    ) -> Self {
        Self { schema, rx }
    }
}

impl Stream for AsyncLogStream {
    type Item = Result<RecordBatch>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut messages = vec![];
        let limit = self.rx.max_capacity();
        if self
            .rx
            .poll_recv_many(cx, &mut messages, limit)
            .is_pending()
        {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        if messages.is_empty() {
            if self.rx.is_closed() {
                // channel closed, aborting
                return Poll::Ready(None);
            }
            // not sure this can happen
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }

        let mut times = PrimitiveBuilder::<TimestampNanosecondType>::with_capacity(messages.len());
        let mut msgs = StringBuilder::new();
        for msg in messages {
            times.append_value(msg.0.timestamp_nanos_opt().unwrap_or_default());
            msgs.append_value(msg.1);
        }

        let rb_res = RecordBatch::try_new(
            self.schema.clone(),
            vec![
                Arc::new(times.finish().with_timezone_utc()),
                Arc::new(msgs.finish()),
            ],
        )
        .map_err(|e| DataFusionError::ArrowError(e, None));
        Poll::Ready(Some(rb_res))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.rx.len(), Some(self.rx.len()))
    }
}

impl RecordBatchStream for AsyncLogStream {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }
}
