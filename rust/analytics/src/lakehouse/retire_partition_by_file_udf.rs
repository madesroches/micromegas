use anyhow::{Context, Result};
use async_trait::async_trait;
use datafusion::{
    arrow::{
        array::{Array, ArrayRef, StringArray, StringBuilder},
        datatypes::DataType,
    },
    common::internal_err,
    config::ConfigOptions,
    error::DataFusionError,
    logical_expr::{
        ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
        async_udf::AsyncScalarUDFImpl,
    },
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::Row;
use std::sync::Arc;

use super::write_partition::add_file_for_cleanup;

/// A scalar UDF that retires a single partition by its file path.
///
/// This function provides surgical precision for partition retirement,
/// ensuring only the exact specified partition is removed from the lakehouse.
#[derive(Debug)]
pub struct RetirePartitionByFile {
    signature: Signature,
    lake: Arc<DataLakeConnection>,
}

impl RetirePartitionByFile {
    pub fn new(lake: Arc<DataLakeConnection>) -> Self {
        Self {
            signature: Signature::exact(vec![DataType::Utf8], Volatility::Volatile),
            lake,
        }
    }

    /// Retires a single partition by its file path within an existing transaction.
    ///
    /// # Arguments
    /// * `transaction` - Database transaction to use
    /// * `file_path` - The exact file path of the partition to retire
    ///
    /// # Returns
    /// * `Ok(())` on successful retirement
    /// * `Err(anyhow::Error)` with descriptive message for any failure
    async fn retire_partition_in_transaction(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        file_path: &str,
    ) -> Result<()> {
        // First, check if the partition exists and get its details
        let partition_query = sqlx::query(
            "SELECT file_path, file_size FROM lakehouse_partitions WHERE file_path = $1",
        )
        .bind(file_path)
        .fetch_optional(&mut **transaction)
        .await
        .with_context(|| format!("querying partition {file_path}"))?;

        let Some(partition_row) = partition_query else {
            anyhow::bail!("Partition not found: {file_path}");
        };

        let file_size: i64 = partition_row.try_get("file_size")?;

        // Add to temporary files for cleanup (expires in 1 hour)
        add_file_for_cleanup(transaction, file_path, file_size).await?;

        // Remove from active partitions
        let delete_result = sqlx::query("DELETE FROM lakehouse_partitions WHERE file_path = $1")
            .bind(file_path)
            .execute(&mut **transaction)
            .await
            .with_context(|| format!("deleting partition {file_path}"))?;

        if delete_result.rows_affected() == 0 {
            // This shouldn't happen since we checked existence above, but handle it gracefully
            anyhow::bail!("Partition not found during deletion: {file_path}");
        }

        info!("Successfully retired partition: {}", file_path);
        Ok(())
    }
}

impl ScalarUDFImpl for RetirePartitionByFile {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "retire_partition_by_file"
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
            "retire_partition_by_file can only be called from async contexts".into(),
        ))
    }
}

#[async_trait]
impl AsyncScalarUDFImpl for RetirePartitionByFile {
    async fn invoke_async_with_args(
        &self,
        args: ScalarFunctionArgs,
        _config: &ConfigOptions,
    ) -> datafusion::error::Result<ArrayRef> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return internal_err!("retire_partition_by_file expects exactly 1 argument: file_path");
        }

        let file_paths: &StringArray = args[0].as_any().downcast_ref::<_>().ok_or_else(|| {
            DataFusionError::Execution("error casting file_path argument as StringArray".into())
        })?;

        let mut builder = StringBuilder::with_capacity(file_paths.len(), 64);

        // Use a single transaction for the entire batch
        let mut transaction =
            self.lake.db_pool.begin().await.map_err(|e| {
                DataFusionError::Execution(format!("Failed to begin transaction: {e}"))
            })?;

        let mut success_count = 0;
        let mut has_errors = false;

        // Process each file path in the batch within the same transaction
        for index in 0..file_paths.len() {
            if file_paths.is_null(index) {
                builder.append_value("ERROR: file_path cannot be null");
                has_errors = true;
                continue;
            }

            let file_path = file_paths.value(index);

            match self
                .retire_partition_in_transaction(&mut transaction, file_path)
                .await
            {
                Ok(()) => {
                    success_count += 1;
                    builder.append_value(format!("SUCCESS: Retired partition {file_path}"));
                }
                Err(e) => {
                    error!("Failed to retire partition {}: {:?}", file_path, e);
                    builder.append_value(format!("ERROR: {e:?}"));
                    has_errors = true;
                }
            }
        }

        // Commit the transaction only if there were no errors
        if has_errors {
            if let Err(e) = transaction.rollback().await {
                error!("Failed to rollback transaction after errors: {:?}", e);
            }
            info!("Rolled back transaction due to errors in batch retirement");
        } else {
            transaction.commit().await.map_err(|e| {
                DataFusionError::Execution(format!("Failed to commit transaction: {e}"))
            })?;
            info!("Successfully retired {} partitions in batch", success_count);
        }

        Ok(Arc::new(builder.finish()))
    }
}

/// Creates a user-defined function to retire a single partition by its file path.
///
/// This function provides surgical precision for partition retirement, ensuring
/// only the exact specified partition is removed from the lakehouse.
///
/// # Usage
/// ```sql
/// SELECT retire_partition_by_file('/path/to/partition.parquet') as result;
/// ```
///
/// # Returns
/// A string message indicating success or failure:
/// - "SUCCESS: Retired partition <file_path>" on successful retirement
/// - "ERROR: Partition not found: <file_path>" if the partition doesn't exist  
/// - "ERROR: Database error: <details>" for any database-related failures
pub fn make_retire_partition_by_file_udf(
    lake: Arc<DataLakeConnection>,
) -> datafusion::logical_expr::async_udf::AsyncScalarUDF {
    datafusion::logical_expr::async_udf::AsyncScalarUDF::new(Arc::new(RetirePartitionByFile::new(
        lake,
    )))
}
