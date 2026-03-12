use super::{
    lakehouse_context::LakehouseContext, partition_cache::QueryPartitionProvider,
    process_streams::get_process_thread_list, session_configurator::NoOpSessionConfigurator,
    view_factory::ViewFactory,
};
use crate::{dfext::expressions::exp_to_string, span_table::get_spans_schema, time::TimeRange};
use async_stream::try_stream;
use datafusion::{
    arrow::{
        array::{ArrayRef, RecordBatch, StringDictionaryBuilder},
        datatypes::{DataType, Field, Int16Type, Schema, SchemaRef},
    },
    catalog::{Session, TableFunctionImpl, TableProvider},
    common::{Result as DFResult, plan_err},
    execution::{SendableRecordBatchStream, TaskContext},
    logical_expr::{Expr, TableType},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
        execution_plan::{Boundedness, EmissionType},
        limit::GlobalLimitExec,
        projection::ProjectionExec,
        stream::RecordBatchStreamAdapter,
    },
};
use futures::{StreamExt, TryStreamExt};
use micromegas_tracing::prelude::*;
use std::{
    any::Any,
    fmt::{self, Debug, Formatter},
    sync::Arc,
};

/// Span types to include in the output
#[derive(Debug, Clone, Copy)]
pub enum SpanTypes {
    Thread,
    Async,
    Both,
}

fn output_schema() -> SchemaRef {
    let mut fields = vec![
        Field::new(
            "stream_id",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "thread_name",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
    ];
    fields.extend(get_spans_schema().fields.iter().map(|f| f.as_ref().clone()));
    Arc::new(Schema::new(fields))
}

fn augment_batch(
    batch: &RecordBatch,
    schema: SchemaRef,
    stream_id: &str,
    thread_name: &str,
) -> DFResult<RecordBatch> {
    let n = batch.num_rows();
    let mut stream_id_builder = StringDictionaryBuilder::<Int16Type>::new();
    let mut thread_name_builder = StringDictionaryBuilder::<Int16Type>::new();
    stream_id_builder.append_values(stream_id, n);
    thread_name_builder.append_values(thread_name, n);
    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(stream_id_builder.finish()),
        Arc::new(thread_name_builder.finish()),
    ];
    columns.extend(batch.columns().iter().cloned());
    RecordBatch::try_new(schema, columns).map_err(Into::into)
}

// --- TableFunction ---

#[derive(Debug)]
pub struct ProcessSpansTableFunction {
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
}

impl ProcessSpansTableFunction {
    pub fn new(
        lakehouse: Arc<LakehouseContext>,
        view_factory: Arc<ViewFactory>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        query_range: Option<TimeRange>,
    ) -> Self {
        Self {
            lakehouse,
            view_factory,
            part_provider,
            query_range,
        }
    }
}

impl TableFunctionImpl for ProcessSpansTableFunction {
    #[span_fn]
    fn call(&self, exprs: &[Expr]) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        let arg1 = exprs.first().map(exp_to_string);
        let Some(Ok(process_id)) = arg1 else {
            return plan_err!(
                "First argument to process_spans must be a string (the process ID), given {:?}",
                arg1
            );
        };

        let arg2 = exprs.get(1).map(exp_to_string);
        let Some(Ok(span_types_str)) = arg2 else {
            return plan_err!(
                "Second argument to process_spans must be a string ('thread', 'async', or 'both'), given {:?}",
                arg2
            );
        };

        let span_types = match span_types_str.as_str() {
            "thread" => SpanTypes::Thread,
            "async" => SpanTypes::Async,
            "both" => SpanTypes::Both,
            _ => {
                return plan_err!(
                    "span_types must be 'thread', 'async', or 'both', given: {span_types_str}"
                );
            }
        };

        let schema = output_schema();
        let execution_plan = Arc::new(ProcessSpansExecutionPlan::new(
            schema,
            process_id,
            span_types,
            self.query_range,
            self.lakehouse.clone(),
            self.view_factory.clone(),
            self.part_provider.clone(),
        ));

        Ok(Arc::new(ProcessSpansTableProvider { execution_plan }))
    }
}

// --- ExecutionPlan ---

pub struct ProcessSpansExecutionPlan {
    schema: SchemaRef,
    process_id: String,
    span_types: SpanTypes,
    query_range: Option<TimeRange>,
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    properties: PlanProperties,
}

impl ProcessSpansExecutionPlan {
    fn new(
        schema: SchemaRef,
        process_id: String,
        span_types: SpanTypes,
        query_range: Option<TimeRange>,
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
            query_range,
            lakehouse,
            view_factory,
            part_provider,
            properties,
        }
    }
}

impl Debug for ProcessSpansExecutionPlan {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessSpansExecutionPlan")
            .field("process_id", &self.process_id)
            .field("span_types", &self.span_types)
            .finish()
    }
}

