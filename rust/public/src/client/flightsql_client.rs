use anyhow::{Context, Result};
use arrow_flight::{decode::FlightRecordBatchStream, sql::client::FlightSqlServiceClient};
use datafusion::arrow::array::RecordBatch;
use futures::stream::StreamExt;
use micromegas_analytics::time::TimeRange;
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

    fn set_query_range(&mut self, query_range: Option<TimeRange>) {
        self.inner.set_header(
            "query_range_begin",
            query_range.map_or(String::from(""), |r| r.begin.to_rfc3339()),
        );
        self.inner.set_header(
            "query_range_end",
            query_range.map_or(String::from(""), |r| r.end.to_rfc3339()),
        );
    }

    /// Execute SQL query
    pub async fn query(
        &mut self,
        sql: String,
        query_range: Option<TimeRange>,
    ) -> Result<Vec<RecordBatch>> {
        let mut record_batch_stream = self.query_stream(sql, query_range).await?;
        let mut batches = vec![];
        while let Some(batch_res) = record_batch_stream.next().await {
            batches.push(batch_res?);
        }
        Ok(batches)
    }

    pub async fn query_stream(
        &mut self,
        sql: String,
        query_range: Option<TimeRange>,
    ) -> Result<FlightRecordBatchStream> {
        self.set_query_range(query_range);
        let info = self.inner.execute(sql, None).await?;
        let ticket = info.endpoint[0]
            .ticket
            .clone()
            .with_context(|| "reading ticket from endpoint")?;
        let flight_data_stream = self.inner.do_get(ticket).await?.into_inner();
        Ok(FlightRecordBatchStream::new(flight_data_stream))
    }
}
