use super::{partition_cache::QueryPartitionProvider, view_factory::ViewFactory};
use crate::dfext::typed_column::typed_column_by_name;
use crate::time::TimeRange as QueryTimeRange;
use datafusion::{
    arrow::{
        array::{
            BinaryArray, Int32Array, RecordBatch, StringArray, TimestampNanosecondArray,
            UInt32Array,
        },
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
use micromegas_perfetto::StreamingPerfettoWriter;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::{
    any::Any,
    collections::HashMap,
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

/// A writer that captures Perfetto packets and sends them as chunks
struct ChunkWriter {
    sender: mpsc::Sender<DFResult<RecordBatch>>,
    chunk_id: i32,
    buffer: Vec<u8>,
}

impl ChunkWriter {
    fn new(sender: mpsc::Sender<DFResult<RecordBatch>>) -> Self {
        Self {
            sender,
            chunk_id: 0,
            buffer: Vec::new(),
        }
    }
}

impl std::io::Write for ChunkWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Flush immediately when called
        if !self.buffer.is_empty() {
            let chunk_id_array = Int32Array::from(vec![self.chunk_id]);
            let chunk_data_array = BinaryArray::from(vec![self.buffer.as_slice()]);

            let batch = RecordBatch::try_from_iter(vec![
                (
                    "chunk_id",
                    Arc::new(chunk_id_array) as Arc<dyn datafusion::arrow::array::Array>,
                ),
                (
                    "chunk_data",
                    Arc::new(chunk_data_array) as Arc<dyn datafusion::arrow::array::Array>,
                ),
            ])
            .map_err(std::io::Error::other)?;

            // Use blocking send - this is not ideal but necessary for the Write trait
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                self.sender.send(Ok(batch)).await.map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Receiver dropped")
                })
            })?;

            self.chunk_id += 1;
            self.buffer.clear();
        }
        Ok(())
    }
}

/// Get process executable name from the processes table
async fn get_process_exe(
    process_id: &str,
    ctx: &datafusion::execution::context::SessionContext,
) -> anyhow::Result<String> {
    let sql = format!(
        r#"
        SELECT exe
        FROM processes
        WHERE process_id = '{}'
        LIMIT 1
        "#,
        process_id
    );

    let df = ctx.sql(&sql).await?;
    let batches = df.collect().await?;

    if batches.is_empty() || batches[0].num_rows() == 0 {
        anyhow::bail!("Process {} not found", process_id);
    }

    let exes: &StringArray = typed_column_by_name(&batches[0], "exe")?;
    Ok(exes.value(0).to_owned())
}

/// Get thread information from the streams table
async fn get_thread_info(
    process_id: &str,
    ctx: &datafusion::execution::context::SessionContext,
) -> anyhow::Result<HashMap<String, (i32, String)>> {
    let sql = format!(
        r#"
        SELECT DISTINCT stream_id,
               property_get(properties, 'thread-name') as thread_name,
               property_get(properties, 'thread-id') as thread_id
        FROM streams
        WHERE process_id = '{}'
        AND array_has(tags, 'cpu')
        ORDER BY stream_id
        "#,
        process_id
    );

    let df = ctx.sql(&sql).await?;
    let batches = df.collect().await?;
    let mut threads = HashMap::new();

    for batch in batches {
        let stream_ids: &StringArray = typed_column_by_name(&batch, "stream_id")?;
        let thread_names: &StringArray = typed_column_by_name(&batch, "thread_name")?;
        let thread_ids: &StringArray = typed_column_by_name(&batch, "thread_id")?;

        for i in 0..batch.num_rows() {
            let stream_id = stream_ids.value(i).to_owned();
            let thread_name = thread_names.value(i).to_owned();
            let thread_id_str = thread_ids.value(i);

            // Parse thread ID or use hash if parsing fails
            let thread_id: i32 = if let Ok(id) = thread_id_str.parse::<i32>() {
                id
            } else {
                // Use a hash of the stream_id as the thread ID
                (stream_id
                    .as_bytes()
                    .iter()
                    .fold(0u32, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u32))
                    % 65536) as i32
            };

            threads.insert(stream_id, (thread_id, thread_name));
        }
    }

    Ok(threads)
}

/// Generate thread spans and emit them as packets
async fn generate_thread_spans(
    writer: &mut StreamingPerfettoWriter<&mut ChunkWriter>,
    _process_id: &str,
    ctx: &datafusion::execution::context::SessionContext,
    time_range: &TimeRange,
    threads: &HashMap<String, (i32, String)>,
) -> anyhow::Result<()> {
    for stream_id in threads.keys() {
        let sql = format!(
            r#"
            SELECT begin, end, name, filename, target, line
            FROM view_instance('thread_spans', '{}')
            WHERE begin >= TIMESTAMP '{}'
              AND end <= TIMESTAMP '{}'
            ORDER BY begin
            "#,
            stream_id,
            time_range.start.to_rfc3339(),
            time_range.end.to_rfc3339()
        );

        let df = ctx.sql(&sql).await?;
        let batches = df.collect().await?;

        for batch in batches {
            let begin_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "begin")?;
            let end_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "end")?;
            let names: &StringArray = typed_column_by_name(&batch, "name")?;
            let filenames: &StringArray = typed_column_by_name(&batch, "filename")?;
            let targets: &StringArray = typed_column_by_name(&batch, "target")?;
            let lines: &UInt32Array = typed_column_by_name(&batch, "line")?;

            for i in 0..batch.num_rows() {
                let begin_ns = begin_times.value(i) as u64;
                let end_ns = end_times.value(i) as u64;
                let name = names.value(i);
                let filename = filenames.value(i);
                let target = targets.value(i);
                let line = lines.value(i);

                // Set the current thread for this span and emit the span
                let (thread_id, thread_name) = &threads[stream_id];
                writer.emit_thread_descriptor(stream_id, *thread_id, thread_name)?;
                writer.emit_span(begin_ns, end_ns, name, target, filename, line)?;
                writer
                    .flush()
                    .map_err(|e| anyhow::anyhow!("Failed to send thread span: {}", e))?;
            }
        }
    }

    Ok(())
}

