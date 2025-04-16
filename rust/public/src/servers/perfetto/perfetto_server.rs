use crate::client::{
    flightsql_client_factory::FlightSQLClientFactory, perfetto_trace_client::format_perfetto_trace,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use micromegas_analytics::time::TimeRange;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct FetchTraceParams {
    pub process_id: String,
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

pub struct PerfettoTraceServer {
    pub client_factory: Arc<dyn FlightSQLClientFactory>,
}

impl PerfettoTraceServer {
    pub fn new(client_factory: Arc<dyn FlightSQLClientFactory>) -> Self {
        Self { client_factory }
    }
}

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
    let buffer = format_perfetto_trace(&mut client, &process_id, TimeRange::new(begin, end))
        .await
        .with_context(|| "format_perfetto_trace")?;
    Ok(buffer.into())
}
