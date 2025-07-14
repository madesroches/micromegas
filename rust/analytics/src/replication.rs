use anyhow::{Context, Result};
use arrow_flight::decode::FlightRecordBatchStream;
use chrono::DateTime;
use datafusion::arrow::array::{
    BinaryArray, GenericListArray, Int32Array, Int64Array, StringArray, TimestampNanosecondArray,
};
use futures::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::info;
use std::sync::Arc;
use uuid::Uuid;

use crate::{arrow_properties::read_property_list, dfext::typed_column::typed_column_by_name};

async fn ingest_streams(
    lake: Arc<DataLakeConnection>,
    mut rb_stream: FlightRecordBatchStream,
) -> Result<i64> {
    let mut tr = lake.db_pool.begin().await?;
    let mut nb_rows: i64 = 0;
    while let Some(res) = rb_stream.next().await {
        let b = res?;
        nb_rows += b.num_rows() as i64;
        let stream_id_column: &StringArray = typed_column_by_name(&b, "stream_id")?;
        let process_id_column: &StringArray = typed_column_by_name(&b, "process_id")?;
        let dependencies_metadata_column: &BinaryArray =
            typed_column_by_name(&b, "dependencies_metadata")?;
        let objects_metadata_column: &BinaryArray = typed_column_by_name(&b, "objects_metadata")?;
        let tags_column: &GenericListArray<i32> = typed_column_by_name(&b, "tags")?;
        let properties_column: &GenericListArray<i32> = typed_column_by_name(&b, "properties")?;
        let insert_time_column: &TimestampNanosecondArray =
            typed_column_by_name(&b, "insert_time")?;

        for row in 0..b.num_rows() {
            let stream_id = Uuid::parse_str(stream_id_column.value(row))?;
            let process_id = Uuid::parse_str(process_id_column.value(row))?;
            let tags: Vec<String> = tags_column
                .value(row)
                .as_any()
                .downcast_ref::<StringArray>()
                .with_context(|| "casting tags")?
                .iter()
                .map(|item| String::from(item.unwrap_or_default()))
                .collect();
            let properties = read_property_list(properties_column.value(row))?;

            sqlx::query("INSERT INTO streams VALUES($1,$2,$3,$4,$5,$6,$7);")
                .bind(stream_id)
                .bind(process_id)
                .bind(dependencies_metadata_column.value(row))
                .bind(objects_metadata_column.value(row))
                .bind(tags)
                .bind(properties)
                .bind(DateTime::from_timestamp_nanos(
                    insert_time_column.value(row),
                ))
                .execute(&mut *tr)
                .await
                .with_context(|| "inserting into streams")?;
        }
    }
    tr.commit().await?;
    info!("ingested {nb_rows} streams");
    Ok(nb_rows)
}

async fn ingest_processes(
    lake: Arc<DataLakeConnection>,
    mut rb_stream: FlightRecordBatchStream,
) -> Result<i64> {
    let mut tr = lake.db_pool.begin().await?;
    let mut nb_rows: i64 = 0;
    while let Some(res) = rb_stream.next().await {
        let b = res?;
        nb_rows += b.num_rows() as i64;
        let process_id_column: &StringArray = typed_column_by_name(&b, "process_id")?;
        let exe_column: &StringArray = typed_column_by_name(&b, "exe")?;
        let username_column: &StringArray = typed_column_by_name(&b, "username")?;
        let realname_column: &StringArray = typed_column_by_name(&b, "realname")?;
        let computer_column: &StringArray = typed_column_by_name(&b, "computer")?;
        let distro_column: &StringArray = typed_column_by_name(&b, "distro")?;
        let cpu_brand_column: &StringArray = typed_column_by_name(&b, "cpu_brand")?;
        let process_tsc_freq_column: &Int64Array = typed_column_by_name(&b, "tsc_frequency")?;
        let start_time_column: &TimestampNanosecondArray = typed_column_by_name(&b, "start_time")?;
        let start_ticks_column: &Int64Array = typed_column_by_name(&b, "start_ticks")?;
        let insert_time_column: &TimestampNanosecondArray =
            typed_column_by_name(&b, "insert_time")?;
        let parent_process_id_column: &StringArray = typed_column_by_name(&b, "parent_process_id")?;
        let properties_column: &GenericListArray<i32> = typed_column_by_name(&b, "properties")?;
        for row in 0..b.num_rows() {
            let process_id = Uuid::parse_str(process_id_column.value(row))?;
            let parent_process_str = parent_process_id_column.value(row);
            let parent_process_id = if parent_process_str.is_empty() {
                None
            } else {
                Some(Uuid::parse_str(parent_process_str)?)
            };
            let properties = read_property_list(properties_column.value(row))?;
            sqlx::query(
                "INSERT INTO processes VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13);",
            )
            .bind(process_id)
            .bind(exe_column.value(row))
            .bind(username_column.value(row))
            .bind(realname_column.value(row))
            .bind(computer_column.value(row))
            .bind(distro_column.value(row))
            .bind(cpu_brand_column.value(row))
            .bind(process_tsc_freq_column.value(row))
            .bind(DateTime::from_timestamp_nanos(start_time_column.value(row)))
            .bind(start_ticks_column.value(row))
            .bind(DateTime::from_timestamp_nanos(
                insert_time_column.value(row),
            ))
            .bind(parent_process_id)
            .bind(properties)
            .execute(&mut *tr)
            .await
            .with_context(|| "executing sql insert into processes")?;
        }
    }
    tr.commit().await?;
    info!("ingested {nb_rows} processes");
    Ok(nb_rows)
}

