use crate::time::TimeRange;
use async_trait::async_trait;
use datafusion::{
    arrow::{array::StringBuilder, datatypes::DataType},
    common::internal_err,
    error::DataFusionError,
    logical_expr::{
        ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
        async_udf::AsyncScalarUDFImpl,
    },
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// An async scalar UDF that deletes duplicate processes within the query time range.
///
/// A duplicate is defined as multiple rows with the same `process_id`. This function
/// keeps the row with the earliest `insert_time` and deletes all others.
///
/// The time range is passed via the constructor (not as SQL arguments), similar to
/// `ViewInstanceTableFunction`.
#[derive(Debug)]
pub struct DeleteDuplicateProcesses {
    signature: Signature,
    lake: Arc<DataLakeConnection>,
    query_range: Option<TimeRange>,
}

impl PartialEq for DeleteDuplicateProcesses {
    fn eq(&self, other: &Self) -> bool {
        self.signature == other.signature && self.query_range == other.query_range
    }
}

impl Eq for DeleteDuplicateProcesses {}

impl std::hash::Hash for DeleteDuplicateProcesses {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.signature.hash(state);
        self.query_range.hash(state);
    }
}

impl DeleteDuplicateProcesses {
    pub fn new(lake: Arc<DataLakeConnection>, query_range: Option<TimeRange>) -> Self {
        Self {
            signature: Signature::exact(vec![], Volatility::Volatile),
            lake,
            query_range,
        }
    }
}

impl ScalarUDFImpl for DeleteDuplicateProcesses {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "delete_duplicate_processes"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> datafusion::error::Result<DataType> {
        Ok(DataType::Utf8)
    }

    fn invoke_with_args(
        &self,
        _args: ScalarFunctionArgs,
    ) -> datafusion::error::Result<ColumnarValue> {
        Err(DataFusionError::NotImplemented(
            "delete_duplicate_processes can only be called from async contexts".into(),
        ))
    }
}

#[async_trait]
impl AsyncScalarUDFImpl for DeleteDuplicateProcesses {
    #[span_fn]
    async fn invoke_async_with_args(
        &self,
        args: ScalarFunctionArgs,
    ) -> datafusion::error::Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if !args.is_empty() {
            return internal_err!("delete_duplicate_processes expects no arguments");
        }

        let Some(range) = &self.query_range else {
            return internal_err!(
                "delete_duplicate_processes requires a query time range to be set"
            );
        };

        let deleted_count = delete_duplicate_processes(&self.lake.db_pool, *range)
            .await
            .map_err(|e| DataFusionError::Execution(format!("Failed to delete duplicates: {e}")))?;

        let mut builder = StringBuilder::with_capacity(1, 64);
        builder.append_value(format!("Deleted {deleted_count} duplicate processes"));

        Ok(ColumnarValue::Array(Arc::new(builder.finish())))
    }
}

/// Creates a user-defined function to delete duplicate processes.
///
/// # Usage
/// ```sql
/// -- Time range set via Python client: client.query(sql, begin, end)
/// SELECT delete_duplicate_processes();
/// -- Returns: "Deleted 42 duplicate processes"
/// ```
pub fn make_delete_duplicate_processes_udf(
    lake: Arc<DataLakeConnection>,
    query_range: Option<TimeRange>,
) -> datafusion::logical_expr::async_udf::AsyncScalarUDF {
    datafusion::logical_expr::async_udf::AsyncScalarUDF::new(Arc::new(
        DeleteDuplicateProcesses::new(lake, query_range),
    ))
}

/// Deletes duplicate processes within the specified time range.
///
/// A duplicate is defined as multiple rows with the same `process_id`. This function
/// keeps the row with the earliest `insert_time` and deletes all others.
///
/// Returns the number of deleted duplicate processes.
#[span_fn]
pub async fn delete_duplicate_processes(
    pool: &sqlx::PgPool,
    time_range: TimeRange,
) -> anyhow::Result<u64> {
    let mut transaction = pool.begin().await?;

    // Delete duplicates, keeping the row with the earliest insert_time for each process_id.
    // Note: The time range is used to identify process_ids that have duplicates, but once
    // identified, ALL duplicate rows for that process_id are deleted regardless of their
    // insert_time. This ensures complete cleanup of duplicates even if some copies
    // exist outside the query range.
    let delete_result = sqlx::query(
        "WITH dups AS (
            SELECT process_id, MIN(insert_time) as keep_time
            FROM processes
            WHERE insert_time >= $1 AND insert_time < $2
            GROUP BY process_id
            HAVING COUNT(*) > 1
        )
        DELETE FROM processes p
        USING dups d
        WHERE p.process_id = d.process_id
        AND p.insert_time != d.keep_time",
    )
    .bind(time_range.begin)
    .bind(time_range.end)
    .execute(&mut *transaction)
    .await?;

    let deleted_count = delete_result.rows_affected();

    transaction.commit().await?;

    if deleted_count > 0 {
        info!(
            "Deleted {deleted_count} duplicate processes in range [{}, {})",
            time_range.begin, time_range.end
        );
    }

    Ok(deleted_count)
}
