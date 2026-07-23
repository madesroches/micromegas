use crate::{
    lakehouse::async_parquet_writer::AsyncParquetWriter, response_writer::Logger, time::TimeRange,
};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::{array::RecordBatch, datatypes::Schema},
    parquet::{
        arrow::AsyncArrowWriter,
        basic::Compression,
        file::properties::{WriterProperties, WriterVersion},
    },
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use object_store::ObjectStoreExt;
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

    instrument_named!(
        sqlx::query("INSERT INTO temporary_files VALUES ($1, $2, $3)")
            .bind(file_path)
            .bind(file_size)
            .bind(expiration)
            .execute(&mut **transaction),
        "sql_insert_temporary_file"
    )
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

#[span_fn]
async fn retire_expired_partitions_batch(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<bool> {
    let batch_size: i32 = 1000;
    let mut transaction = lake.db_pool.begin().await?;
    let rows = instrument_named!(
        sqlx::query(
            "DELETE FROM lakehouse_partitions
         WHERE (view_set_name, view_instance_id, begin_insert_time, end_insert_time) IN (
             SELECT view_set_name, view_instance_id, begin_insert_time, end_insert_time
             FROM lakehouse_partitions
             WHERE end_insert_time < $1
             LIMIT $2
         )
         RETURNING file_path, file_size;",
        )
        .bind(expiration)
        .bind(batch_size)
        .fetch_all(&mut *transaction),
        "sql_delete_expired_partitions_batch"
    )
    .await
    .with_context(|| "deleting expired partitions batch")?;

    if rows.is_empty() {
        return Ok(false);
    }
    let count = rows.len();
    for row in &rows {
        let file_path: Option<String> = row.try_get("file_path")?;
        let file_size: i64 = row.try_get("file_size")?;
        if let Some(path) = file_path {
            debug!("retiring expired partition file {path} ({file_size} bytes)");
            add_file_for_cleanup(&mut transaction, &path, file_size).await?;
        }
    }
    transaction.commit().await.with_context(|| "commit")?;
    info!("retired {count} expired partitions");
    Ok(count == batch_size as usize)
}

#[span_fn]
pub async fn retire_expired_partitions(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<()> {
    while retire_expired_partitions_batch(lake, expiration).await? {}
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
        instrument_named!(
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
            .fetch_all(&mut **transaction),
            "sql_select_old_partitions"
        )
        .await
        .with_context(|| "listing old partitions (exact match)")?
    } else {
        // For time ranges, use inclusive inequalities
        instrument_named!(
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
            .fetch_all(&mut **transaction),
            "sql_select_old_partitions"
        )
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
        instrument_named!(
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
            .execute(&mut **transaction),
            "sql_delete_old_partitions"
        )
        .await
        .with_context(|| "deleting out of date partitions (exact match)")?
    } else {
        // For time ranges, use inclusive inequalities
        instrument_named!(
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
            .execute(&mut **transaction),
            "sql_delete_old_partitions"
        )
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

/// Deletes `file_path` from object storage unless a partition row references it.
///
/// A failed commit may still have been applied server-side, so the returned error alone can't
/// tell us whether the file is orphaned. Check the authoritative state instead: the path carries
/// a per-write UUID, so if no `lakehouse_partitions` row references it, nothing ever will and it
/// is safe to delete.
async fn delete_if_orphan(lake: &DataLakeConnection, file_path: &str) -> Result<()> {
    let referenced = instrument_named!(
        sqlx::query("SELECT 1 FROM lakehouse_partitions WHERE file_path = $1 LIMIT 1;")
            .bind(file_path)
            .fetch_optional(&lake.db_pool),
        "sql_select_partition_file_referenced"
    )
    .await
    .with_context(|| "checking whether partition file is referenced")?
    .is_some();
    if !referenced {
        let path = object_store::path::Path::from(file_path);
        lake.blob_storage
            .inner()
            .delete(&path)
            .await
            .with_context(|| format!("deleting orphaned partition file {file_path}"))?;
    }
    Ok(())
}

async fn insert_partition(
    lake: &DataLakeConnection,
    partition: &Partition,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let result = insert_partition_transaction(lake, partition, logger).await;
    if result.is_err()
        && let Some(file_path) = &partition.file_path
    {
        // The insert failed. A failed commit may still have been applied server-side, so we
        // can't assume the file is unreferenced -- delete_if_orphan checks and deletes only if
        // nothing references it. Best-effort: never mask the original error.
        if let Err(cleanup_err) = delete_if_orphan(lake, file_path).await {
            warn!("delete_if_orphan failed for {file_path}: {cleanup_err}");
        }
    }
    result
}

async fn insert_partition_transaction(
    lake: &DataLakeConnection,
    partition: &Partition,
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
    // pg_advisory_xact_lock automatically releases when transaction ends.
    // The lock only serializes writers of this exact (view, instance, range) key, avoiding
    // duplicate work; correctness against overlapping writers of *different* ranges is enforced
    // by the lakehouse_partitions_no_overlap exclusion constraint at insert time.
    instrument_named!(
        sqlx::query("SELECT pg_advisory_xact_lock($1);")
            .bind(lock_key)
            .execute(&mut *transaction),
        "sql_advisory_lock"
    )
    .await
    .with_context(|| "acquiring advisory lock")?;

    // Decode source_data_hash back to the row count (it's stored as i64 little-endian bytes)
    let source_row_count = partition_source_data::hash_to_object_count(&partition.source_data_hash)
        .with_context(|| "decoding source_data_hash to row count")?;

    // LOG: Lock acquired, starting partition write
    logger
        .write_log_entry(format!(
            "[PARTITION_WRITE_START] view={}/{} time_range=[{}, {}] source_rows={} - lock acquired",
            partition.view_metadata.view_set_name,
            partition.view_metadata.view_instance_id,
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

    // Insert the new partition with format version 2 (Arrow 57.0)
    let insert_result = instrument_named!(
        sqlx::query(
            "INSERT INTO lakehouse_partitions VALUES($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 2, $13);",
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
        .bind(&partition.sort_order)
        .execute(&mut *transaction),
        "sql_insert_partition"
    )
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
                    partition.view_metadata.view_set_name,
                    partition.view_metadata.view_instance_id,
                    partition.begin_insert_time(),
                    partition.end_insert_time(),
                    source_row_count,
                    e
                ))
                .await?;
            // Translate an exclusion-constraint violation (raw SQLSTATE 23P01) into a legible
            // domain error: it means an existing partition overlaps this one without being
            // contained by it, so the containment-based retire in this transaction did not
            // replace it -- e.g. a concurrent maintenance merge committed a wider partition.
            let overlap_detail = e.as_database_error().and_then(|db_err| {
                (db_err.constraint() == Some("lakehouse_partitions_no_overlap"))
                    .then(|| db_err.to_string())
            });
            if let Some(detail) = overlap_detail {
                anyhow::bail!(
                    "new partition {}/{} [{}, {}] overlaps an existing partition it does not \
                     fully contain, so this write cannot replace it (likely a concurrent \
                     materialization or merge). Retire the conflicting partition (e.g. \
                     retire_partition_by_metadata) or align the requested range/delta, then \
                     retry. Postgres detail: {detail}",
                    partition.view_metadata.view_set_name,
                    partition.view_metadata.view_instance_id,
                    partition.begin_insert_time().to_rfc3339(),
                    partition.end_insert_time().to_rfc3339(),
                );
            }
            return Err(insert_result.unwrap_err().into());
        }
    };

    // Commit the transaction (this also releases the advisory lock). On failure the transaction
    // rolls back and the caller's delete_if_orphan reclaims the now-unreferenced parquet file.
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
    file_path: Option<String>,
    file_size: i64,
    event_time_range: Option<TimeRange>,
}

/// Writes rows from the stream and tracks event time ranges.
pub async fn write_rows_and_track_times(
    rb_stream: &mut Receiver<Result<PartitionRowSet, anyhow::Error>>,
    arrow_writer: &mut AsyncArrowWriter<AsyncParquetWriter>,
    logger: &Arc<dyn Logger>,
    desc: &str,
) -> Result<Option<TimeRange>> {
    let mut min_event_time: Option<DateTime<Utc>> = None;
    let mut max_event_time: Option<DateTime<Utc>> = None;
    let mut write_progression = 0;

    while let Some(msg) = rb_stream.recv().await {
        let row_set = msg?;
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
                        file_path: None,
                        file_size: 0,
                        event_time_range: None,
                    });
                }

                // Non-empty file: keep it and return the result
                debug!(
                    "wrote nb_rows={} size={} path={file_path}",
                    num_rows,
                    byte_counter.load(std::sync::atomic::Ordering::Relaxed)
                );
                let file_size = byte_counter.load(std::sync::atomic::Ordering::Relaxed);
                Ok(PartitionWriteResult {
                    num_rows,
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
            file_path: None,
            file_size: 0,
            event_time_range: None,
        })
    }
}

