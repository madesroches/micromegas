use super::{partition_cache::QueryPartitionProvider, view_factory::ViewFactory};
use crate::time::TimeRange as QueryTimeRange;
use datafusion::{
    arrow::{
        array::{BinaryArray, Int32Array, RecordBatch},
        datatypes::SchemaRef,
    },
    catalog::{Session, TableProvider},
    common::Result as DFResult,
    execution::{SendableRecordBatchStream, TaskContext, runtime_env::RuntimeEnv},
    logical_expr::{Expr, TableType},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
        execution_plan::{Boundedness, EmissionType},
        stream::RecordBatchStreamAdapter,
    },
};
use futures::stream;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::{
    any::Any,
    fmt::{self, Debug, Formatter},
    sync::Arc,
};
use tokio::sync::mpsc;

/// Span types to include in the trace
#[derive(Debug, Clone, Copy)]
pub enum SpanTypes {
    Thread,
    Async,
    Both,
}

/// Time range for the trace
#[derive(Debug, Clone)]
pub struct TimeRange {
    pub start: chrono::DateTime<chrono::Utc>,
    pub end: chrono::DateTime<chrono::Utc>,
}

/// Execution plan that generates Perfetto trace chunks
pub struct PerfettoTraceExecutionPlan {
    schema: SchemaRef,
    process_id: String,
    span_types: SpanTypes,
    time_range: TimeRange,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    object_store: Arc<dyn ObjectStore>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<QueryTimeRange>,
    properties: PlanProperties,
}

impl PerfettoTraceExecutionPlan {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        schema: SchemaRef,
        process_id: String,
        span_types: SpanTypes,
        time_range: TimeRange,
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        object_store: Arc<dyn ObjectStore>,
        view_factory: Arc<ViewFactory>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        query_range: Option<QueryTimeRange>,
    ) -> Self {
        let properties = PlanProperties::new(
            EquivalenceProperties::new(schema.clone()),
            Partitioning::UnknownPartitioning(1),
            EmissionType::Final,
            Boundedness::Bounded,
        );

        Self {
            schema,
            process_id,
            span_types,
            time_range,
            runtime,
            lake,
            object_store,
            view_factory,
            part_provider,
            query_range,
            properties,
        }
    }
}

impl Debug for PerfettoTraceExecutionPlan {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PerfettoTraceExecutionPlan")
            .field("process_id", &self.process_id)
            .field("span_types", &self.span_types)
            .field("time_range", &self.time_range)
            .finish()
    }
}

impl DisplayAs for PerfettoTraceExecutionPlan {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "PerfettoTraceExecutionPlan: process_id={}, span_types={:?}, time_range={}..{}",
            self.process_id, self.span_types, self.time_range.start, self.time_range.end
        )
    }
}

impl ExecutionPlan for PerfettoTraceExecutionPlan {
    fn name(&self) -> &str {
        "PerfettoTraceExecutionPlan"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn properties(&self) -> &PlanProperties {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    fn with_new_children(
        self: Arc<Self>,
        _children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> DFResult<Arc<dyn ExecutionPlan>> {
        Ok(self)
    }

    fn execute(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> DFResult<SendableRecordBatchStream> {
        let schema = self.schema.clone();
        let process_id = self.process_id.clone();
        let span_types = self.span_types;
        let time_range = self.time_range.clone();
        let runtime = self.runtime.clone();
        let lake = self.lake.clone();
        let object_store = self.object_store.clone();
        let view_factory = self.view_factory.clone();
        let part_provider = self.part_provider.clone();
        let query_range = self.query_range;

        // Create a channel for streaming chunks
        let (chunk_sender, chunk_receiver) = mpsc::channel::<DFResult<RecordBatch>>(16);

        // Spawn the trace generation task
        tokio::spawn(async move {
            let result = generate_trace_chunks(
                process_id,
                span_types,
                time_range,
                runtime,
                lake,
                object_store,
                view_factory,
                part_provider,
                query_range,
                chunk_sender.clone(),
            )
            .await;

            // Send error if generation failed
            if let Err(e) = result {
                let _ = chunk_sender
                    .send(Err(datafusion::error::DataFusionError::Execution(format!(
                        "Trace generation failed: {}",
                        e
                    ))))
                    .await;
            }
        });

        // Create a stream from the receiver
        let stream = stream::unfold(chunk_receiver, |mut receiver| async move {
            receiver.recv().await.map(|batch| (batch, receiver))
        });

        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
    }
}

/// Generate Perfetto trace chunks and send them through the channel
#[expect(clippy::too_many_arguments)]
async fn generate_trace_chunks(
    process_id: String,
    span_types: SpanTypes,
    time_range: TimeRange,
    _runtime: Arc<RuntimeEnv>,
    _lake: Arc<DataLakeConnection>,
    _object_store: Arc<dyn ObjectStore>,
    _view_factory: Arc<ViewFactory>,
    _part_provider: Arc<dyn QueryPartitionProvider>,
    _query_range: Option<QueryTimeRange>,
    chunk_sender: mpsc::Sender<DFResult<RecordBatch>>,
) -> anyhow::Result<()> {
    // Phase 5: Just return dummy chunks for now
    // Phase 6 will implement the actual trace generation logic

    info!(
        "Generating Perfetto trace chunks for process {} with span types {:?} from {} to {}",
        process_id, span_types, time_range.start, time_range.end
    );

    // Send a few dummy chunks to verify the streaming infrastructure works
    for chunk_id in 0..3 {
        let chunk_data = format!("Dummy chunk {} for process {}", chunk_id, process_id);

        // Create the RecordBatch with chunk_id and chunk_data
        let chunk_id_array = Int32Array::from(vec![chunk_id]);
        let chunk_data_array = BinaryArray::from(vec![chunk_data.as_bytes()]);

        let batch = RecordBatch::try_from_iter(vec![
            (
                "chunk_id",
                Arc::new(chunk_id_array) as Arc<dyn datafusion::arrow::array::Array>,
            ),
            (
                "chunk_data",
                Arc::new(chunk_data_array) as Arc<dyn datafusion::arrow::array::Array>,
            ),
        ])?;

        // Send the chunk
        if chunk_sender.send(Ok(batch)).await.is_err() {
            // Receiver dropped, stop generating
            break;
        }
    }

    Ok(())
}

/// TableProvider wrapper for PerfettoTraceExecutionPlan
#[derive(Debug)]
pub struct PerfettoTraceTableProvider {
    execution_plan: Arc<PerfettoTraceExecutionPlan>,
}

impl PerfettoTraceTableProvider {
    pub fn new(execution_plan: Arc<PerfettoTraceExecutionPlan>) -> Self {
        Self { execution_plan }
    }
}

#[async_trait::async_trait]
impl TableProvider for PerfettoTraceTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.execution_plan.schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        _projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        _limit: Option<usize>,
    ) -> DFResult<Arc<dyn ExecutionPlan>> {
        Ok(self.execution_plan.clone())
    }
}