impl DisplayAs for ProcessSpansExecutionPlan {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "ProcessSpansExecutionPlan: process_id={}, span_types={:?}",
            self.process_id, self.span_types
        )
    }
}

impl ExecutionPlan for ProcessSpansExecutionPlan {
    fn name(&self) -> &str {
        "ProcessSpansExecutionPlan"
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
        let stream_schema = schema.clone();
        let process_id = self.process_id.clone();
        let span_types = self.span_types;
        let query_range = self.query_range;
        let lakehouse = self.lakehouse.clone();
        let view_factory = self.view_factory.clone();
        let part_provider = self.part_provider.clone();

        let record_batch_stream = try_stream! {
            let schema = stream_schema;
            let ctx = super::query::make_session_context(
                lakehouse,
                part_provider,
                query_range,
                view_factory,
                Arc::new(NoOpSessionConfigurator),
            )
            .await
            .map_err(|e| datafusion::error::DataFusionError::Execution(
                format!("Failed to create session context: {e}"),
            ))?;

            // Thread spans
            if matches!(span_types, SpanTypes::Thread | SpanTypes::Both) {
                let threads = get_process_thread_list(&process_id, &ctx)
                    .await
                    .map_err(|e| datafusion::error::DataFusionError::Execution(
                        format!("Failed to get thread list: {e}"),
                    ))?;

                let max_concurrent = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4);

                let queries: Vec<(String, String, String)> = threads
                    .iter()
                    .map(|(stream_id, _thread_id, display_name)| {
                        let sql = format!(
                            "SELECT * FROM view_instance('thread_spans', '{stream_id}')"
                        );
                        (stream_id.clone(), display_name.clone(), sql)
                    })
                    .collect();

                let stream_results: Vec<(String, String, SendableRecordBatchStream)> =
                    futures::stream::iter(queries)
                        .map(|(stream_id, thread_name, sql)| {
                            let ctx = ctx.clone();
                            async move {
                                spawn_with_context(async move {
                                    let df = ctx.sql(&sql).await?;
                                    let s = df.execute_stream().await?;
                                    Ok::<_, anyhow::Error>((stream_id, thread_name, s))
                                })
                                .await?
                            }
                        })
                        .buffered(max_concurrent)
                        .try_collect()
                        .await
                        .map_err(|e| datafusion::error::DataFusionError::Execution(
                            format!("Failed to query thread spans: {e}"),
                        ))?;

                for (stream_id, thread_name, mut data_stream) in stream_results {
                    while let Some(batch) = data_stream.try_next().await? {
                        let augmented = augment_batch(&batch, schema.clone(), &stream_id, &thread_name)?;
                        yield augmented;
                    }
                }
            }

            // Async spans
            if matches!(span_types, SpanTypes::Async | SpanTypes::Both) {
                let async_sql = format!(
                    "SELECT \
                        b.span_id as id, \
                        b.parent_span_id as parent, \
                        b.depth, \
                        b.hash, \
                        b.time as \"begin\", \
                        e.time as \"end\", \
                        arrow_cast(e.time, 'Int64') - arrow_cast(b.time, 'Int64') as duration, \
                        b.name, \
                        b.target, \
                        b.filename, \
                        b.line \
                    FROM (SELECT * FROM view_instance('async_events', '{process_id}') \
                          WHERE event_type = 'begin') b \
                    INNER JOIN (SELECT * FROM view_instance('async_events', '{process_id}') \
                          WHERE event_type = 'end') e \
                    ON b.span_id = e.span_id \
                    WHERE b.time < e.time \
                    ORDER BY b.time"
                );

                let df = ctx.sql(&async_sql).await
                    .map_err(|e| datafusion::error::DataFusionError::Execution(
                        format!("Failed to query async spans: {e}"),
                    ))?;
                let mut async_stream = df.execute_stream().await
                    .map_err(|e| datafusion::error::DataFusionError::Execution(
                        format!("Failed to execute async spans stream: {e}"),
                    ))?;

                while let Some(batch) = async_stream.try_next().await? {
                    let augmented = augment_batch(&batch, schema.clone(), "", "async")?;
                    yield augmented;
                }
            }
        };

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            schema,
            record_batch_stream,
        )))
    }
}

// --- TableProvider ---

#[derive(Debug)]
struct ProcessSpansTableProvider {
    execution_plan: Arc<ProcessSpansExecutionPlan>,
}

#[async_trait::async_trait]
impl TableProvider for ProcessSpansTableProvider {
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
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> DFResult<Arc<dyn ExecutionPlan>> {
        let mut plan: Arc<dyn ExecutionPlan> = self.execution_plan.clone();
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
        if let Some(fetch) = limit {
            plan = Arc::new(GlobalLimitExec::new(plan, 0, Some(fetch)));
        }
        Ok(plan)
    }
}
