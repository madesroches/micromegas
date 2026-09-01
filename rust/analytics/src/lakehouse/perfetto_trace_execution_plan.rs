use super::{
    audience_guard::{AudienceGuard, Authorized, IdKind},
    lakehouse_context::LakehouseContext,
    partition_cache::QueryPartitionProvider,
    process_streams::get_process_thread_list,
    read_scope::CallerContext,
    session_configurator::NoOpSessionConfigurator,
    view_factory::ViewFactory,
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
    error::DataFusionError,
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
    fmt::{self, Debug, Formatter},
    sync::Arc,
};
use uuid::Uuid;

pub use super::process_spans_table_function::SpanTypes;

/// Marker error for a process that could not be found. This is a caller error (the
/// requested process id doesn't exist), not a server-side failure, so it must stay
/// classified as `DataFusionError::Execution` while every other failure inside
/// `generate_streaming_perfetto_trace` (session context, object store, DB, parquet
/// reads, ...) is a genuine internal failure.
#[derive(Debug, thiserror::Error)]
#[error("Process {0} not found")]
struct ProcessNotFoundError(String);

/// Execution plan that generates Perfetto trace chunks
pub struct PerfettoTraceExecutionPlan {
    schema: SchemaRef,
    process_id: String,
    process_uuid: Uuid,
    span_types: SpanTypes,
    time_range: TimeRange,
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    guard: Arc<AudienceGuard>,
    /// Set only by [`PerfettoTraceTableProvider::scan`], after `AudienceGuard::authorize`
    /// succeeds -- see `process_spans_table_function.rs`'s identical field for the full
    /// rationale (fail-closed by construction, not by convention).
    authorized: Option<Authorized>,
    properties: Arc<PlanProperties>,
}

