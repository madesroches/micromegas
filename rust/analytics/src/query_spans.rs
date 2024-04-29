use crate::{arrow_utils::make_empty_record_batch, metadata::find_stream};
use anyhow::{Context, Result};
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use sqlx::types::chrono::{DateTime, Utc};

pub async fn query_spans(
    data_lake: &DataLakeConnection,
    stream_id: &str,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<RecordBatch> {
    let mut connection = data_lake.db_pool.acquire().await?;
    let stream_info = find_stream(&mut connection, stream_id)
        .await
        .with_context(|| "find_stream")?;
    dbg!(stream_info);
    Ok(make_empty_record_batch())
}