/// Generate async spans and emit them as packets
async fn generate_async_spans(
    writer: &mut StreamingPerfettoWriter<&mut ChunkWriter>,
    process_id: &str,
    ctx: &datafusion::execution::context::SessionContext,
    time_range: &TimeRange,
) -> anyhow::Result<()> {
    let sql = format!(
        r#"
        WITH begin_events AS (
            SELECT span_id, time as begin_time, name, filename, target, line
            FROM view_instance('async_events', '{}')
            WHERE time >= TIMESTAMP '{}'
              AND time <= TIMESTAMP '{}'
              AND event_type = 'begin'
        ),
        end_events AS (
            SELECT span_id, time as end_time
            FROM view_instance('async_events', '{}')
            WHERE time >= TIMESTAMP '{}'
              AND time <= TIMESTAMP '{}'
              AND event_type = 'end'
        )
        SELECT 
            b.span_id,
            b.begin_time,
            e.end_time,
            b.name,
            b.filename,
            b.target,
            b.line
        FROM begin_events b
        INNER JOIN end_events e ON b.span_id = e.span_id
        ORDER BY b.begin_time
        "#,
        process_id,
        time_range.start.to_rfc3339(),
        time_range.end.to_rfc3339(),
        process_id,
        time_range.start.to_rfc3339(),
        time_range.end.to_rfc3339()
    );

    let df = ctx.sql(&sql).await?;
    let batches = df.collect().await?;

    for batch in batches {
        let begin_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "begin_time")?;
        let end_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "end_time")?;
        let names: &StringArray = typed_column_by_name(&batch, "name")?;
        let filenames: &StringArray = typed_column_by_name(&batch, "filename")?;
        let targets: &StringArray = typed_column_by_name(&batch, "target")?;
        let lines: &UInt32Array = typed_column_by_name(&batch, "line")?;

        for i in 0..batch.num_rows() {
            let begin_ns = begin_times.value(i) as u64;
            let end_ns = end_times.value(i) as u64;
            let name = names.value(i);
            let filename = filenames.value(i);
            let target = targets.value(i);
            let line = lines.value(i);

            if end_ns >= begin_ns {
                // Emit begin event
                writer.emit_async_span_begin(begin_ns, name, target, filename, line)?;
                writer
                    .flush()
                    .map_err(|e| anyhow::anyhow!("Failed to send async span begin: {}", e))?;

                // Emit end event
                writer.emit_async_span_end(end_ns, name, target, filename, line)?;
                writer
                    .flush()
                    .map_err(|e| anyhow::anyhow!("Failed to send async span end: {}", e))?;
            } else {
                warn!("Skipping async span '{}' with invalid duration", name);
            }
        }
    }

    Ok(())
}

/// Generate Perfetto trace chunks and send them through the channel
#[expect(clippy::too_many_arguments)]
async fn generate_trace_chunks(
    process_id: String,
    span_types: SpanTypes,
    time_range: TimeRange,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    _object_store: Arc<dyn ObjectStore>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<QueryTimeRange>,
    chunk_sender: mpsc::Sender<DFResult<RecordBatch>>,
) -> anyhow::Result<()> {
    info!(
        "Generating Perfetto trace chunks for process {} with span types {:?} from {} to {}",
        process_id, span_types, time_range.start, time_range.end
    );

    let mut chunk_writer = ChunkWriter::new(chunk_sender);

    // Create a context for making queries
    let ctx =
        super::query::make_session_context(runtime, lake, part_provider, query_range, view_factory)
            .await?;

    // Phase 6: Generate real Perfetto trace using a single streaming writer
    let mut writer = StreamingPerfettoWriter::new(&mut chunk_writer, &process_id);

    // Step 1: Get process metadata and emit process descriptor
    let process_exe = get_process_exe(&process_id, &ctx).await?;
    writer.emit_process_descriptor(&process_exe)?;
    writer
        .flush()
        .map_err(|e| anyhow::anyhow!("Failed to send process descriptor: {}", e))?;

    // Step 2: Get thread information and emit thread descriptors
    let threads = get_thread_info(&process_id, &ctx).await?;
    for (stream_id, (thread_id, thread_name)) in &threads {
        writer.emit_thread_descriptor(stream_id, *thread_id, thread_name)?;
        writer
            .flush()
            .map_err(|e| anyhow::anyhow!("Failed to send thread descriptor: {}", e))?;
    }

    // Step 3: Emit async track descriptor if we're including async spans
    if matches!(span_types, SpanTypes::Async | SpanTypes::Both) {
        writer.emit_async_track_descriptor()?;
        writer
            .flush()
            .map_err(|e| anyhow::anyhow!("Failed to send async track descriptor: {}", e))?;
    }

    // Step 4: Generate thread spans if requested
    if matches!(span_types, SpanTypes::Thread | SpanTypes::Both) {
        generate_thread_spans(&mut writer, &process_id, &ctx, &time_range, &threads).await?;
    }

    // Step 5: Generate async spans if requested
    if matches!(span_types, SpanTypes::Async | SpanTypes::Both) {
        generate_async_spans(&mut writer, &process_id, &ctx, &time_range).await?;
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
