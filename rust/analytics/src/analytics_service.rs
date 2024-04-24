use anyhow::{Context, Result};
use bytes::Buf;
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use serde::Deserialize;

use crate::sql_arrow_bridge::rows_to_record_batch;

#[derive(Debug, Clone)]
pub struct AnalyticsService {
    data_lake: DataLakeConnection,
}

#[derive(Debug, Deserialize)]
pub struct QueryProcessesRequest {
    pub limit: i64,
}

impl AnalyticsService {
    pub fn new(data_lake: DataLakeConnection) -> Self {
        Self { data_lake }
    }

    pub async fn query_processes(&self, body: bytes::Bytes) -> Result<RecordBatch> {
        let request: QueryProcessesRequest = ciborium::from_reader(body.reader())
            .with_context(|| "parsing QueryProcessesRequest")?;
        let mut connection = self.data_lake.db_pool.acquire().await?;
        let rows = sqlx::query(
            "SELECT process_id, tsc_frequency
             FROM processes
             LIMIT $1",
        )
        .bind(request.limit)
        .fetch_all(&mut *connection)
        .await?;
        rows_to_record_batch(&rows)
    }
}
