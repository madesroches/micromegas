use crate::{
    arrow_utils::serialize_parquet_metadata, lakehouse::async_parquet_writer::AsyncParquetWriter,
    response_writer::Logger,
};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::{array::RecordBatch, datatypes::Schema},
    parquet::{
        arrow::{arrow_to_parquet_schema, AsyncArrowWriter},
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
use std::sync::{atomic::AtomicI64, Arc};
use tokio::sync::mpsc::Receiver;

use super::{partition::Partition, view::ViewMetadata};

/// RecordBatch + time range associated with the events
pub struct PartitionRowSet {
    pub min_time_row: DateTime<Utc>,
    pub max_time_row: DateTime<Utc>,
    pub rows: RecordBatch,
}

/// retire_expired_partitions moves old partitions to the temporary_files table
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

/// retire_partitions removes out of date partitions from the active set.
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
    let old_partitions = sqlx::query(
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
    .with_context(|| "listing old partitions")?;
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
    .with_context(|| "deleting out of date partitions")?;
    Ok(())
}

async fn write_partition_metadata(
    lake: &DataLakeConnection,
    partition: &Partition,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let file_metadata_buffer: Vec<u8> = serialize_parquet_metadata(&partition.file_metadata)
        .with_context(|| "serialize_parquet_metadata")?
        .into();

    let mut tr = lake.db_pool.begin().await?;

    // for jit partitions, we assume that the blocks were registered in order
    // since they are built based on begin_ticks, not insert_time
    retire_partitions(
        &mut tr,
        &partition.view_metadata.view_set_name,
        &partition.view_metadata.view_instance_id,
        partition.begin_insert_time,
        partition.end_insert_time,
        logger,
    )
    .await
    .with_context(|| "retire_partitions")?;

    sqlx::query(
        "INSERT INTO lakehouse_partitions VALUES($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12);",
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
    .execute(&mut *tr)
    .await
    .with_context(|| "inserting into lakehouse_partitions")?;

    tr.commit().await.with_context(|| "commit")?;
    Ok(())
}

/// write_partition_from_rows creates parquet file from a stream of [PartitionRowSet]
#[allow(clippy::too_many_arguments)]
pub async fn write_partition_from_rows(
    lake: Arc<DataLakeConnection>,
    view_metadata: ViewMetadata,
    file_schema: Arc<Schema>,
    begin_insert_time: DateTime<Utc>, //todo:change to min_insert_time & max_insert_time
    end_insert_time: DateTime<Utc>,
    source_data_hash: Vec<u8>,
    mut rb_stream: Receiver<PartitionRowSet>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let file_id = uuid::Uuid::new_v4();
    let file_path = format!(
        "views/{}/{}/{}/{}_{file_id}.parquet",
        &view_metadata.view_set_name,
        &view_metadata.view_instance_id,
        begin_insert_time.format("%Y-%m-%d"),
        begin_insert_time.format("%H-%M-%S")
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
        AsyncArrowWriter::try_new(object_store_writer, file_schema.clone(), Some(props))?;

    let desc = format!(
        "[{}, {}] {} {}",
        view_metadata.view_set_name,
        view_metadata.view_instance_id,
        begin_insert_time.to_rfc3339(),
        end_insert_time.to_rfc3339()
    );

    let mut min_event_time: Option<DateTime<Utc>> = None;
    let mut max_event_time: Option<DateTime<Utc>> = None;
    let mut write_progression = 0;
    while let Some(row_set) = rb_stream.recv().await {
        min_event_time = Some(
            min_event_time
                .unwrap_or(row_set.min_time_row)
                .min(row_set.min_time_row),
        );
        max_event_time = Some(
            max_event_time
                .unwrap_or(row_set.max_time_row)
                .max(row_set.max_time_row),
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
                .await?;
        }
    }

    if min_event_time.is_none() || max_event_time.is_none() {
        logger
            .write_log_entry(format!(
                "no data for {desc} partition, not writing the object"
            ))
            .await?;
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
    write_partition_metadata(
        &lake,
        &Partition {
            view_metadata,
            begin_insert_time,
            end_insert_time,
            min_event_time: min_event_time.unwrap(),
            max_event_time: max_event_time.unwrap(),
            updated: sqlx::types::chrono::Utc::now(),
            file_path,
            file_size: byte_counter.load(std::sync::atomic::Ordering::Relaxed),
            source_data_hash,
            file_metadata: Arc::new(
                to_parquet_meta_data(&file_schema, thrift_file_meta)
                    .with_context(|| "to_parquet_meta_data")?,
            ),
        },
        logger,
    )
    .await?;

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
    let schema_descr = Arc::new(arrow_to_parquet_schema(schema)?);
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
