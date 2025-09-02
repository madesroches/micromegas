use crate::client::{
    flightsql_client_factory::FlightSQLClientFactory, perfetto_trace_client::format_perfetto_trace,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use micromegas_analytics::time::TimeRange;
use serde::Deserialize;
use std::sync::Arc;

/// Parameters for fetching a Perfetto trace.
#[derive(Debug, Deserialize)]
pub struct FetchTraceParams {
    pub process_id: String,
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// A server for serving Perfetto traces.
pub struct PerfettoTraceServer {
    pub client_factory: Arc<dyn FlightSQLClientFactory>,
}

impl PerfettoTraceServer {
    /// Creates a new `PerfettoTraceServer`.
    ///
    /// # Arguments
    ///
    /// * `client_factory` - A factory for creating FlightSQL clients.
    pub fn new(client_factory: Arc<dyn FlightSQLClientFactory>) -> Self {
        Self { client_factory }
    }
}

/// Shows a Perfetto trace in a web browser.
///
/// This function generates an HTML page that embeds the Perfetto UI
/// and loads the trace data from the `/fetch_trace` endpoint.
///
/// # Arguments
///
/// * `caller` - The name of the caller, used for display in the HTML.
/// * `params` - Parameters for fetching the trace, including process ID and time range.
pub async fn show_trace(caller: &str, params: FetchTraceParams) -> Result<String> {
    let process_id = params.process_id;
    let begin = params.begin.to_rfc3339();
    let end = params.end.to_rfc3339();
    let content = std::fmt::format(format_args!(
        include_str!("show_trace.html"),
        caller = caller,
        process_id = process_id,
        begin = begin,
        end = end
    ));
    Ok(content)
}

/// Fetches a Perfetto trace.
///
/// This function retrieves the trace data from the FlightSQL server
/// and returns it as a `bytes::Bytes` object.
///
/// # Arguments
///
/// * `server` - The `PerfettoTraceServer` instance.
/// * `_caller` - The name of the caller (unused in this function).
/// * `params` - Parameters for fetching the trace, including process ID and time range.
pub async fn fetch_trace(
    server: Arc<PerfettoTraceServer>,
    _caller: &str,
    params: FetchTraceParams,
) -> Result<bytes::Bytes> {
    let process_id = params.process_id;
    let begin = params.begin;
    let end = params.end;
    let mut client = server
        .client_factory
        .make_client()
        .await
        .with_context(|| "make_client")?;
    let buffer = format_perfetto_trace(
        &mut client,
        &process_id,
        TimeRange::new(begin, end),
        crate::client::SpanTypes::Both,
    )
    .await
    .with_context(|| "format_perfetto_trace")?;
    Ok(buffer.into())
}
