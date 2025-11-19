use crate::{
    arrow_utils::serialize_parquet_metadata, lakehouse::async_parquet_writer::AsyncParquetWriter,
    response_writer::Logger, time::TimeRange,
};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::{array::RecordBatch, datatypes::Schema},
    parquet::{
        arrow::AsyncArrowWriter,
        basic::Compression,
        file::{
            metadata::ParquetMetaData,
            properties::{WriterProperties, WriterVersion},
        },
    },
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use object_store::buffered::BufWriter;
use sqlx::Row;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, atomic::AtomicI64};
use tokio::sync::mpsc::Receiver;

use super::{partition::Partition, partition_source_data, view::ViewMetadata};

/// Adds a file to the temporary_files table for cleanup.
///
/// Files added to temporary_files will be automatically deleted by the cleanup process
/// after the expiration time. The default expiration is 1 hour from now.
pub async fn add_file_for_cleanup(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    file_path: &str,
    file_size: i64,
) -> Result<()> {
    let expiration = Utc::now()
        + TimeDelta::try_hours(1)
            .with_context(|| "calculating expiration time for temporary file")?;

    sqlx::query("INSERT INTO temporary_files VALUES ($1, $2, $3)")
        .bind(file_path)
        .bind(file_size)
        .bind(expiration)
        .execute(&mut **transaction)
        .await
        .with_context(|| format!("adding file {file_path} to temporary files for cleanup"))?;

    Ok(())
}

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

    let mut file_paths = Vec::new();
    for old_part in &old_partitions {
        let file_path: Option<String> = old_part.try_get("file_path")?;
        let file_size: i64 = old_part.try_get("file_size")?;
        if let Some(path) = file_path {
            info!("adding out of date partition {path} to temporary files to be deleted");
            add_file_for_cleanup(&mut transaction, &path, file_size).await?;
            file_paths.push(path);
        }
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

    // LOG: Found partitions for retirement (only if any found)
    if !old_partitions.is_empty() {
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
    }

    let mut file_paths = Vec::new();
    for old_part in &old_partitions {
        let file_path: Option<String> = old_part.try_get("file_path")?;
        let file_size: i64 = old_part.try_get("file_size")?;
        if let Some(path) = file_path {
            logger
                .write_log_entry(format!(
                    "adding out of date partition {path} to temporary files to be deleted"
                ))
                .await?;
            add_file_for_cleanup(transaction, &path, file_size).await?;
            file_paths.push(path);
        }
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

async fn insert_partition(
    lake: &DataLakeConnection,
    partition: &Partition,
    file_metadata: Option<&Arc<ParquetMetaData>>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    // Generate deterministic lock key for this partition
    let lock_key = generate_partition_lock_key(
        &partition.view_metadata.view_set_name,
        &partition.view_metadata.view_instance_id,
        partition.begin_insert_time(),
        partition.end_insert_time(),
    );

    let mut transaction = lake.db_pool.begin().await?;

    debug!(
        "[PARTITION_LOCK] view={}/{} time_range=[{}, {}] lock_key={} - acquiring advisory lock",
        &partition.view_metadata.view_set_name,
        &partition.view_metadata.view_instance_id,
        partition.begin_insert_time(),
        partition.end_insert_time(),
        lock_key
    );

    // Acquire advisory lock - this will block until we can proceed
    // pg_advisory_xact_lock automatically releases when transaction ends
    sqlx::query("SELECT pg_advisory_xact_lock($1);")
        .bind(lock_key)
        .execute(&mut *transaction)
        .await
        .with_context(|| "acquiring advisory lock")?;

    // Decode source_data_hash back to the row count (it's stored as i64 little-endian bytes)
    let source_row_count = partition_source_data::hash_to_object_count(&partition.source_data_hash)
        .with_context(|| "decoding source_data_hash to row count")?;

    // LOG: Lock acquired, starting partition write
    logger
        .write_log_entry(format!(
            "[PARTITION_WRITE_START] view={}/{} time_range=[{}, {}] source_rows={} - lock acquired",
            &partition.view_metadata.view_set_name,
            &partition.view_metadata.view_instance_id,
            partition.begin_insert_time(),
            partition.end_insert_time(),
            source_row_count
        ))
        .await?;

    // for jit partitions, we assume that the blocks were registered in order
    // since they are built based on begin_ticks, not insert_time
    retire_partitions(
        &mut transaction,
        &partition.view_metadata.view_set_name,
        &partition.view_metadata.view_instance_id,
        partition.begin_insert_time(),
        partition.end_insert_time(),
        logger.clone(),
    )
    .await
    .with_context(|| "retire_partitions")?;

    debug!(
        "[PARTITION_INSERT_ATTEMPT] view={}/{} time_range=[{}, {}] source_rows={} file_path={:?}",
        &partition.view_metadata.view_set_name,
        &partition.view_metadata.view_instance_id,
        partition.begin_insert_time(),
        partition.end_insert_time(),
        source_row_count,
        partition.file_path
    );

    // Insert the parquet metadata into the dedicated metadata table within the same transaction
    // Only insert metadata if partition has a file (not empty)
    if let (Some(file_path), Some(metadata)) = (&partition.file_path, file_metadata) {
        let metadata_bytes = serialize_parquet_metadata(metadata)
            .with_context(|| "serializing parquet metadata for dedicated table")?;
        let insert_time = sqlx::types::chrono::Utc::now();

        sqlx::query(
            "INSERT INTO partition_metadata (file_path, metadata, insert_time)
             VALUES ($1, $2, $3)",
        )
        .bind(file_path)
        .bind(metadata_bytes.as_ref())
        .bind(insert_time)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("inserting metadata for file: {}", file_path))?;
    }

    // Insert the new partition (without metadata column)
    let insert_result = sqlx::query(
        "INSERT INTO lakehouse_partitions VALUES($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12);",
    )
    .bind(&*partition.view_metadata.view_set_name)
    .bind(&*partition.view_metadata.view_instance_id)
    .bind(partition.begin_insert_time())
    .bind(partition.end_insert_time())
    .bind(partition.min_event_time())
    .bind(partition.max_event_time())
    .bind(partition.updated)
    .bind(&partition.file_path)
    .bind(partition.file_size)
    .bind(&partition.view_metadata.file_schema_hash)
    .bind(&partition.source_data_hash)
    .bind(partition.num_rows)
    .execute(&mut *transaction)
    .await;

    match insert_result {
        Ok(_) => {
            debug!(
                "[PARTITION_INSERT_SUCCESS] view={}/{} time_range=[{}, {}] source_rows={}",
                &partition.view_metadata.view_set_name,
                &partition.view_metadata.view_instance_id,
                partition.begin_insert_time(),
                partition.end_insert_time(),
                source_row_count
            );
        }
        Err(ref e) => {
            logger
                .write_log_entry(format!(
                    "[PARTITION_INSERT_ERROR] view={}/{} time_range=[{}, {}] source_rows={} error={}",
                    &partition.view_metadata.view_set_name,
                    &partition.view_metadata.view_instance_id,
                    partition.begin_insert_time(),
                    partition.end_insert_time(),
                    source_row_count,
                    e
                ))
                .await?;
            return Err(insert_result.unwrap_err().into());
        }
    };

    // Commit the transaction (this also releases the advisory lock)
    transaction.commit().await.with_context(|| "commit")?;

    info!(
        "[PARTITION_WRITE_COMMIT] view={}/{} time_range=[{}, {}] file_path={:?} - lock released",
        &partition.view_metadata.view_set_name,
        &partition.view_metadata.view_instance_id,
        partition.begin_insert_time(),
        partition.end_insert_time(),
        partition.file_path
    );
    Ok(())
}

/// Result of writing rows to a partition file.
struct PartitionWriteResult {
    num_rows: i64,
    file_metadata: Option<Arc<ParquetMetaData>>,
    file_path: Option<String>,
    file_size: i64,
    event_time_range: Option<TimeRange>,
}

/// Writes rows from the stream and tracks event time ranges.
async fn write_rows_and_track_times(
    rb_stream: &mut Receiver<PartitionRowSet>,
    arrow_writer: &mut AsyncArrowWriter<AsyncParquetWriter>,
    logger: &Arc<dyn Logger>,
    desc: &str,
) -> Result<Option<TimeRange>> {
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

        // Log progress every 10MB to avoid spamming and prevent idle timeout
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

    Ok(match (min_event_time, max_event_time) {
        (Some(begin), Some(end)) => Some(TimeRange { begin, end }),
        _ => None,
    })
}

/// Finalizes the partition write, closing the file and creating metadata.
async fn finalize_partition_write(
    event_time_range: Option<TimeRange>,
    arrow_writer: AsyncArrowWriter<AsyncParquetWriter>,
    file_path: String,
    byte_counter: &Arc<AtomicI64>,
    logger: &Arc<dyn Logger>,
    desc: &str,
    object_store: Arc<dyn object_store::ObjectStore>,
) -> Result<PartitionWriteResult> {
    if let Some(event_time_range) = event_time_range {
        // Potentially non-empty partition: close the file and get metadata
        let close_result = arrow_writer.close().await;

        match close_result {
            Ok(parquet_metadata) => {
                let num_rows = parquet_metadata.file_metadata().num_rows();

                // Check if the file actually contains rows
                // Even if we tracked event times, the file might be empty
                if num_rows == 0 {
                    // File contains no rows - treat as empty partition
                    logger
                        .write_log_entry(format!(
                            "created 0-row file, treating as empty partition for {desc}"
                        ))
                        .await
                        .with_context(|| "writing log entry")?;

                    // Delete the empty file
                    let path = object_store::path::Path::from(file_path.as_str());
                    if let Err(delete_err) = object_store.delete(&path).await {
                        warn!("failed to delete empty file {}: {}", file_path, delete_err);
                    }

                    return Ok(PartitionWriteResult {
                        num_rows: 0,
                        file_metadata: None,
                        file_path: None,
                        file_size: 0,
                        event_time_range: None,
                    });
                }

                // Non-empty file: keep it and return full metadata
                debug!(
                    "wrote nb_rows={} size={} path={file_path}",
                    num_rows,
                    byte_counter.load(std::sync::atomic::Ordering::Relaxed)
                );
                let file_metadata = Arc::new(parquet_metadata);
                let file_size = byte_counter.load(std::sync::atomic::Ordering::Relaxed);
                Ok(PartitionWriteResult {
                    num_rows,
                    file_metadata: Some(file_metadata),
                    file_path: Some(file_path),
                    file_size,
                    event_time_range: Some(event_time_range),
                })
            }
            Err(e) => {
                // Close failed - try to delete any partial file that may have been written
                warn!(
                    "arrow_writer.close failed, attempting to delete partial file: {}",
                    file_path
                );
                let path = object_store::path::Path::from(file_path.as_str());
                if let Err(delete_err) = object_store.delete(&path).await {
                    warn!(
                        "failed to delete partial file {}: {}",
                        file_path, delete_err
                    );
                }
                Err(e).with_context(|| "arrow_writer.close")
            }
        }
    } else {
        // Empty partition: no data was written, but the arrow writer may have written
        // a partial file header. Drop the writer and delete any partial file.
        drop(arrow_writer);

        logger
            .write_log_entry(format!("creating empty partition record for {desc}"))
            .await
            .with_context(|| "writing log entry")?;

        // Try to delete any partial file that may have been created
        // (ignore errors - file may not exist if no header was written)
        let path = object_store::path::Path::from(file_path.as_str());
        let _ = object_store.delete(&path).await;

        Ok(PartitionWriteResult {
            num_rows: 0,
            file_metadata: None,
            file_path: None,
            file_size: 0,
            event_time_range: None,
        })
    }
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

    // Write rows and track event time ranges
    let event_time_range =
        write_rows_and_track_times(&mut rb_stream, &mut arrow_writer, &logger, &desc).await?;

    // Finalize the write (close file or create empty metadata)
    let result = finalize_partition_write(
        event_time_range,
        arrow_writer,
        file_path,
        &byte_counter,
        &logger,
        &desc,
        lake.blob_storage.inner(),
    )
    .await?;

    insert_partition(
        &lake,
        &Partition {
            view_metadata,
            insert_time_range: insert_range,
            event_time_range: result.event_time_range,
            updated: sqlx::types::chrono::Utc::now(),
            file_path: result.file_path,
            file_size: result.file_size,
            source_data_hash,
            num_rows: result.num_rows,
        },
        result.file_metadata.as_ref(),
        logger,
    )
    .await
    .with_context(|| "insert_partition")?;
    Ok(())
}
