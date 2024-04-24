use anyhow::Result;
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;

use crate::sql_arrow_bridge::rows_to_record_batch;

#[derive(Debug, Clone)]
pub struct AnalyticsService {
    data_lake: DataLakeConnection,
}

impl AnalyticsService {
    pub fn new(data_lake: DataLakeConnection) -> Self {
        Self { data_lake }
    }

    pub async fn query_processes(&self, limit: i64) -> Result<RecordBatch> {
        let mut connection = self.data_lake.db_pool.acquire().await?;
        let rows = sqlx::query(
            "SELECT process_id, tsc_frequency
             FROM processes
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&mut *connection)
        .await?;
        rows_to_record_batch(&rows)
    }
}
