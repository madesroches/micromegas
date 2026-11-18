use super::provider::CallContext;
use crate::client::flightsql_client::QueryEstimate;
use crate::client::flightsql_client_factory::{
    BearerFlightSQLClientFactory, FlightSQLClientFactory,
};
use anyhow::Result;
use arrow::array::RecordBatch;
use arrow_flight::error::FlightError;
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use micromegas_telemetry::time_range::TimeRange;
use std::sync::Arc;

/// Where queries go. Resolved per call so a host can follow a setting that changes at runtime.
#[async_trait]
pub trait FlightUrlResolver: Send + Sync {
    async fn resolve(&self) -> Result<String>;
}

pub struct FixedUrl(pub String);

#[async_trait]
impl FlightUrlResolver for FixedUrl {
    async fn resolve(&self) -> Result<String> {
        Ok(self.0.clone())
    }
}

/// The FlightSQL operations the query tools need. A seam so the tools can be tested without a
/// server.
#[async_trait]
pub trait FlightBackend: Send + Sync {
    /// Plans the query without running it.
    async fn prepare(
        &self,
        call: &CallContext,
        sql: &str,
        range: Option<TimeRange>,
    ) -> Result<QueryEstimate>;

    /// Starts the query, asking the server for at most `row_limit` rows. Dropping the stream
    /// cancels it.
    async fn execute(
        &self,
        call: &CallContext,
        sql: &str,
        range: Option<TimeRange>,
        row_limit: usize,
    ) -> Result<BoxStream<'static, Result<RecordBatch>>>;
}

/// Talks to a FlightSQL server over gRPC, forwarding the caller's bearer token.
pub struct GrpcFlightBackend {
    resolver: Arc<dyn FlightUrlResolver>,
}

impl GrpcFlightBackend {
    pub fn new(resolver: Arc<dyn FlightUrlResolver>) -> Self {
        Self { resolver }
    }

    // `build_origin_metadata` in the HTTP gateway is not reusable: it suffixes the client type
    // with `+gateway` and echoes a caller-supplied request id.
    async fn make_client(
        &self,
        call: &CallContext,
    ) -> Result<crate::client::flightsql_client::Client> {
        let url = self.resolver.resolve().await?;
        let mut factory = BearerFlightSQLClientFactory::new_with_client_type(
            url,
            call.authorization.clone(),
            "mcp".to_string(),
        )
        .with_metadata("x-client-agent", sanitize_header(&call.client_agent))
        .with_metadata("x-request-id", uuid::Uuid::new_v4().to_string());
        if let Some(ip) = forwarded_for_value(&call.client_ip) {
            factory = factory.with_metadata("x-forwarded-for", ip);
        }
        factory.make_client().await
    }
}

// FlightSQL resolves the caller address from X-Forwarded-For (see `resolve_client_ip`) and
// ignores any custom header, so this is what makes allowlists and the audit log see the agent
// rather than this server. Unparseable values are dropped: the server would ignore them anyway.
fn forwarded_for_value(client_ip: &str) -> Option<String> {
    client_ip
        .trim()
        .parse::<std::net::IpAddr>()
        .ok()
        .map(|ip| ip.to_string())
}

/// gRPC metadata values must be visible ASCII.
fn sanitize_header(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

#[async_trait]
impl FlightBackend for GrpcFlightBackend {
    async fn prepare(
        &self,
        call: &CallContext,
        sql: &str,
        range: Option<TimeRange>,
    ) -> Result<QueryEstimate> {
        let mut client = self.make_client(call).await?;
        client.prepare_query(sql.to_string(), range).await
    }

    async fn execute(
        &self,
        call: &CallContext,
        sql: &str,
        range: Option<TimeRange>,
        row_limit: usize,
    ) -> Result<BoxStream<'static, Result<RecordBatch>>> {
        let mut client = self.make_client(call).await?;
        client
            .inner_mut()
            .set_header("limit", row_limit.to_string());
        let stream = client.query_stream(sql.to_string(), range).await?;
        Ok(stream.map(|r| r.map_err(anyhow::Error::from)).boxed())
    }
}

/// Message shown to the agent for a failed call, naming the gRPC code when there is one.
pub fn describe_error(error: &anyhow::Error) -> String {
    let status = error.downcast_ref::<tonic::Status>().or_else(|| {
        match error.downcast_ref::<FlightError>() {
            Some(FlightError::Tonic(status)) => Some(status),
            _ => None,
        }
    });
    match status {
        Some(status) => format!("{:?}: {}", status.code(), status.message()),
        None => format!("{error:#}"),
    }
}

#[cfg(test)]
mod forwarded_for_tests {
    use super::forwarded_for_value;

    #[test]
    fn forwards_only_parseable_ips() {
        assert_eq!(forwarded_for_value("10.1.2.3").as_deref(), Some("10.1.2.3"));
        assert_eq!(forwarded_for_value("::1").as_deref(), Some("::1"));
        assert_eq!(forwarded_for_value("unknown"), None);
    }
}
