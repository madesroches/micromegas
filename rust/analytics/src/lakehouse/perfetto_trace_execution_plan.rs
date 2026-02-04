use super::{
    lakehouse_context::LakehouseContext, partition_cache::QueryPartitionProvider,
    session_configurator::NoOpSessionConfigurator, view_factory::ViewFactory,
};
use crate::dfext::{
    string_column_accessor::string_column_by_name, typed_column::typed_column_by_name,
};
use crate::time::TimeRange;
use async_stream::stream;
use datafusion::{
    arrow::{
        array::{RecordBatch, TimestampNanosecondArray, UInt32Array},
        datatypes::SchemaRef,
    },
    catalog::{Session, TableProvider},
    common::Result as DFResult,
    execution::{SendableRecordBatchStream, TaskContext},
    logical_expr::{Expr, TableType},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
        execution_plan::{Boundedness, EmissionType},
        limit::GlobalLimitExec,
        stream::RecordBatchStreamAdapter,
    },
};
use futures::{StreamExt, TryStreamExt, stream};
use micromegas_perfetto::{chunk_sender::ChunkSender, streaming_writer::PerfettoWriter};
use micromegas_tracing::prelude::*;
use std::{
    any::Any,
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
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    properties: PlanProperties,
}

impl PerfettoTraceExecutionPlan {
    pub fn new(
        schema: SchemaRef,
        process_id: String,
        span_types: SpanTypes,
        time_range: TimeRange,
        lakehouse: Arc<LakehouseContext>,
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
            lakehouse,
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

	#[span_fn]
    fn execute(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> DFResult<SendableRecordBatchStream> {
        let schema = self.schema.clone();
        let process_id = self.process_id.clone();
        let span_types = self.span_types;
        let time_range = self.time_range;
        let lakehouse = self.lakehouse.clone();
        let view_factory = self.view_factory.clone();
        let part_provider = self.part_provider.clone();

        // Create the stream directly without channels
        let stream = generate_perfetto_trace_stream(
            process_id,
            span_types,
            time_range,
            lakehouse,
            view_factory,
            part_provider,
        );

        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
    }
}

/// Creates a stream of Perfetto trace chunks using streaming architecture
#[span_fn]
fn generate_perfetto_trace_stream(
    process_id: String,
    span_types: SpanTypes,
    time_range: TimeRange,
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
) -> impl futures::Stream<Item = DFResult<RecordBatch>> {
    stream! {
        // Create channel for streaming chunks
        const CHUNK_SIZE: usize = 8 * 1024; // 8KB chunks
        let (chunk_sender, mut chunk_receiver) = tokio::sync::mpsc::channel(16);

        // Create ChunkSender that will stream data through the channel
        let chunk_sender_writer = ChunkSender::new(chunk_sender, CHUNK_SIZE);

        // Spawn background task to generate trace
        let generation_task = spawn_with_context(async move {
            generate_streaming_perfetto_trace(
                chunk_sender_writer,
                process_id,
                span_types,
                time_range,
                lakehouse,
                view_factory,
                part_provider,
            ).await
        });

        // Stream chunks as they become available
        while let Some(chunk_result) = chunk_receiver.recv().await {
            match chunk_result {
                Ok(batch) => yield Ok(batch),
                Err(e) => {
                    error!("Error in chunk generation: {:?}", e);
                    yield Err(datafusion::error::DataFusionError::Execution(
                        format!("Chunk generation failed: {}", e)
                    ));
                    return;
                }
            }
        }

        // Wait for generation task to complete and check for errors
        match generation_task.await {
            Ok(Ok(())) => {}, // Success
            Ok(Err(e)) => {
                error!("Trace generation failed: {:?}", e);
                yield Err(datafusion::error::DataFusionError::Execution(
                    format!("Trace generation failed: {}", e)
                ));
            }
            Err(e) => {
                error!("Task panicked: {:?}", e);
                yield Err(datafusion::error::DataFusionError::Execution(
                    format!("Task panicked: {}", e)
                ));
            }
        }
    }
}

/// Generate Perfetto trace using streaming architecture
async fn generate_streaming_perfetto_trace(
    chunk_sender: ChunkSender,
    process_id: String,
    span_types: SpanTypes,
    time_range: TimeRange,
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
) -> anyhow::Result<()> {
    info!(
        "Generating streaming Perfetto trace for process {} with span types {:?} from {} to {}",
        process_id, span_types, time_range.begin, time_range.end
    );

    // Create a context for making queries
    let ctx = super::query::make_session_context(
        lakehouse,
        part_provider,
        Some(TimeRange {
            begin: time_range.begin,
            end: time_range.end,
        }),
        view_factory,
        Arc::new(NoOpSessionConfigurator),
    )
    .await?;

    // Use ChunkSender directly as the writer destination
    let mut writer = PerfettoWriter::new(Box::new(chunk_sender), &process_id);

    let process_exe = get_process_exe(&process_id, &ctx).await?;
    writer.emit_process_descriptor(&process_exe).await?;
    writer.flush().await?; // Forces chunk emission

    let threads = get_process_thread_list(&process_id, &ctx).await?;
    for (stream_id, thread_id, thread_name) in &threads {
        writer
            .emit_thread_descriptor(stream_id, *thread_id, thread_name)
            .await?;
    }
    if !threads.is_empty() {
        writer.flush().await?; // Forces chunk emission
    }

    if matches!(span_types, SpanTypes::Async | SpanTypes::Both) {
        writer.emit_async_track_descriptor().await?;
        writer.flush().await?; // Forces chunk emission
    }

    if matches!(span_types, SpanTypes::Thread | SpanTypes::Both) {
        generate_thread_spans_with_writer(&mut writer, &ctx, &time_range, &threads).await?;
    }

    if matches!(span_types, SpanTypes::Async | SpanTypes::Both) {
        generate_async_spans_with_writer(&mut writer, &process_id, &ctx, &time_range).await?;
    }

    writer.flush().await?; // Final chunk - this handles the chunk_sender.flush() internally
    Ok(())
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

    let exes = string_column_by_name(&batches[0], "exe")?;
    Ok(exes.value(0)?.to_owned())
}

/// Get thread information from the streams table
/// Returns (stream_id, thread_id_numeric, display_name) where display_name is "name-id" or just "id"
async fn get_process_thread_list(
    process_id: &str,
    ctx: &datafusion::execution::context::SessionContext,
) -> anyhow::Result<Vec<(String, i32, String)>> {
    // Query blocks table to get streams, then filter by checking if thread spans exist
    let sql = format!(
        r#"
        SELECT b.stream_id,
               property_get("streams.properties", 'thread-name') as thread_name,
               property_get("streams.properties", 'thread-id') as thread_id
        FROM blocks b
        WHERE b.process_id = '{}'
        AND array_has(b."streams.tags", 'cpu')
        GROUP BY stream_id, thread_name, thread_id
        ORDER BY stream_id
        "#,
        process_id
    );

    let df = ctx.sql(&sql).await?;
    let batches = df.collect().await?;
    let mut threads = Vec::new();

    for batch in batches {
        let stream_ids = string_column_by_name(&batch, "stream_id")?;
        let thread_names = string_column_by_name(&batch, "thread_name")?;
        let thread_ids = string_column_by_name(&batch, "thread_id")?;

        for i in 0..batch.num_rows() {
            let stream_id = stream_ids.value(i)?.to_owned();
            let thread_name = thread_names.value(i)?;
            let thread_id_str = thread_ids.value(i)?;

            // thread_id falls back to stream_id if not available
            let thread_id_for_display = if thread_id_str.is_empty() {
                &stream_id
            } else {
                thread_id_str
            };

            // Parse numeric thread_id for Perfetto (use 0 if not parseable)
            let thread_id_numeric = thread_id_str.parse::<i64>().unwrap_or(0) as i32;

            // Build display name: "name-id" if name exists, otherwise just "id"
            let display_name = if thread_name.is_empty() {
                thread_id_for_display.to_owned()
            } else {
                format!("{}-{}", thread_name, thread_id_for_display)
            };

            threads.push((stream_id, thread_id_numeric, display_name));
        }
    }

    Ok(threads)
}

/// Format the SQL query for thread spans
fn format_thread_spans_query(stream_id: &str, time_range: &TimeRange) -> String {
    format!(
        r#"
        SELECT "begin", "end", name, filename, target, line
        FROM view_instance('thread_spans', '{}')
        WHERE begin <= TIMESTAMP '{}'
          AND end >= TIMESTAMP '{}'
        ORDER BY begin
        "#,
        stream_id,
        time_range.end.to_rfc3339(),
        time_range.begin.to_rfc3339()
    )
}

/// Generate thread spans with parallel JIT and sequential writing.
///
/// JIT partition locking is per-(view_set_name, view_instance_id), and each thread
/// has a unique stream_id used as view_instance_id. This means different threads
/// get different lock keys, making parallel JIT safe.
///
/// Strategy:
/// - Spawn tasks with spawn_with_context() for true multi-threaded parallelism
/// - Use buffered() to limit concurrent spawned tasks
/// - Collect all streams preserving order
/// - Consume streams sequentially to write each thread's spans together
async fn generate_thread_spans_with_writer(
    writer: &mut PerfettoWriter,
    ctx: &datafusion::execution::context::SessionContext,
    time_range: &TimeRange,
    threads: &[(String, i32, String)],
) -> anyhow::Result<()> {
    let max_concurrent = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // Prepare query inputs upfront
    let queries: Vec<(String, String)> = threads
        .iter()
        .map(|(stream_id, _, _)| {
            (
                stream_id.clone(),
                format_thread_spans_query(stream_id, time_range),
            )
        })
        .collect();

    // Build streams in parallel using spawn for true multi-threading
    let streams: Vec<(String, SendableRecordBatchStream)> = stream::iter(queries)
        .map(|(stream_id, sql)| {
            let ctx = ctx.clone();
            async move {
                spawn_with_context(async move {
                    let df = ctx.sql(&sql).await?;
                    let stream = df.execute_stream().await?;
                    Ok::<_, anyhow::Error>((stream_id, stream))
                })
                .await?
            }
        })
        .buffered(max_concurrent)
        .try_collect()
        .await?;

    // Consume streams sequentially - each thread's spans written together
    for (stream_id, mut data_stream) in streams {
        writer.set_current_thread(&stream_id);

        let mut span_count = 0;
        while let Some(batch_result) = data_stream.next().await {
            let batch = batch_result?;
            let begin_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "begin")?;
            let end_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "end")?;
            let names = string_column_by_name(&batch, "name")?;
            let filenames = string_column_by_name(&batch, "filename")?;
            let targets = string_column_by_name(&batch, "target")?;
            let lines: &UInt32Array = typed_column_by_name(&batch, "line")?;

            for i in 0..batch.num_rows() {
                let begin_ns = begin_times.value(i) as u64;
                let end_ns = end_times.value(i) as u64;
                let name = names.value(i)?;
                let filename = filenames.value(i)?;
                let target = targets.value(i)?;
                let line = lines.value(i);

                writer
                    .emit_span(begin_ns, end_ns, name, target, filename, line)
                    .await?;

                span_count += 1;
                if span_count % 10 == 0 {
                    writer.flush().await?;
                }
            }
        }
    }
    Ok(())
}

