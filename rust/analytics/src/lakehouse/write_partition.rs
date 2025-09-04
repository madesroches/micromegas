use crate::{
    arrow_utils::serialize_parquet_metadata, lakehouse::async_parquet_writer::AsyncParquetWriter,
    response_writer::Logger, time::TimeRange,
};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::{array::RecordBatch, datatypes::Schema},
    parquet::{
        arrow::{ArrowSchemaConverter, AsyncArrowWriter},
        basic::Compression,
        file::{
            metadata::{ParquetMetaData, RowGroupMetaData},
            properties::{WriterProperties, WriterVersion},
        },
        schema::types::SchemaDescriptor,
    },
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use object_store::buffered::BufWriter;
use sqlx::Row;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{atomic::AtomicI64, Arc};
use tokio::sync::mpsc::Receiver;

use super::{partition::Partition, view::ViewMetadata};

/// A set of rows for a partition, along with their time range.
pub struct PartitionRowSet {
    pub rows_time_range: TimeRange,
    pub rows: RecordBatch,
}

impl PartitionRowSet {
    pub fn new(rows_time_range: TimeRange, rows: RecordBatch) -> Self {
        Self {
            rows_time_range,
            rows,
        }
    }
}

/// Retires partitions that have exceeded their expiration time.
pub async fn retire_expired_partitions(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<()> {
    let mut transaction = lake.db_pool.begin().await?;
    let old_partitions = sqlx::query(
        "SELECT file_path, file_size
         FROM lakehouse_partitions
         WHERE end_insert_time < $1
         ;",
    )
    .bind(expiration)
    .fetch_all(&mut *transaction)
    .await
    .with_context(|| "listing expired partitions")?;

    for old_part in old_partitions {
        let file_path: String = old_part.try_get("file_path")?;
        let file_size: i64 = old_part.try_get("file_size")?;
        let temp_expiration =
            Utc::now() + TimeDelta::try_hours(1).with_context(|| "making one hour")?;
        info!("adding out of date partition {file_path} to temporary files to be deleted");
        sqlx::query("INSERT INTO temporary_files VALUES ($1, $2, $3);")
            .bind(file_path)
            .bind(file_size)
            .bind(temp_expiration)
            .execute(&mut *transaction)
            .await
            .with_context(|| "adding old partition to temporary files to be deleted")?;
    }

    sqlx::query(
        "DELETE from lakehouse_partitions
         WHERE end_insert_time < $1
         ;",
    )
    .bind(expiration)
    .execute(&mut *transaction)
    .await
    .with_context(|| "deleting expired partitions")?;
    transaction.commit().await.with_context(|| "commit")?;
    Ok(())
}

/// Retires partitions from the active set.
/// Overlap is determined by the insert_time of the telemetry.
pub async fn retire_partitions(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    view_set_name: &str,
    view_instance_id: &str,
    begin_insert_time: DateTime<Utc>,
    end_insert_time: DateTime<Utc>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    // this is not an overlap test, we need to assume that we are not making a new smaller partition
    // where a bigger one existed
    // its gets tricky in the jit case where a partition can have only one block and begin_insert == end_insert

    //todo: use DELETE+RETURNING
    let old_partitions = if begin_insert_time == end_insert_time {
        // For identical timestamps, look for exact matches to handle single-block partitions
        sqlx::query(
            "SELECT file_path, file_size
             FROM lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND begin_insert_time = $3
             AND end_insert_time = $3
             ;",
        )
        .bind(view_set_name)
        .bind(view_instance_id)
        .bind(begin_insert_time)
        .fetch_all(&mut **transaction)
        .await
        .with_context(|| "listing old partitions (exact match)")?
    } else {
        // For time ranges, use inclusive inequalities
        sqlx::query(
            "SELECT file_path, file_size
             FROM lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND begin_insert_time >= $3
             AND end_insert_time <= $4
             ;",
        )
        .bind(view_set_name)
        .bind(view_instance_id)
        .bind(begin_insert_time)
        .bind(end_insert_time)
        .fetch_all(&mut **transaction)
        .await
        .with_context(|| "listing old partitions (range)")?
    };

    // LOG: Found partitions for retirement
    logger
        .write_log_entry(format!(
            "[RETIRE_FOUND] view={}/{} time_range=[{}, {}] found_partitions={}",
            view_set_name,
            view_instance_id,
            begin_insert_time,
            end_insert_time,
            old_partitions.len()
        ))
        .await?;
    for old_part in old_partitions {
        let file_path: String = old_part.try_get("file_path")?;
        let file_size: i64 = old_part.try_get("file_size")?;
        let expiration = Utc::now() + TimeDelta::try_hours(1).with_context(|| "making one hour")?;
        logger
            .write_log_entry(format!(
                "adding out of date partition {file_path} to temporary files to be deleted"
            ))
            .await?;
        sqlx::query("INSERT INTO temporary_files VALUES ($1, $2, $3);")
            .bind(file_path)
            .bind(file_size)
            .bind(expiration)
            .execute(&mut **transaction)
            .await
            .with_context(|| "adding old partition to temporary files to be deleted")?;
    }

    if begin_insert_time == end_insert_time {
        // For identical timestamps, delete exact matches to handle single-block partitions
        sqlx::query(
            "DELETE from lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND begin_insert_time = $3
             AND end_insert_time = $3
             ;",
        )
        .bind(view_set_name)
        .bind(view_instance_id)
        .bind(begin_insert_time)
        .execute(&mut **transaction)
        .await
        .with_context(|| "deleting out of date partitions (exact match)")?
    } else {
        // For time ranges, use inclusive inequalities
        sqlx::query(
            "DELETE from lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND begin_insert_time >= $3
             AND end_insert_time <= $4
             ;",
        )
        .bind(view_set_name)
        .bind(view_instance_id)
        .bind(begin_insert_time)
        .bind(end_insert_time)
        .execute(&mut **transaction)
        .await
        .with_context(|| "deleting out of date partitions (range)")?
    };
    Ok(())
}

/// Generate a deterministic advisory lock key for a partition
fn generate_partition_lock_key(
    view_set_name: &str,
    view_instance_id: &str,
    begin_insert_time: DateTime<Utc>,
    end_insert_time: DateTime<Utc>,
) -> i64 {
    let mut hasher = DefaultHasher::new();
    view_set_name.hash(&mut hasher);
    view_instance_id.hash(&mut hasher);
    begin_insert_time.hash(&mut hasher);
    end_insert_time.hash(&mut hasher);
    hasher.finish() as i64
}

async fn write_partition_metadata_attempt(
    lake: &DataLakeConnection,
    partition: &Partition,
    file_metadata: &Arc<ParquetMetaData>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let file_metadata_buffer: Vec<u8> = serialize_parquet_metadata(file_metadata)
        .with_context(|| "serialize_parquet_metadata")?
        .into();

    // Generate deterministic lock key for this partition
    let lock_key = generate_partition_lock_key(
        &partition.view_metadata.view_set_name,
        &partition.view_metadata.view_instance_id,
        partition.begin_insert_time,
        partition.end_insert_time,
    );

    let mut transaction = lake.db_pool.begin().await?;

    debug!(
        "[PARTITION_LOCK] view={}/{} time_range=[{}, {}] lock_key={} - acquiring advisory lock",
        &partition.view_metadata.view_set_name,
        &partition.view_metadata.view_instance_id,
        partition.begin_insert_time,
        partition.end_insert_time,
        lock_key
    );

    // Acquire advisory lock - this will block until we can proceed
    // pg_advisory_xact_lock automatically releases when transaction ends
    sqlx::query("SELECT pg_advisory_xact_lock($1);")
        .bind(lock_key)
        .execute(&mut *transaction)
        .await
        .with_context(|| "acquiring advisory lock")?;

    // LOG: Lock acquired, starting partition write
    logger
        .write_log_entry(format!(
            "[PARTITION_WRITE_START] view={}/{} time_range=[{}, {}] source_hash={:?} - lock acquired",
            &partition.view_metadata.view_set_name,
            &partition.view_metadata.view_instance_id,
            partition.begin_insert_time,
            partition.end_insert_time,
            partition.source_data_hash
        ))
        .await?;

    // for jit partitions, we assume that the blocks were registered in order
    // since they are built based on begin_ticks, not insert_time
    retire_partitions(
        &mut transaction,
        &partition.view_metadata.view_set_name,
        &partition.view_metadata.view_instance_id,
        partition.begin_insert_time,
        partition.end_insert_time,
        logger.clone(),
    )
    .await
    .with_context(|| "retire_partitions")?;

    debug!(
        "[PARTITION_INSERT_ATTEMPT] view={}/{} time_range=[{}, {}] source_hash={:?} file_path={}",
        &partition.view_metadata.view_set_name,
        &partition.view_metadata.view_instance_id,
        partition.begin_insert_time,
        partition.end_insert_time,
        partition.source_data_hash,
        partition.file_path
    );

    // Insert the new partition
    let insert_result = sqlx::query(
        "INSERT INTO lakehouse_partitions VALUES($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13);",
    )
    .bind(&*partition.view_metadata.view_set_name)
    .bind(&*partition.view_metadata.view_instance_id)
    .bind(partition.begin_insert_time)
    .bind(partition.end_insert_time)
    .bind(partition.min_event_time)
    .bind(partition.max_event_time)
    .bind(partition.updated)
    .bind(&partition.file_path)
    .bind(partition.file_size)
    .bind(&partition.view_metadata.file_schema_hash)
	.bind(&partition.source_data_hash)
	.bind(file_metadata_buffer)
	.bind(partition.num_rows)
    .execute(&mut *transaction)
    .await;

    match insert_result {
        Ok(_) => {
            debug!(
                "[PARTITION_INSERT_SUCCESS] view={}/{} time_range=[{}, {}] source_hash={:?}",
                &partition.view_metadata.view_set_name,
                &partition.view_metadata.view_instance_id,
                partition.begin_insert_time,
                partition.end_insert_time,
                partition.source_data_hash
            );
        }
        Err(ref e) => {
            logger
                .write_log_entry(format!(
                    "[PARTITION_INSERT_ERROR] view={}/{} time_range=[{}, {}] source_hash={:?} error={}",
                    &partition.view_metadata.view_set_name,
                    &partition.view_metadata.view_instance_id,
                    partition.begin_insert_time,
                    partition.end_insert_time,
                    partition.source_data_hash,
                    e
                ))
                .await?;
            return Err(insert_result.unwrap_err().into());
        }
    };

    // Commit the transaction (this also releases the advisory lock)
    transaction.commit().await.with_context(|| "commit")?;

    info!(
        "[PARTITION_WRITE_COMMIT] view={}/{} time_range=[{}, {}] file_path={} - lock released",
        &partition.view_metadata.view_set_name,
        &partition.view_metadata.view_instance_id,
        partition.begin_insert_time,
        partition.end_insert_time,
        partition.file_path
    );
    Ok(())
}

// Keep old function name for compatibility
async fn write_partition_metadata(
    lake: &DataLakeConnection,
    partition: &Partition,
    file_metadata: Arc<ParquetMetaData>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    write_partition_metadata_attempt(lake, partition, &file_metadata, logger).await
}

/// Writes a partition to a Parquet file from a stream of `PartitionRowSet`s.
pub async fn write_partition_from_rows(
    lake: Arc<DataLakeConnection>,
    view_metadata: ViewMetadata,
    file_schema: Arc<Schema>,
    insert_range: TimeRange,
    source_data_hash: Vec<u8>,
    mut rb_stream: Receiver<PartitionRowSet>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let file_id = uuid::Uuid::new_v4();
    let file_path = format!(
        "views/{}/{}/{}/{}_{file_id}.parquet",
        &view_metadata.view_set_name,
        &view_metadata.view_instance_id,
        insert_range.begin.format("%Y-%m-%d"),
        insert_range.begin.format("%H-%M-%S")
    );
    let byte_counter = Arc::new(AtomicI64::new(0));
    let object_store_writer = AsyncParquetWriter::new(
        BufWriter::new(
            lake.blob_storage.inner(),
            object_store::path::Path::parse(&file_path).with_context(|| "parsing path")?,
        )
        .with_max_concurrency(2),
        byte_counter.clone(),
    );

    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::LZ4_RAW)
        .build();
    let mut arrow_writer =
        AsyncArrowWriter::try_new(object_store_writer, file_schema.clone(), Some(props))
            .with_context(|| "allocating async arrow writer")?;

    let desc = format!(
        "[{}, {}] {} {}",
        view_metadata.view_set_name,
        view_metadata.view_instance_id,
        insert_range.begin.to_rfc3339(),
        insert_range.end.to_rfc3339()
    );

    let mut min_event_time: Option<DateTime<Utc>> = None;
    let mut max_event_time: Option<DateTime<Utc>> = None;
    let mut write_progression = 0;
    while let Some(row_set) = rb_stream.recv().await {
        min_event_time = Some(
            min_event_time
                .unwrap_or(row_set.rows_time_range.begin)
                .min(row_set.rows_time_range.begin),
        );
        max_event_time = Some(
            max_event_time
                .unwrap_or(row_set.rows_time_range.end)
                .max(row_set.rows_time_range.end),
        );
        arrow_writer
            .write(&row_set.rows)
            .await
            .with_context(|| "arrow_writer.write")?;
        if arrow_writer.in_progress_size() > 100 * 1024 * 1024 {
            arrow_writer
                .flush()
                .await
                .with_context(|| "arrow_writer.flush")?;
        }

        // we don't want to spam the connection with progress reports
        // but we also don't want to trigger the idle timeout
        let progression = arrow_writer.bytes_written() / (10 * 1024 * 1024);
        if progression != write_progression {
            write_progression = progression;
            let written = arrow_writer.bytes_written();
            logger
                .write_log_entry(format!("{desc}: written {written} bytes"))
                .await
                .with_context(|| "writing log entry")?;
        }
    }

    if min_event_time.is_none() || max_event_time.is_none() {
        logger
            .write_log_entry(format!(
                "no data for {desc} partition, not writing the object"
            ))
            .await
            .with_context(|| "writing log entry")?;
        // should we check that there is no stale partition left behind?
        return Ok(());
    }
    let thrift_file_meta = arrow_writer
        .close()
        .await
        .with_context(|| "arrow_writer.close")?;
    debug!(
        "wrote nb_rows={} size={} path={file_path}",
        thrift_file_meta.num_rows,
        byte_counter.load(std::sync::atomic::Ordering::Relaxed)
    );
    let num_rows = thrift_file_meta.num_rows;
    let file_metadata = Arc::new(
        to_parquet_meta_data(&file_schema, thrift_file_meta)
            .with_context(|| "to_parquet_meta_data")?,
    );

    write_partition_metadata(
        &lake,
        &Partition {
            view_metadata,
            begin_insert_time: insert_range.begin,
            end_insert_time: insert_range.end,
            min_event_time: min_event_time.unwrap(),
            max_event_time: max_event_time.unwrap(),
            updated: sqlx::types::chrono::Utc::now(),
            file_path,
            file_size: byte_counter.load(std::sync::atomic::Ordering::Relaxed),
            source_data_hash,
            num_rows,
        },
        file_metadata,
        logger,
    )
    .await
    .with_context(|| "write_partition_metadata")?;
    Ok(())
}
// from parquet/src/file/footer.rs
fn parse_column_orders(
    t_column_orders: Option<Vec<datafusion::parquet::format::ColumnOrder>>,
    schema_descr: &SchemaDescriptor,
) -> Option<Vec<datafusion::parquet::basic::ColumnOrder>> {
    match t_column_orders {
        Some(orders) => {
            // Should always be the case
            assert_eq!(
                orders.len(),
                schema_descr.num_columns(),
                "Column order length mismatch"
            );
            let mut res = Vec::new();
            for (i, column) in schema_descr.columns().iter().enumerate() {
                match orders[i] {
                    datafusion::parquet::format::ColumnOrder::TYPEORDER(_) => {
                        let sort_order = datafusion::parquet::basic::ColumnOrder::get_sort_order(
                            column.logical_type(),
                            column.converted_type(),
                            column.physical_type(),
                        );
                        res.push(datafusion::parquet::basic::ColumnOrder::TYPE_DEFINED_ORDER(
                            sort_order,
                        ));
                    }
                }
            }
            Some(res)
        }
        None => None,
    }
}

fn to_parquet_meta_data(
    schema: &Schema,
    thrift_file_meta: datafusion::parquet::format::FileMetaData,
) -> Result<ParquetMetaData> {
    let schema_descr = Arc::new(ArrowSchemaConverter::new().convert(schema)?);
    let mut groups = vec![];
    for rg in thrift_file_meta.row_groups {
        groups.push(
            RowGroupMetaData::from_thrift(schema_descr.clone(), rg)
                .with_context(|| "RowGroupMetaData::from_thrift")?,
        );
    }
    Ok(ParquetMetaData::new(
        datafusion::parquet::file::metadata::FileMetaData::new(
            thrift_file_meta.version,
            thrift_file_meta.num_rows,
            thrift_file_meta.created_by,
            thrift_file_meta.key_value_metadata,
            schema_descr.clone(),
            parse_column_orders(thrift_file_meta.column_orders, &schema_descr),
        ),
        groups,
    ))
}
