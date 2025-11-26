use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::{
        array::{Array, StringArray, StringBuilder, TimestampNanosecondArray},
        datatypes::{DataType, TimeUnit},
    },
    common::internal_err,
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

/// A scalar UDF that retires a single partition by its metadata.
///
/// This function retires partitions by their metadata identifiers (view_set_name,
/// view_instance_id, begin_insert_time, end_insert_time). This works for both empty
/// partitions (file_path=NULL) and non-empty partitions.
///
/// This is the preferred method for retiring partitions as it uses the partition's
/// natural identifiers rather than relying on file paths.
#[derive(Debug)]
pub struct RetirePartitionByMetadata {
    signature: Signature,
    lake: Arc<DataLakeConnection>,
}

impl PartialEq for RetirePartitionByMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.signature == other.signature
    }
}

impl Eq for RetirePartitionByMetadata {}

impl std::hash::Hash for RetirePartitionByMetadata {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.signature.hash(state);
    }
}

impl RetirePartitionByMetadata {
    pub fn new(lake: Arc<DataLakeConnection>) -> Self {
        Self {
            signature: Signature::exact(
                vec![
                    DataType::Utf8,
                    DataType::Utf8,
                    DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                    DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                ],
                Volatility::Volatile,
            ),
            lake,
        }
    }

    /// Retires a single partition by its metadata within an existing transaction.
    ///
    /// # Arguments
    /// * `transaction` - Database transaction to use
    /// * `view_set_name` - The name of the view set
    /// * `view_instance_id` - The instance ID (e.g., process_id or 'global')
    /// * `begin_insert_time` - Begin insert time timestamp
    /// * `end_insert_time` - End insert time timestamp
    ///
    /// # Returns
    /// * `Ok(())` on successful retirement
    /// * `Err(anyhow::Error)` with descriptive message for any failure
    async fn retire_partition_in_transaction(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        view_set_name: &str,
        view_instance_id: &str,
        begin_insert_time: DateTime<Utc>,
        end_insert_time: DateTime<Utc>,
    ) -> Result<()> {
        // First, check if the partition exists and get its details
        let partition_query = sqlx::query(
            "SELECT file_path, file_size FROM lakehouse_partitions 
             WHERE view_set_name = $1 
               AND view_instance_id = $2 
               AND begin_insert_time = $3 
               AND end_insert_time = $4",
        )
        .bind(view_set_name)
        .bind(view_instance_id)
        .bind(begin_insert_time)
        .bind(end_insert_time)
        .fetch_optional(&mut **transaction)
        .await
        .with_context(|| {
            format!(
                "querying partition {view_set_name}/{view_instance_id} [{begin_insert_time}, {end_insert_time})"
            )
        })?;

        let Some(partition_row) = partition_query else {
            anyhow::bail!(
                "Partition not found: {view_set_name}/{view_instance_id} [{begin_insert_time}, {end_insert_time})"
            );
        };

        // Handle file cleanup if file_path is not NULL
        let file_path_opt: Option<String> = partition_row.try_get("file_path")?;
        if let Some(file_path) = file_path_opt {
            let file_size: i64 = partition_row.try_get("file_size")?;
            // Add to temporary files for cleanup (expires in 1 hour)
            add_file_for_cleanup(transaction, &file_path, file_size).await?;
        }

        // Remove from active partitions
        let delete_result = sqlx::query(
            "DELETE FROM lakehouse_partitions 
             WHERE view_set_name = $1 
               AND view_instance_id = $2 
               AND begin_insert_time = $3 
               AND end_insert_time = $4",
        )
        .bind(view_set_name)
        .bind(view_instance_id)
        .bind(begin_insert_time)
        .bind(end_insert_time)
        .execute(&mut **transaction)
        .await
        .with_context(|| {
            format!(
                "deleting partition {view_set_name}/{view_instance_id} [{begin_insert_time}, {end_insert_time})"
            )
        })?;

        if delete_result.rows_affected() == 0 {
            // This shouldn't happen since we checked existence above, but handle it gracefully
            anyhow::bail!(
                "Partition not found during deletion: {view_set_name}/{view_instance_id} [{begin_insert_time}, {end_insert_time})"
            );
        }

        info!(
            "Successfully retired partition: {}/{} [{}, {})",
            view_set_name, view_instance_id, begin_insert_time, end_insert_time
        );
        Ok(())
    }
}

impl ScalarUDFImpl for RetirePartitionByMetadata {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "retire_partition_by_metadata"
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
            "retire_partition_by_metadata can only be called from async contexts".into(),
        ))
    }
}