/// Generate async spans using the provided PerfettoWriter
async fn generate_async_spans_with_writer(
    writer: &mut PerfettoWriter,
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
        let names = string_column_by_name(&batch, "name")?;
        let filenames = string_column_by_name(&batch, "filename")?;
        let targets = string_column_by_name(&batch, "target")?;
        let lines: &UInt32Array = typed_column_by_name(&batch, "line")?;
        for i in 0..batch.num_rows() {
            let _span_id = span_ids.value(i);
            let begin_ns = begin_times.value(i) as u64;
            let end_ns = end_times.value(i) as u64;
            let name = names.value(i)?;
            let filename = filenames.value(i)?;
            let target = targets.value(i)?;
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
                warn!("Skipping async span with invalid duration");
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
        limit: Option<usize>,
    ) -> DFResult<Arc<dyn ExecutionPlan>> {
        // Wrap the execution plan in a GlobalLimitExec if a limit is provided.
        // DataFusion trusts us to apply the limit - if we ignore it, too many rows
        // will be returned to the client.
        let plan: Arc<dyn ExecutionPlan> = self.execution_plan.clone();
        if let Some(fetch) = limit {
            Ok(Arc::new(GlobalLimitExec::new(plan, 0, Some(fetch))))
        } else {
            Ok(plan)
        }
    }
}