impl PerfettoTraceExecutionPlan {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        schema: SchemaRef,
        process_id: String,
        process_uuid: Uuid,
        span_types: SpanTypes,
        time_range: TimeRange,
        lakehouse: Arc<LakehouseContext>,
        view_factory: Arc<ViewFactory>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        guard: Arc<AudienceGuard>,
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
            process_uuid,
            span_types,
            time_range,
            lakehouse,
            view_factory,
            part_provider,
            guard,
            authorized: None,
            properties: Arc::new(properties),
        }
    }

    /// See `ProcessSpansExecutionPlan::with_authorized`'s identical doc comment.
    fn with_authorized(&self, authorized: Authorized) -> Self {
        Self {
            schema: self.schema.clone(),
            process_id: self.process_id.clone(),
            process_uuid: self.process_uuid,
            span_types: self.span_types,
            time_range: self.time_range,
            lakehouse: self.lakehouse.clone(),
            view_factory: self.view_factory.clone(),
            part_provider: self.part_provider.clone(),
            guard: self.guard.clone(),
            authorized: Some(authorized),
            properties: self.properties.clone(),
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

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn properties(&self) -> &Arc<PlanProperties> {
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
        // No witness ⇒ this plan never went through `scan` -- fail-closed by construction.
        let Some(authorized) = &self.authorized else {
            return Err(DataFusionError::Internal(
                "perfetto_trace_chunks: unauthorized plan (no witness from scan)".into(),
            ));
        };
        let caller = authorized.internal_caller();

        // Create the stream directly without channels
        let stream = generate_perfetto_trace_stream(
            process_id,
            span_types,
            time_range,
            lakehouse,
            view_factory,
            part_provider,
            caller,
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
    caller: CallerContext,
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
                caller,
            ).await
        });

        // Stream chunks as they become available
        while let Some(chunk_result) = chunk_receiver.recv().await {
            match chunk_result {
                Ok(batch) => yield Ok(batch),
                Err(e) => {
                    // ChunkSender only ever pushes `Ok(batch)` into this channel, so
                    // reaching this branch means the channel itself misbehaved - an
                    // internal failure, never a caller mistake.
                    error!("Error in chunk generation: {:?}", e);
                    yield Err(datafusion::error::DataFusionError::Internal(
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
                if e.downcast_ref::<ProcessNotFoundError>().is_some() {
                    // Caller asked for a process id that doesn't exist.
                    warn!("Trace generation failed: {:?}", e);
                    yield Err(datafusion::error::DataFusionError::Execution(
                        format!("Trace generation failed: {}", e)
                    ));
                } else {
                    // Everything else (session context, object store, DB, parquet
                    // reads, ...) is a genuine server-side failure.
                    error!("Trace generation failed: {:?}", e);
                    yield Err(datafusion::error::DataFusionError::Internal(
                        format!("Trace generation failed: {}", e)
                    ));
                }
            }
            Err(e) => {
                error!("Task panicked: {:?}", e);
                yield Err(datafusion::error::DataFusionError::Internal(
                    format!("Task panicked: {}", e)
                ));
            }
        }
    }
}

/// Generate Perfetto trace using streaming architecture
#[allow(clippy::too_many_arguments)]
async fn generate_streaming_perfetto_trace(
    chunk_sender: ChunkSender,
    process_id: String,
    span_types: SpanTypes,
    time_range: TimeRange,
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    caller: CallerContext,
) -> anyhow::Result<()> {
    info!(
        "Generating streaming Perfetto trace for process {} with span types {:?} from {} to {}",
        process_id, span_types, time_range.begin, time_range.end
    );

    // Runs under the witness's internal caller (`caller`, threaded in from `execute`), not the
    // caller's own scope: every SQL statement below is server-constructed and confined to the
    // process id the guard already authorized (`get_process_exe`, `get_process_thread_list`, the
    // `view_instance` calls further down) -- if that process is readable, everything these
    // statements can reach is readable too. A deliberate deviation from naive scope inheritance.
    let ctx = super::query::make_session_context(
        lakehouse,
        part_provider,
        Some(TimeRange {
            begin: time_range.begin,
            end: time_range.end,
        }),
        view_factory,
        Arc::new(NoOpSessionConfigurator),
        caller,
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
        return Err(ProcessNotFoundError(process_id.to_owned()).into());
    }

    let exes = string_column_by_name(&batches[0], "exe")?;
    Ok(exes.value(0)?.to_owned())
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
    for (stream_id, data_stream) in streams {
        write_thread_spans(writer, &stream_id, data_stream).await?;
    }
    Ok(())
}

/// Writes one thread's spans, in row order, from its query result stream to the Perfetto writer.
///
/// `begin` is only guaranteed sorted *within* a single thread's stream (different threads are
/// independent timelines), so the monotonicity tracker below is local to this call. It is the
/// runtime backstop for the ordering `ThreadSpansView::get_scan_output_ordering` declares to
/// DataFusion: once the redundant `Sort` node is elided, DataFusion trusts that declared ordering
/// and never re-validates it against the actual rows, so a violated invariant would otherwise
/// silently mis-order the exported trace. Errors instead of writing a row whose `begin` regresses.
pub async fn write_thread_spans(
    writer: &mut PerfettoWriter,
    stream_id: &str,
    mut data_stream: SendableRecordBatchStream,
) -> anyhow::Result<()> {
    writer.set_current_thread(stream_id);

    let mut previous_begin_ns: Option<i64> = None;
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
            let begin_time = begin_times.value(i);
            if let Some(previous) = previous_begin_ns
                && begin_time < previous
            {
                anyhow::bail!(
                    "thread spans out of order for stream {stream_id}: begin {begin_time} follows {previous}"
                );
            }
            previous_begin_ns = Some(begin_time);

            let begin_ns = begin_time as u64;
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
    /// `pub(crate)`, not `pub`: only ever called from `perfetto_trace_table_function.rs`, same
    /// crate. So an external test crate can't build an un-authorized plan directly, matching
    /// `process_spans`' existing module-private shape.
    pub(crate) fn new(execution_plan: Arc<PerfettoTraceExecutionPlan>) -> Self {
        Self { execution_plan }
    }
}

#[async_trait::async_trait]
impl TableProvider for PerfettoTraceTableProvider {
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
        let authorized = self
            .execution_plan
            .guard
            .authorize(
                self.execution_plan.process_uuid,
                IdKind::Process,
                "perfetto_trace_chunks",
            )
            .await?;
        // Wrap the execution plan in a GlobalLimitExec if a limit is provided.
        // DataFusion trusts us to apply the limit - if we ignore it, too many rows
        // will be returned to the client.
        let plan: Arc<dyn ExecutionPlan> =
            Arc::new(self.execution_plan.with_authorized(authorized));
        if let Some(fetch) = limit {
            Ok(Arc::new(GlobalLimitExec::new(plan, 0, Some(fetch))))
        } else {
            Ok(plan)
        }
    }
}
