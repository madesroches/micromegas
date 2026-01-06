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

/// An async scalar UDF that deletes duplicate blocks within the query time range.
///
/// A duplicate is defined as multiple rows with the same `block_id`. This function
/// keeps the row with the earliest `insert_time` and deletes all others.
///
/// The time range is passed via the constructor (not as SQL arguments), similar to
/// `ViewInstanceTableFunction`.
#[derive(Debug)]
pub struct DeleteDuplicateBlocks {
    signature: Signature,
    lake: Arc<DataLakeConnection>,
    query_range: Option<TimeRange>,
}

impl PartialEq for DeleteDuplicateBlocks {
    fn eq(&self, other: &Self) -> bool {
        self.signature == other.signature && self.query_range == other.query_range
    }
}

impl Eq for DeleteDuplicateBlocks {}

impl std::hash::Hash for DeleteDuplicateBlocks {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.signature.hash(state);
        self.query_range.hash(state);
    }
}

impl DeleteDuplicateBlocks {
    pub fn new(lake: Arc<DataLakeConnection>, query_range: Option<TimeRange>) -> Self {
        Self {
            signature: Signature::exact(vec![], Volatility::Volatile),
            lake,
            query_range,
        }
    }
}

impl ScalarUDFImpl for DeleteDuplicateBlocks {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "delete_duplicate_blocks"
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
            "delete_duplicate_blocks can only be called from async contexts".into(),
        ))
    }
}

#[async_trait]
impl AsyncScalarUDFImpl for DeleteDuplicateBlocks {
    #[span_fn]
    async fn invoke_async_with_args(
        &self,
        args: ScalarFunctionArgs,
    ) -> datafusion::error::Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if !args.is_empty() {
            return internal_err!("delete_duplicate_blocks expects no arguments");
        }

        let Some(range) = &self.query_range else {
            return internal_err!("delete_duplicate_blocks requires a query time range to be set");
        };

        let mut transaction =
            self.lake.db_pool.begin().await.map_err(|e| {
                DataFusionError::Execution(format!("Failed to begin transaction: {e}"))
            })?;

        // Delete duplicates, keeping the row with the earliest insert_time for each block_id
        let delete_result = sqlx::query(
            "WITH dups AS (
                SELECT block_id, MIN(insert_time) as keep_time
                FROM blocks
                WHERE insert_time >= $1 AND insert_time < $2
                GROUP BY block_id
                HAVING COUNT(*) > 1
            )
            DELETE FROM blocks b
            USING dups d
            WHERE b.block_id = d.block_id
            AND b.insert_time != d.keep_time",
        )
        .bind(range.begin)
        .bind(range.end)
        .execute(&mut *transaction)
        .await
        .map_err(|e| DataFusionError::Execution(format!("Failed to delete duplicates: {e}")))?;

        let deleted_count = delete_result.rows_affected();

        transaction.commit().await.map_err(|e| {
            DataFusionError::Execution(format!("Failed to commit transaction: {e}"))
        })?;

        info!(
            "Deleted {deleted_count} duplicate blocks in range [{}, {})",
            range.begin, range.end
        );

        let mut builder = StringBuilder::with_capacity(1, 64);
        builder.append_value(format!("Deleted {deleted_count} duplicate blocks"));

        Ok(ColumnarValue::Array(Arc::new(builder.finish())))
    }
}

/// Creates a user-defined function to delete duplicate blocks.
///
/// # Usage
/// ```sql
/// -- Time range set via Python client: client.query(sql, begin, end)
/// SELECT delete_duplicate_blocks();
/// -- Returns: "Deleted 42 duplicate blocks"
/// ```
pub fn make_delete_duplicate_blocks_udf(
    lake: Arc<DataLakeConnection>,
    query_range: Option<TimeRange>,
) -> datafusion::logical_expr::async_udf::AsyncScalarUDF {
    datafusion::logical_expr::async_udf::AsyncScalarUDF::new(Arc::new(DeleteDuplicateBlocks::new(
        lake,
        query_range,
    )))
}
