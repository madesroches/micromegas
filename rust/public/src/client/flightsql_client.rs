use anyhow::{Context, Result};
use arrow::{array::RecordBatch, datatypes::SchemaRef};
use arrow_flight::{
    decode::FlightRecordBatchStream, flight_service_client::FlightServiceClient,
    sql::client::FlightSqlServiceClient,
};
use futures::stream::StreamExt;
use micromegas_telemetry::time_range::TimeRange;
use std::sync::Arc;
use tonic::transport::Channel;

/// Represents a prepared statement with its schema and query string.
#[derive(Debug)]
pub struct PreparedStatement {
    pub schema: SchemaRef,
    pub query: String,
}

/// What the server reports about a query before running it.
#[derive(Debug)]
pub struct QueryEstimate {
    pub schema: SchemaRef,
    /// Estimated bytes the query will scan; `None` when the server reports no estimate.
    pub total_bytes: Option<u64>,
    /// How the estimate was obtained (e.g. `footer`, `file_size_share`, `opaque`).
    pub estimate_precision: Option<String>,
    /// Whether the estimate is within the caller's scan cap; `None` when not reported.
    pub within_scan_cap: Option<bool>,
}

/// Micromegas FlightSQL client
pub struct Client {
    inner: FlightSqlServiceClient<Channel>,
}

impl Client {
    /// Creates a new client from a grpc channel
    ///
    /// # Arguments
    ///
    /// * `channel` - The gRPC channel to use for communication.
    pub fn new(channel: Channel) -> Self {
        let flight_client =
            FlightServiceClient::new(channel).max_decoding_message_size(100 * 1024 * 1024);
        let inner = FlightSqlServiceClient::new_from_inner(flight_client);
        Self { inner }
    }

    /// Returns a mutable reference to the inner `FlightSqlServiceClient`.
    ///
    /// This allows direct access to the underlying FlightSQL client for advanced operations.
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

    /// Executes a SQL query and returns the results as a vector of `RecordBatch`.
    ///
    /// # Arguments
    ///
    /// * `sql` - The SQL query string to execute.
    /// * `query_range` - An optional `TimeRange` to filter the query results by time.
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

    /// Executes a SQL query and returns the results as a `FlightRecordBatchStream`.
    ///
    /// This function is useful for processing large query results incrementally.
    ///
    /// # Arguments
    ///
    /// * `sql` - The SQL query string to execute.
    /// * `query_range` - An optional `TimeRange` to filter the query results by time.
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

    /// Prepares a SQL statement and returns a `PreparedStatement`.
    ///
    /// This function allows to compute the schema of a query without actually executing it.
    ///
    /// # Arguments
    ///
    /// * `sql` - The SQL query string to prepare.
    pub async fn prepare_statement(&mut self, sql: String) -> Result<PreparedStatement> {
        self.set_query_range(None);
        let prepared = self.inner.prepare(sql.clone(), None).await?;
        Ok(PreparedStatement {
            schema: Arc::new(prepared.dataset_schema()?.clone()),
            query: sql,
        })
    }

    /// Plans a query without running it: returns its schema and the server's scan estimate.
    ///
    /// The estimate fields come from `FlightInfo.total_bytes` (negative means unknown) and an
    /// optional JSON `app_metadata` object with `estimate_precision` and `within_scan_cap`.
    /// A server that reports none of them yields `None` for each.
    ///
    /// # Arguments
    ///
    /// * `sql` - The SQL query string to plan.
    /// * `query_range` - An optional `TimeRange` the query would run over.
    pub async fn prepare_query(
        &mut self,
        sql: String,
        query_range: Option<TimeRange>,
    ) -> Result<QueryEstimate> {
        self.set_query_range(query_range);
        let prepared = self.inner.prepare(sql.clone(), None).await?;
        let schema = Arc::new(prepared.dataset_schema()?.clone());
        let info = self.inner.execute(sql, None).await?;
        let total_bytes = u64::try_from(info.total_bytes).ok();
        let metadata: serde_json::Value =
            serde_json::from_slice(&info.app_metadata).unwrap_or(serde_json::Value::Null);
        Ok(QueryEstimate {
            schema,
            total_bytes,
            estimate_precision: metadata
                .get("estimate_precision")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            within_scan_cap: metadata.get("within_scan_cap").and_then(|v| v.as_bool()),
        })
    }
}
