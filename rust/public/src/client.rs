use anyhow::{Context, Result};
use arrow_flight::{decode::FlightRecordBatchStream, sql::client::FlightSqlServiceClient};
use chrono::{DateTime, Utc};
use datafusion::arrow::array::RecordBatch;
use futures::stream::StreamExt;
use tonic::transport::Channel;

/// Micromegas FlightSQL client
pub struct Client {
    inner: FlightSqlServiceClient<Channel>,
}

impl Client {
    /// Creates a new client from a grpc channel
    pub fn new(channel: Channel) -> Self {
        let inner = FlightSqlServiceClient::new(channel);
        Self { inner }
    }

    pub fn inner_mut(&mut self) -> &mut FlightSqlServiceClient<Channel> {
        &mut self.inner
    }

    /// Execute SQL query
    pub async fn query(
        &mut self,
        sql: String,
        begin: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<RecordBatch>> {
        self.inner
            .set_header("query_range_begin", begin.to_rfc3339());
        self.inner.set_header("query_range_end", end.to_rfc3339());
        let info = self.inner.execute(sql, None).await?;
        let ticket = info.endpoint[0]
            .ticket
            .clone()
            .with_context(|| "reading ticket from endpoint")?;
        let flight_data_stream = self.inner.do_get(ticket).await?.into_inner();
        let mut record_batch_stream = FlightRecordBatchStream::new(flight_data_stream);
        let mut batches = vec![];
        while let Some(batch_res) = record_batch_stream.next().await {
            batches.push(batch_res?);
        }
        Ok(batches)
    }
}