/// Writes a partition to a Parquet file from a stream of `PartitionRowSet`s.
///
/// `sort_order` is recorded on the resulting `Partition` as-is (see
/// `View::get_merged_partition_sort_order` and `MetadataPartitionSpec::sort_order`).
#[expect(clippy::too_many_arguments)]
pub async fn write_partition_from_rows(
    lake: Arc<DataLakeConnection>,
    view_metadata: ViewMetadata,
    file_schema: Arc<Schema>,
    insert_range: TimeRange,
    source_data_hash: Vec<u8>,
    sort_order: Option<Vec<String>>,
    mut rb_stream: Receiver<Result<PartitionRowSet, anyhow::Error>>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let file_id = uuid::Uuid::new_v4();
    let file_path = format!(
        "views/{}/{}/{}/{}_{file_id}.parquet",
        view_metadata.view_set_name,
        view_metadata.view_instance_id,
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

    // Configure writer with page-level statistics enabled (default in Arrow 57.0+)
    // This ensures ColumnIndex with proper null_pages field is written for DataFusion 51+ compatibility
    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::LZ4_RAW)
        // Explicitly enable page-level statistics for clarity (this is the default in Arrow 57.0+)
        // This generates ColumnIndex structures with proper null_pages field
        .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Page)
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
        match write_rows_and_track_times(&mut rb_stream, &mut arrow_writer, &logger, &desc).await {
            Ok(range) => range,
            Err(e) => {
                // The writer is dropped without close/abort on this error path, which can
                // leave already-uploaded multipart data orphaned in object storage. Delete
                // any partial file before propagating the error (mirror finalize cleanup).
                drop(arrow_writer);
                warn!(
                    "write_rows_and_track_times failed, attempting to delete partial file: {}",
                    file_path
                );
                let path = object_store::path::Path::from(file_path.as_str());
                if let Err(delete_err) = lake.blob_storage.inner().delete(&path).await {
                    warn!(
                        "failed to delete partial file {}: {}",
                        file_path, delete_err
                    );
                }
                return Err(e).with_context(|| "write_rows_and_track_times");
            }
        };

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

    // On failure insert_partition reclaims the now-unreferenced parquet file via delete_if_orphan.
    let warm_file_path = result.file_path.clone();
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
            sort_order,
        },
        logger,
    )
    .await
    .with_context(|| "insert_partition")?;

    // The file is now durable in S3 and registered in PostgreSQL: warm the
    // object cache with its key so the follow-up query's first read is a
    // cache hit instead of a cold origin GET. Fire-and-forget: this must
    // never delay or fail the write/materialization path.
    if let Some(file_path) = &warm_file_path {
        lake.warm_object(file_path, result.file_size);
    }
    Ok(())
}
