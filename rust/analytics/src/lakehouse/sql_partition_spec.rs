use super::{
    view::{PartitionSpec, ViewMetadata},
    write_partition::write_partition_from_rows,
};
use crate::{
    dfext::{min_max_time_df::min_max_time_dataframe, typed_column::typed_column_by_name},
    lakehouse::write_partition::PartitionRowSet,
    record_batch_transformer::RecordBatchTransformer,
    response_writer::Logger,
    time::TimeRange,
};
use anyhow::Result;
use async_trait::async_trait;
use datafusion::{
    arrow::{
        array::{Int64Array, RecordBatch},
        datatypes::Schema,
    },
    prelude::*,
};
use futures::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::trace;
use std::sync::Arc;

/// A `PartitionSpec` implementation for SQL-defined partitions.
pub struct SqlPartitionSpec {
    ctx: SessionContext,
    transformer: Arc<dyn RecordBatchTransformer>,
    schema: Arc<Schema>,
    extract_query: String,
    min_event_time_column: Arc<String>,
    max_event_time_column: Arc<String>,
    view_metadata: ViewMetadata,
    insert_range: TimeRange,
    record_count: i64,
}

impl SqlPartitionSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: SessionContext,
        transformer: Arc<dyn RecordBatchTransformer>,
        schema: Arc<Schema>,
        extract_query: String,
        min_event_time_column: Arc<String>,
        max_event_time_column: Arc<String>,
        view_metadata: ViewMetadata,
        insert_range: TimeRange,
        record_count: i64,
    ) -> Self {
        Self {
            ctx,
            transformer,
            schema,
            extract_query,
            min_event_time_column,
            max_event_time_column,
            view_metadata,
            insert_range,
            record_count,
        }
    }
}

impl std::fmt::Debug for SqlPartitionSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SqlPartitionSpec")
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
            self.insert_range.begin.to_rfc3339(),
            self.insert_range.end.to_rfc3339()
        );
        logger.write_log_entry(format!("writing {desc}")).await?;
        let df = self.ctx.sql(&self.extract_query).await?;
        let mut stream = df.execute_stream().await?;

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let join_handle = tokio::spawn(write_partition_from_rows(
            lake.clone(),
            self.view_metadata.clone(),
            self.schema.clone(),
            self.insert_range,
            self.get_source_data_hash(),
            rx,
            logger.clone(),
        ));

        while let Some(rb_res) = stream.next().await {
            let rb = self.transformer.transform(rb_res?).await?;
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

/// Fetches a `SqlPartitionSpec` by executing a count query and an extract query.
#[allow(clippy::too_many_arguments)]
pub async fn fetch_sql_partition_spec(
    ctx: SessionContext,
    transformer: Arc<dyn RecordBatchTransformer>,
    schema: Arc<Schema>,
    count_src_sql: String,
    extract_query: String,
    min_event_time_column: Arc<String>,
    max_event_time_column: Arc<String>,
    view_metadata: ViewMetadata,
    insert_range: TimeRange,
) -> Result<SqlPartitionSpec> {
    let df = ctx.sql(&count_src_sql).await?;
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
    if count > 0 {
        trace!(
            "fetch_sql_partition_spec for view {}, count={count}",
            &*view_metadata.view_set_name
        );
    }
    Ok(SqlPartitionSpec::new(
        ctx,
        transformer,
        schema,
        extract_query,
        min_event_time_column,
        max_event_time_column,
        view_metadata,
        insert_range,
        count,
    ))
}
