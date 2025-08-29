use super::{partition_cache::QueryPartitionProvider, view_factory::ViewFactory};
use crate::dfext::typed_column::typed_column_by_name;
use crate::time::TimeRange;
use async_stream::stream;
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
use futures::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_perfetto::AsyncStreamingPerfettoWriter;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::{
    any::Any,
    collections::HashMap,
    fmt::{self, Debug, Formatter},
    sync::Arc,
};

/// Span types to include in the trace
#[derive(Debug, Clone, Copy)]
pub enum SpanTypes {
    Thread,
    Async,
    Both,
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
            self.process_id, self.span_types, self.time_range.begin, self.time_range.end
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
        let time_range = self.time_range;
        let runtime = self.runtime.clone();
        let lake = self.lake.clone();
        let object_store = self.object_store.clone();
        let view_factory = self.view_factory.clone();
        let part_provider = self.part_provider.clone();

        // Create the stream directly without channels
        let stream = generate_perfetto_trace_stream(
            process_id,
            span_types,
            time_range,
            runtime,
            lake,
            object_store,
            view_factory,
            part_provider,
        );

        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
    }
}

/// Creates a stream of Perfetto trace chunks by generating the complete trace first
#[expect(clippy::too_many_arguments)]
fn generate_perfetto_trace_stream(
    process_id: String,
    span_types: SpanTypes,
    time_range: TimeRange,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    object_store: Arc<dyn ObjectStore>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
) -> impl futures::Stream<Item = DFResult<RecordBatch>> {
    stream! {
        // Generate the complete trace in memory first
        match generate_complete_perfetto_trace(
            process_id,
            span_types,
            time_range,
            runtime,
            lake,
            object_store,
            view_factory,
            part_provider,
        ).await {
            Ok(trace_data) => {
                // Split the trace data into chunks and yield each as a RecordBatch
                const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks

                for (chunk_id, chunk) in trace_data.chunks(CHUNK_SIZE).enumerate() {
                    let chunk_id_array = Int32Array::from(vec![chunk_id as i32]);
                    let chunk_data_array = BinaryArray::from(vec![chunk]);

                    match RecordBatch::try_from_iter(vec![
                        ("chunk_id", Arc::new(chunk_id_array) as Arc<dyn datafusion::arrow::array::Array>),
                        ("chunk_data", Arc::new(chunk_data_array) as Arc<dyn datafusion::arrow::array::Array>),
                    ]) {
                        Ok(batch) => yield Ok(batch),
                        Err(e) => {
                            error!("Failed to create chunk batch: {:?}", e);
                            yield Err(datafusion::error::DataFusionError::Execution(
                                format!("Failed to create batch: {}", e)
                            ));
                            return;
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to generate trace: {:?}", e);
                yield Err(datafusion::error::DataFusionError::Execution(
                    format!("Trace generation failed: {}", e)
                ));
            }
        }
    }
}

/// Generate the complete Perfetto trace in memory
#[expect(clippy::too_many_arguments)]
async fn generate_complete_perfetto_trace(
    process_id: String,
    span_types: SpanTypes,
    time_range: TimeRange,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    _object_store: Arc<dyn ObjectStore>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
) -> anyhow::Result<Vec<u8>> {
    info!(
        "Generating Perfetto trace for process {} with span types {:?} from {} to {}",
        process_id, span_types, time_range.begin, time_range.end
    );

    // Create a context for making queries
    let ctx = super::query::make_session_context(
        runtime,
        lake,
        part_provider,
        Some(TimeRange {
            begin: time_range.begin,
            end: time_range.end,
        }),
        view_factory,
    )
    .await?;

    // Use Vec<u8> as the writer - this is much simpler than AsyncWrite
    let mut trace_buffer = Vec::new();
    let mut writer = AsyncStreamingPerfettoWriter::new(&mut trace_buffer, &process_id);

    // Get process metadata and emit process descriptor
    let process_exe = get_process_exe(&process_id, &ctx).await?;
    writer.emit_process_descriptor(&process_exe).await?;
    writer.flush().await?;

    // Get thread information and emit thread descriptors
    let threads = get_process_thread_list(&process_id, &ctx).await?;
    for (stream_id, (thread_id, thread_name)) in &threads {
        writer
            .emit_thread_descriptor(stream_id, *thread_id, thread_name)
            .await?;
    }
    if !threads.is_empty() {
        writer.flush().await?;
    }

    // Emit async track descriptor if we're including async spans
    if matches!(span_types, SpanTypes::Async | SpanTypes::Both) {
        writer.emit_async_track_descriptor().await?;
        writer.flush().await?;
    }

    // Generate thread spans if requested
    if matches!(span_types, SpanTypes::Thread | SpanTypes::Both) {
        generate_thread_spans_with_writer(&mut writer, &process_id, &ctx, &time_range, &threads)
            .await?;
    }

    // Generate async spans if requested
    if matches!(span_types, SpanTypes::Async | SpanTypes::Both) {
        generate_async_spans_with_writer(&mut writer, &process_id, &ctx, &time_range).await?;
    }

    // Final flush to ensure all data is written
    writer.flush().await?;

    Ok(trace_buffer)
}

/// Get process executable name from the processes table
async fn get_process_exe(
    process_id: &str,
    ctx: &datafusion::execution::context::SessionContext,
) -> anyhow::Result<String> {
    let sql = format!(
        r#"
        SELECT arrow_cast(exe, 'Utf8') as exe
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
async fn get_process_thread_list(
    process_id: &str,
    ctx: &datafusion::execution::context::SessionContext,
) -> anyhow::Result<HashMap<String, (i32, String)>> {
    // Query blocks table to get streams, then filter by checking if thread spans exist
    let sql = format!(
        r#"
        SELECT DISTINCT arrow_cast(b.stream_id, 'Utf8') as stream_id
        FROM blocks b
        WHERE b.process_id = '{}'
        AND array_has(b."streams.tags", 'cpu')
        "#,
        process_id
    );

    let df = ctx.sql(&sql).await?;
    let batches = df.collect().await?;
    let mut threads = HashMap::new();

    for batch in batches {
        let stream_ids: &StringArray = typed_column_by_name(&batch, "stream_id")?;

        for i in 0..batch.num_rows() {
            let stream_id = stream_ids.value(i).to_owned();

            // Use a hash of the stream_id as the thread ID
            let thread_id: i32 = (stream_id
                .as_bytes()
                .iter()
                .fold(0u32, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u32))
                % 65536) as i32;

            // Use a simplified thread name based on stream_id
            let thread_name = format!("thread-{}", &stream_id[..8]);

            threads.insert(stream_id, (thread_id, thread_name));
        }
    }

    Ok(threads)
}

/// Generate thread spans using the provided AsyncStreamingPerfettoWriter
async fn generate_thread_spans_with_writer<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut AsyncStreamingPerfettoWriter<W>,
    _process_id: &str,
    ctx: &datafusion::execution::context::SessionContext,
    time_range: &TimeRange,
    threads: &HashMap<String, (i32, String)>,
) -> anyhow::Result<()> {
    for stream_id in threads.keys() {
        let sql = format!(
            r#"
            SELECT begin, end, 
                   arrow_cast(name, 'Utf8') as name,
                   arrow_cast(filename, 'Utf8') as filename,
                   arrow_cast(target, 'Utf8') as target,
                   line
            FROM view_instance('thread_spans', '{}')
            WHERE begin <= TIMESTAMP '{}'
              AND end >= TIMESTAMP '{}'
            ORDER BY begin
            "#,
            stream_id,
            time_range.end.to_rfc3339(),
            time_range.begin.to_rfc3339()
        );

        let df = ctx.sql(&sql).await?;
        let mut stream = df.execute_stream().await?;

        let mut span_count = 0;
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
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

                // Use the single writer instance to maintain string interning
                writer
                    .emit_span(begin_ns, end_ns, name, target, filename, line)
                    .await?;

                span_count += 1;
                // Flush every 10 thread spans to create multiple chunks
                if span_count % 10 == 0 {
                    writer.flush().await?;
                }
            }
        }
    }
    Ok(())
}

/// Generate async spans using the provided AsyncStreamingPerfettoWriter
async fn generate_async_spans_with_writer<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut AsyncStreamingPerfettoWriter<W>,
    process_id: &str,
    ctx: &datafusion::execution::context::SessionContext,
    time_range: &TimeRange,
) -> anyhow::Result<()> {
    let sql = format!(
        r#"
        WITH begin_events AS (
            SELECT span_id, time as begin_time, 
                   arrow_cast(name, 'Utf8') as name, 
                   arrow_cast(filename, 'Utf8') as filename, 
                   arrow_cast(target, 'Utf8') as target, 
                   line
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
        time_range.begin.to_rfc3339(),
        time_range.end.to_rfc3339(),
        process_id,
        time_range.begin.to_rfc3339(),
        time_range.end.to_rfc3339(),
    );

    let df = ctx.sql(&sql).await?;
    let mut stream = df.execute_stream().await?;

    let mut span_count = 0;
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        let span_ids: &datafusion::arrow::array::Int64Array =
            typed_column_by_name(&batch, "span_id")?;
        let begin_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "begin_time")?;
        let end_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "end_time")?;
        let names: &StringArray = typed_column_by_name(&batch, "name")?;
        let filenames: &StringArray = typed_column_by_name(&batch, "filename")?;
        let targets: &StringArray = typed_column_by_name(&batch, "target")?;
        let lines: &UInt32Array = typed_column_by_name(&batch, "line")?;
        debug!("span {span_count}");

        for i in 0..batch.num_rows() {
            let _span_id = span_ids.value(i);
            let begin_ns = begin_times.value(i) as u64;
            let end_ns = end_times.value(i) as u64;
            let name = names.value(i);
            let filename = filenames.value(i);
            let target = targets.value(i);
            let line = lines.value(i);

            if begin_ns < end_ns {
                // Emit async span begin and end events with single writer
                writer
                    .emit_async_span_begin(begin_ns, name, target, filename, line)
                    .await?;
                writer
                    .emit_async_span_end(end_ns, name, target, filename, line)
                    .await?;

                span_count += 1;
                // Flush every 10 async spans to create multiple chunks
                if span_count % 10 == 0 {
                    writer.flush().await?;
                }
            } else {
                warn!("Skipping async span '{}' with invalid duration", name);
            }
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