#[async_trait]
impl AsyncScalarUDFImpl for RetirePartitionByMetadata {
    async fn invoke_async_with_args(
        &self,
        args: ScalarFunctionArgs,
    ) -> datafusion::error::Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 4 {
            return internal_err!(
                "retire_partition_by_metadata expects exactly 4 arguments: view_set_name, view_instance_id, begin_insert_time, end_insert_time"
            );
        }

        let view_set_names: &StringArray =
            args[0].as_any().downcast_ref::<_>().ok_or_else(|| {
                DataFusionError::Execution(
                    "error casting view_set_name argument as StringArray".into(),
                )
            })?;

        let view_instance_ids: &StringArray =
            args[1].as_any().downcast_ref::<_>().ok_or_else(|| {
                DataFusionError::Execution(
                    "error casting view_instance_id argument as StringArray".into(),
                )
            })?;

        let begin_insert_times: &TimestampNanosecondArray =
            args[2].as_any().downcast_ref::<_>().ok_or_else(|| {
                DataFusionError::Execution(
                    "error casting begin_insert_time argument as TimestampNanosecondArray".into(),
                )
            })?;

        let end_insert_times: &TimestampNanosecondArray =
            args[3].as_any().downcast_ref::<_>().ok_or_else(|| {
                DataFusionError::Execution(
                    "error casting end_insert_time argument as TimestampNanosecondArray".into(),
                )
            })?;

        let mut builder = StringBuilder::with_capacity(view_set_names.len(), 64);

        // Use a single transaction for the entire batch
        let mut transaction =
            self.lake.db_pool.begin().await.map_err(|e| {
                DataFusionError::Execution(format!("Failed to begin transaction: {e}"))
            })?;

        let mut success_count = 0;
        let mut has_errors = false;

        // Process each partition in the batch within the same transaction
        for index in 0..view_set_names.len() {
            if view_set_names.is_null(index)
                || view_instance_ids.is_null(index)
                || begin_insert_times.is_null(index)
                || end_insert_times.is_null(index)
            {
                builder.append_value("ERROR: all arguments must be non-null");
                has_errors = true;
                continue;
            }

            let view_set_name = view_set_names.value(index);
            let view_instance_id = view_instance_ids.value(index);
            let begin_insert_time_nanos = begin_insert_times.value(index);
            let end_insert_time_nanos = end_insert_times.value(index);

            // Convert nanoseconds to DateTime<Utc> for proper sqlx binding
            let begin_insert_time = DateTime::from_timestamp_nanos(begin_insert_time_nanos);
            let end_insert_time = DateTime::from_timestamp_nanos(end_insert_time_nanos);

            match self
                .retire_partition_in_transaction(
                    &mut transaction,
                    view_set_name,
                    view_instance_id,
                    begin_insert_time,
                    end_insert_time,
                )
                .await
            {
                Ok(()) => {
                    success_count += 1;
                    builder.append_value(format!(
                        "SUCCESS: Retired partition {view_set_name}/{view_instance_id} [{begin_insert_time}, {end_insert_time})"
                    ));
                }
                Err(e) => {
                    error!(
                        "Failed to retire partition {}/{} [{}, {}): {:?}",
                        view_set_name, view_instance_id, begin_insert_time, end_insert_time, e
                    );
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
            builder.append_value(format!(
                "ROLLED_BACK: All {} previous changes were reverted due to errors in batch",
                success_count
            ));
        } else {
            transaction.commit().await.map_err(|e| {
                DataFusionError::Execution(format!("Failed to commit transaction: {e}"))
            })?;
            info!("Successfully retired {} partitions in batch", success_count);
        }

        Ok(ColumnarValue::Array(Arc::new(builder.finish())))
    }
}

/// Creates a user-defined function to retire a single partition by its metadata.
///
/// This function retires partitions by their metadata identifiers rather than file path,
/// making it suitable for both empty partitions (file_path=NULL) and non-empty partitions.
///
/// # Usage
/// ```sql
/// SELECT retire_partition_by_metadata(
///     'log_entries',
///     'process_123',
///     TIMESTAMP '2024-01-01 00:00:00',
///     TIMESTAMP '2024-01-01 01:00:00'
/// ) as result;
/// ```
///
/// # Returns
/// A string message indicating success or failure:
/// - "SUCCESS: Retired partition <view_set>/<instance> [<begin>, <end>)" on successful retirement
/// - "ERROR: Partition not found: <view_set>/<instance> [<begin>, <end>)" if the partition doesn't exist
/// - "ERROR: Database error: \<details\>" for any database-related failures
pub fn make_retire_partition_by_metadata_udf(
    lake: Arc<DataLakeConnection>,
) -> datafusion::logical_expr::async_udf::AsyncScalarUDF {
    datafusion::logical_expr::async_udf::AsyncScalarUDF::new(Arc::new(
        RetirePartitionByMetadata::new(lake),
    ))
}
