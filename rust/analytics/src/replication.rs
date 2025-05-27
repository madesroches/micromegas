use anyhow::{Context, Result};
use arrow_flight::decode::FlightRecordBatchStream;
use chrono::DateTime;
use datafusion::arrow::array::{
    GenericListArray, Int64Array, StringArray, TimestampNanosecondArray,
};
use futures::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::info;
use std::sync::Arc;
use uuid::Uuid;

use crate::{arrow_properties::read_property_list, dfext::typed_column::typed_column_by_name};

pub async fn ingest_processes(
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

pub async fn bulk_ingest(
    lake: Arc<DataLakeConnection>,
    table_name: &str,
    rb_stream: FlightRecordBatchStream,
) -> Result<i64> {
    match table_name {
        "processes" => ingest_processes(lake, rb_stream).await,
        other => anyhow::bail!("bulk ingest for table {other} not supported"),
    }
}
