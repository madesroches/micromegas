use super::{
    view::{PartitionSpec, ViewMetadata},
    write_partition::write_partition_from_rows,
};
use crate::{
    dfext::get_column::{typed_column, typed_column_by_name},
    lakehouse::write_partition::PartitionRowSet,
    response_writer::Logger,
};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::array::{Int64Array, RecordBatch, TimestampNanosecondArray},
    functions_aggregate::min_max::{max, min},
    prelude::*,
};
use futures::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

pub struct SqlPartitionSpec {
    ctx: SessionContext,
    transform_query: Arc<String>,
    event_time_column: Arc<String>,
    view_metadata: ViewMetadata,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    record_count: i64,
}

impl SqlPartitionSpec {
    pub fn new(
        ctx: SessionContext,
        transform_query: Arc<String>,
        event_time_column: Arc<String>,
        view_metadata: ViewMetadata,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
        record_count: i64,
    ) -> Self {
        Self {
            ctx,
            transform_query,
            event_time_column,
            view_metadata,
            begin_insert,
            end_insert,
            record_count,
        }
    }
}

#[async_trait]
impl PartitionSpec for SqlPartitionSpec {
    fn get_source_data_hash(&self) -> Vec<u8> {
        self.record_count.to_le_bytes().to_vec()
    }

    async fn write(&self, lake: Arc<DataLakeConnection>, logger: Arc<dyn Logger>) -> Result<()> {
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
        logger.write_log_entry(format!("writing {desc}")).await?;
        let df = self.ctx.sql(&self.transform_query).await?;
        let schema = df.schema().inner().clone();
        let mut stream = df.execute_stream().await?;

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let join_handle = tokio::spawn(write_partition_from_rows(
            lake.clone(),
            self.view_metadata.clone(),
            schema,
            self.begin_insert,
            self.end_insert,
            self.get_source_data_hash(),
            rx,
            logger.clone(),
        ));

        while let Some(rb_res) = stream.next().await {
            let rb = rb_res?;
            let df = self.ctx.read_batch(rb.clone())?;
            let df = df.aggregate(
                vec![],
                vec![
                    min(col(&*self.event_time_column)),
                    max(col(&*self.event_time_column)),
                ],
            )?;
            let minmax = df.collect().await?;
            if minmax.len() != 1 {
                anyhow::bail!("expected minmax to be size 1");
            }
            let minmax = &minmax[0];
            let min_column: &TimestampNanosecondArray = typed_column(minmax, 0)?;
            let max_column: &TimestampNanosecondArray = typed_column(minmax, 1)?;
            if min_column.is_empty() || max_column.is_empty() {
                anyhow::bail!("expected minmax to be size 1");
            }
            tx.send(PartitionRowSet {
                min_time_row: DateTime::from_timestamp_nanos(min_column.value(0)),
                max_time_row: DateTime::from_timestamp_nanos(max_column.value(0)),
                rows: rb,
            })
            .await?;
        }
        drop(tx);
        join_handle.await??;
        Ok(())
    }
}

pub async fn fetch_sql_partition_spec(
    ctx: SessionContext,
    transform_query: Arc<String>,
    event_time_column: Arc<String>,
    view_metadata: ViewMetadata,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
) -> Result<SqlPartitionSpec> {
    let df = ctx.sql("SELECT COUNT(*) as count FROM source;").await?;
    let batches: Vec<RecordBatch> = df.collect().await?;
    if batches.len() != 1 {
        anyhow::bail!("fetch_sql_partition_spec: query should return a single batch");
    }
    let rb = &batches[0];
    let count_column: &Int64Array = typed_column_by_name(rb, "count")?;
    if count_column.len() != 1 {
        anyhow::bail!("fetch_sql_partition_spec: query should return a single row");
    }
    let count = count_column.value(0);
    Ok(SqlPartitionSpec::new(
        ctx,
        transform_query,
        event_time_column,
        view_metadata,
        begin_insert,
        end_insert,
        count,
    ))
}
