use super::{
    dataframe_time_bounds::DataFrameTimeBounds,
    view::{PartitionSpec, ViewMetadata},
};
use crate::{
    lakehouse::write_partition::{PartitionRowSet, write_partition_from_rows},
    response_writer::Logger,
    sql_arrow_bridge::rows_to_record_batch,
    time::TimeRange,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use datafusion::{arrow::datatypes::Schema, prelude::*};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use sqlx::Row;
use std::sync::Arc;

#[derive(Debug)]
pub struct MetadataPartitionSpec {
    pub view_metadata: ViewMetadata,
    pub schema: Arc<Schema>,
    pub insert_range: TimeRange,
    pub record_count: i64,
    pub data_sql: Arc<String>,
    pub compute_time_bounds: Arc<dyn DataFrameTimeBounds>,
}

pub async fn fetch_metadata_partition_spec(
    pool: &sqlx::PgPool,
    source_count_query: &str,
    data_sql: Arc<String>,
    view_metadata: ViewMetadata,
    schema: Arc<Schema>,
    insert_range: TimeRange,
    compute_time_bounds: Arc<dyn DataFrameTimeBounds>,
) -> Result<MetadataPartitionSpec> {
    //todo: extract this query to allow join (instead of source_table)
    let row = sqlx::query(source_count_query)
        .bind(insert_range.begin)
        .bind(insert_range.end)
        .fetch_one(pool)
        .await
        .with_context(|| "select count source metadata")?;
    Ok(MetadataPartitionSpec {
        view_metadata,
        schema,
        insert_range,
        record_count: row.try_get("count").with_context(|| "reading count")?,
        data_sql,
        compute_time_bounds,
    })
}

#[async_trait]
impl PartitionSpec for MetadataPartitionSpec {
    fn is_empty(&self) -> bool {
        self.record_count < 1
    }

    fn get_source_data_hash(&self) -> Vec<u8> {
        self.record_count.to_le_bytes().to_vec()
    }

    async fn write(&self, lake: Arc<DataLakeConnection>, logger: Arc<dyn Logger>) -> Result<()> {
        // Allow empty record_count - write_partition_from_rows will create
        // an empty partition record if no data is sent through the channel
        let desc = format!(
            "[{}, {}] {} {}",
            self.view_metadata.view_set_name,
            self.view_metadata.view_instance_id,
            self.insert_range.begin.to_rfc3339(),
            self.insert_range.end.to_rfc3339()
        );
        logger.write_log_entry(format!("writing {desc}")).await?;

        let rows = sqlx::query(&self.data_sql)
            .bind(self.insert_range.begin)
            .bind(self.insert_range.end)
            .fetch_all(&lake.db_pool)
            .await?;
        let row_count = rows.len() as i64;

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let join_handle = tokio::spawn(write_partition_from_rows(
            lake.clone(),
            self.view_metadata.clone(),
            self.schema.clone(),
            self.insert_range,
            row_count.to_le_bytes().to_vec(),
            rx,
            logger.clone(),
        ));

        // Only send data if we have rows
        if row_count > 0 {
            let record_batch =
                rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?;
            drop(rows);
            let ctx = SessionContext::new();
            let event_time_range = self
                .compute_time_bounds
                .get_time_bounds(ctx.read_batch(record_batch.clone())?)
                .await?;
            tx.send(PartitionRowSet::new(event_time_range, record_batch))
                .await?;
        }

        drop(tx);
        join_handle.await??;
        Ok(())
    }
}
