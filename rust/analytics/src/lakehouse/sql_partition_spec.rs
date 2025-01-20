use super::view::PartitionSpec;
use crate::{dfext::get_column::get_column, response_writer::Logger};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::array::{Int64Array, RecordBatch},
    prelude::*,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

pub struct SqlPartitionSpec {
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
    record_count: i64,
}

impl SqlPartitionSpec {
    pub fn new(begin_insert: DateTime<Utc>, end_insert: DateTime<Utc>, record_count: i64) -> Self {
        Self {
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
    async fn write(&self, _lake: Arc<DataLakeConnection>, _logger: Arc<dyn Logger>) -> Result<()> {
        todo!();
    }
}

pub async fn fetch_sql_partition_spec(
    ctx: SessionContext,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
) -> Result<SqlPartitionSpec> {
    let df = ctx.sql("SELECT COUNT(*) as count FROM source;").await?;
    let batches: Vec<RecordBatch> = df.collect().await?;
    if batches.len() != 1 {
        anyhow::bail!("fetch_sql_partition_spec: query should return a single batch");
    }
    let rb = &batches[0];
    let count_column: &Int64Array = get_column(rb, "count")?;
    if count_column.len() != 1 {
        anyhow::bail!("fetch_sql_partition_spec: query should return a single row");
    }
    let count = count_column.value(0);
    Ok(SqlPartitionSpec::new(begin_insert, end_insert, count))
}
