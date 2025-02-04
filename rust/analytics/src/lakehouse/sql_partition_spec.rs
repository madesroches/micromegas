use super::{
    view::{PartitionSpec, ViewMetadata},
    write_partition::write_partition_from_rows,
};
use crate::{
    dfext::{min_max_time_df::min_max_time_dataframe, typed_column::typed_column_by_name},
    lakehouse::write_partition::PartitionRowSet,
    response_writer::Logger,
};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::array::{Int64Array, RecordBatch},
    prelude::*,
};
use futures::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::debug;
use std::sync::Arc;

pub struct SqlPartitionSpec {
    ctx: SessionContext,
    transform_query: Arc<String>,
    min_event_time_column: Arc<String>,
    max_event_time_column: Arc<String>,
    view_metadata: ViewMetadata,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    record_count: i64,
}

impl SqlPartitionSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: SessionContext,
        transform_query: Arc<String>,
        min_event_time_column: Arc<String>,
        max_event_time_column: Arc<String>,
        view_metadata: ViewMetadata,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
        record_count: i64,
    ) -> Self {
        Self {
            ctx,
            transform_query,
            min_event_time_column,
            max_event_time_column,
            view_metadata,
            begin_insert,
            end_insert,
            record_count,
        }
    }
}

#[async_trait]
impl PartitionSpec for SqlPartitionSpec {
    fn is_empty(&self) -> bool {
        self.record_count < 1
    }

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
            let (mintime, maxtime) = min_max_time_dataframe(
                self.ctx.read_batch(rb.clone())?,
                &self.min_event_time_column,
                &self.max_event_time_column,
            )
            .await?;
            tx.send(PartitionRowSet {
                min_time_row: mintime,
                max_time_row: maxtime,
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
    min_event_time_column: Arc<String>,
    max_event_time_column: Arc<String>,
    view_metadata: ViewMetadata,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
) -> Result<SqlPartitionSpec> {
    debug!(
        "fetch_sql_partition_spec for view {}",
        &*view_metadata.view_set_name
    );
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
    debug!(
        "fetch_sql_partition_spec for view {}, count={count}",
        &*view_metadata.view_set_name
    );
    Ok(SqlPartitionSpec::new(
        ctx,
        transform_query,
        min_event_time_column,
        max_event_time_column,
        view_metadata,
        begin_insert,
        end_insert,
        count,
    ))
}
