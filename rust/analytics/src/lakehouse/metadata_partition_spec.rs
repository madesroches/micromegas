use super::view::{PartitionSpec, ViewMetadata};
use crate::{
    lakehouse::write_partition::{write_partition_from_rows, PartitionRowSet},
    response_writer::ResponseWriter,
    sql_arrow_bridge::rows_to_record_batch,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::arrow::datatypes::Schema;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use sqlx::Row;
use std::sync::Arc;

pub struct MetadataPartitionSpec {
    pub view_metadata: ViewMetadata,
    pub schema: Arc<Schema>,
    pub begin_insert: DateTime<Utc>,
    pub end_insert: DateTime<Utc>,
    pub record_count: i64,
    pub data_sql: Arc<String>,
    pub event_time_column: Arc<String>,
}

#[allow(clippy::too_many_arguments)]
pub async fn fetch_metadata_partition_spec(
    pool: &sqlx::PgPool,
    source_table: &str,
    event_time_column: Arc<String>,
    data_sql: Arc<String>,
    view_metadata: ViewMetadata,
    schema: Arc<Schema>,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
) -> Result<MetadataPartitionSpec> {
    let row = sqlx::query(&format!(
        "SELECT COUNT(*) as count
         FROM {source_table}
         WHERE insert_time >= $1
         AND insert_time < $2
         ;"
    ))
    .bind(begin_insert)
    .bind(end_insert)
    .fetch_one(pool)
    .await
    .with_context(|| "select count source metadata")?;
    Ok(MetadataPartitionSpec {
        view_metadata,
        schema,
        begin_insert,
        end_insert,
        record_count: row.try_get("count").with_context(|| "reading count")?,
        data_sql,
        event_time_column,
    })
}

#[async_trait]
impl PartitionSpec for MetadataPartitionSpec {
    fn get_source_data_hash(&self) -> Vec<u8> {
        self.record_count.to_le_bytes().to_vec()
    }

    async fn write(
        &self,
        lake: Arc<DataLakeConnection>,
        response_writer: Arc<ResponseWriter>,
    ) -> Result<()> {
        if self.record_count == 0 {
            return Ok(());
        }
        let desc = format!(
            "[{}, {}] {} {}",
            self.view_metadata.view_set_name,
            self.view_metadata.view_instance_id,
            self.begin_insert.to_rfc3339(),
            self.end_insert.to_rfc3339()
        );
        response_writer
            .write_string(&format!("writing {desc}"))
            .await?;

        let rows = sqlx::query(&self.data_sql)
            .bind(self.begin_insert)
            .bind(self.end_insert)
            .fetch_all(&lake.db_pool)
            .await?;
        let row_count = rows.len() as i64;
        if row_count == 0 {
            return Ok(());
        }
        let min_event_time: DateTime<Utc> = rows[0].try_get(&**self.event_time_column)?;
        assert!(min_event_time >= self.begin_insert);
        assert!(min_event_time <= self.end_insert);
        let max_event_time: DateTime<Utc> =
            rows[rows.len() - 1].try_get(&**self.event_time_column)?;
        assert!(max_event_time >= self.begin_insert);
        assert!(max_event_time <= self.end_insert);
        let record_batch =
            rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?;
        drop(rows);

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let join_handle = tokio::spawn(write_partition_from_rows(
            lake.clone(),
            self.view_metadata.clone(),
            self.schema.clone(),
            self.begin_insert,
            self.end_insert,
            row_count.to_le_bytes().to_vec(),
            rx,
            1024 * 1024,
            response_writer.clone(),
        ));
        tx.send(PartitionRowSet {
            min_time_row: min_event_time,
            max_time_row: max_event_time,
            rows: record_batch,
        })
        .await?;
        drop(tx);
        join_handle.await??;
        Ok(())
    }
}
