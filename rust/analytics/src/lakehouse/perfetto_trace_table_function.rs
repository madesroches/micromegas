use super::{partition_cache::QueryPartitionProvider, view_factory::ViewFactory};
use crate::{
    dfext::expressions::{exp_to_string, exp_to_timestamp},
    time::TimeRange,
};
use datafusion::{
    arrow::datatypes::{DataType, Field, Schema},
    catalog::{TableFunctionImpl, TableProvider},
    common::plan_err,
    execution::runtime_env::RuntimeEnv,
    logical_expr::Expr,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::sync::Arc;

/// `PerfettoTraceTableFunction` generates Perfetto trace chunks from process telemetry data.
///
/// SQL Interface:
/// ```sql
/// SELECT chunk_id, chunk_data
/// FROM perfetto_trace_chunks(
///     'process_id',                              -- Process UUID (required)
///     'span_types',                              -- 'thread', 'async', or 'both' (required)
///     TIMESTAMP '2024-01-01T00:00:00Z',          -- Start time as UTC timestamp (required)
///     TIMESTAMP '2024-01-01T01:00:00Z'           -- End time as UTC timestamp (required)
/// ) ORDER BY chunk_id
/// ```
///
/// Returns a table with schema:
/// - chunk_id: Int32 - Sequential chunk identifier
/// - chunk_data: Binary - Binary protobuf TracePacket data
///
#[derive(Debug)]
pub struct PerfettoTraceTableFunction {
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    object_store: Arc<dyn ObjectStore>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
}

impl PerfettoTraceTableFunction {
    pub fn new(
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        object_store: Arc<dyn ObjectStore>,
        view_factory: Arc<ViewFactory>,
        part_provider: Arc<dyn QueryPartitionProvider>,
    ) -> Self {
        Self {
            runtime,
            lake,
            object_store,
            view_factory,
            part_provider,
        }
    }

    /// Create the output schema for the table function
    pub fn output_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("chunk_id", DataType::Int32, false),
            Field::new("chunk_data", DataType::Binary, false),
        ]))
    }
}

impl TableFunctionImpl for PerfettoTraceTableFunction {
    #[span_fn]
    fn call(&self, exprs: &[Expr]) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        // Parse process_id (arg 1)
        let arg1 = exprs.first().map(exp_to_string);
        let Some(Ok(process_id)) = arg1 else {
            return plan_err!(
                "First argument to perfetto_trace_chunks must be a string (the process ID), given {:?}",
                arg1
            );
        };

        // Parse span_types (arg 2)
        let arg2 = exprs.get(1).map(exp_to_string);
        let Some(Ok(span_types_str)) = arg2 else {
            return plan_err!(
                "Second argument to perfetto_trace_chunks must be a string ('thread', 'async', or 'both'), given {:?}",
                arg2
            );
        };

        let span_types = match span_types_str.as_str() {
            "thread" => SpanTypes::Thread,
            "async" => SpanTypes::Async,
            "both" => SpanTypes::Both,
            _ => {
                return plan_err!(
                    "span_types must be 'thread', 'async', or 'both', given: {}",
                    span_types_str
                );
            }
        };

        // Parse start_time (arg 3) - expecting a timestamp expression
        let arg3 = exprs.get(2).map(exp_to_timestamp);
        let Some(Ok(start_time)) = arg3 else {
            return plan_err!(
                "Third argument to perfetto_trace_chunks must be a timestamp (start time), given {:?}",
                arg3
            );
        };

        // Parse end_time (arg 4) - expecting a timestamp expression
        let arg4 = exprs.get(3).map(exp_to_timestamp);
        let Some(Ok(end_time)) = arg4 else {
            return plan_err!(
                "Fourth argument to perfetto_trace_chunks must be a timestamp (end time), given {:?}",
                arg4
            );
        };

        // Create time range from parsed timestamps
        let time_range = TimeRange {
            begin: start_time,
            end: end_time,
        };

        // Create the execution plan that will generate the trace chunks
        let execution_plan = Arc::new(PerfettoTraceExecutionPlan::new(
            Self::output_schema(),
            process_id,
            span_types,
            time_range,
            self.runtime.clone(),
            self.lake.clone(),
            self.object_store.clone(),
            self.view_factory.clone(),
            self.part_provider.clone(),
        ));

        // Wrap it in a TableProvider
        Ok(Arc::new(PerfettoTraceTableProvider::new(execution_plan)))
    }
}

// Import the execution plan
use super::perfetto_trace_execution_plan::{
    PerfettoTraceExecutionPlan, PerfettoTraceTableProvider, SpanTypes,
};