async fn ingest_payloads(
    lake: Arc<DataLakeConnection>,
    mut rb_stream: FlightRecordBatchStream,
) -> Result<i64> {
    let mut nb_rows: i64 = 0;
    while let Some(res) = rb_stream.next().await {
        let b = res?;
        nb_rows += b.num_rows() as i64;
        let process_id_column: &StringArray = typed_column_by_name(&b, "process_id")?;
        let stream_id_column: &StringArray = typed_column_by_name(&b, "stream_id")?;
        let block_id_column: &StringArray = typed_column_by_name(&b, "block_id")?;
        let payload_column: &BinaryArray = typed_column_by_name(&b, "payload")?;
        for row in 0..b.num_rows() {
            let process_id = process_id_column.value(row);
            let stream_id = stream_id_column.value(row);
            let block_id = block_id_column.value(row);
            let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");
            let payload = bytes::Bytes::copy_from_slice(payload_column.value(row));
            lake.blob_storage
                .put(&obj_path, payload)
                .await
                .with_context(|| "Error writing block to blob storage")?;
        }
    }
    info!("ingested {nb_rows} payloads");
    Ok(nb_rows)
}

async fn ingest_blocks(
    lake: Arc<DataLakeConnection>,
    mut rb_stream: FlightRecordBatchStream,
) -> Result<i64> {
    let mut tr = lake.db_pool.begin().await?;
    let mut nb_rows: i64 = 0;
    while let Some(res) = rb_stream.next().await {
        let b = res?;
        nb_rows += b.num_rows() as i64;
        let block_id_column: &StringArray = typed_column_by_name(&b, "block_id")?;
        let stream_id_column: &StringArray = typed_column_by_name(&b, "stream_id")?;
        let process_id_column: &StringArray = typed_column_by_name(&b, "process_id")?;
        let begin_time_column: &TimestampNanosecondArray = typed_column_by_name(&b, "begin_time")?;
        let begin_ticks_column: &Int64Array = typed_column_by_name(&b, "begin_ticks")?;
        let end_time_column: &TimestampNanosecondArray = typed_column_by_name(&b, "end_time")?;
        let end_ticks_column: &Int64Array = typed_column_by_name(&b, "end_ticks")?;
        let nb_objects_column: &Int32Array = typed_column_by_name(&b, "nb_objects")?;
        let object_offset_column: &Int64Array = typed_column_by_name(&b, "object_offset")?;
        let payload_size_column: &Int64Array = typed_column_by_name(&b, "payload_size")?;
        let insert_time_column: &TimestampNanosecondArray =
            typed_column_by_name(&b, "insert_time")?;
        for row in 0..b.num_rows() {
            let block_id = Uuid::parse_str(block_id_column.value(row))?;
            let stream_id = Uuid::parse_str(stream_id_column.value(row))?;
            let process_id = Uuid::parse_str(process_id_column.value(row))?;
            sqlx::query("INSERT INTO blocks VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11);")
                .bind(block_id)
                .bind(stream_id)
                .bind(process_id)
                .bind(DateTime::from_timestamp_nanos(begin_time_column.value(row)))
                .bind(begin_ticks_column.value(row))
                .bind(DateTime::from_timestamp_nanos(end_time_column.value(row)))
                .bind(end_ticks_column.value(row))
                .bind(nb_objects_column.value(row))
                .bind(object_offset_column.value(row))
                .bind(payload_size_column.value(row))
                .bind(DateTime::from_timestamp_nanos(
                    insert_time_column.value(row),
                ))
                .execute(&mut *tr)
                .await
                .with_context(|| "executing sql insert into blocks")?;
        }
    }
    tr.commit().await?;
    info!("ingested {nb_rows} blocks");
    Ok(nb_rows)
}

/// Ingests data from a FlightRecordBatchStream into the specified table.
pub async fn bulk_ingest(
    lake: Arc<DataLakeConnection>,
    table_name: &str,
    rb_stream: FlightRecordBatchStream,
) -> Result<i64> {
    match table_name {
        "processes" => ingest_processes(lake, rb_stream).await,
        "streams" => ingest_streams(lake, rb_stream).await,
        "blocks" => ingest_blocks(lake, rb_stream).await,
        "payloads" => ingest_payloads(lake, rb_stream).await,
        other => anyhow::bail!("bulk ingest for table {other} not supported"),
    }
}
