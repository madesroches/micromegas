use anyhow::{Context, Result};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::arrow::array::ListBuilder;
use datafusion::common::arrow::array::StructBuilder;
use datafusion::common::cast::as_struct_array;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;

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
        let _rows = sqlx::query(
            "SELECT process_id
             FROM processes
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&mut *connection)
        .await?;
        let mut list_builder = ListBuilder::new(StructBuilder::from_fields([], 0));
        let array = list_builder.finish();
        Ok(as_struct_array(array.values())
            .with_context(|| "casting list values to struct srray")?
            .into())
    }
}
