//! Glue between OTLP request bytes and the micromegas ingestion service.
//!
//! Stays free of HTTP framework types so the same path works under axum, integration
//! tests, or anything else that hands us a buffer of bytes. Errors map onto the OTLP/HTTP
//! response surface in the server crate.

use crate::block::{ProcessFromResource, split_logs, split_metrics, split_traces};
use crate::error::{OtelError, Signal};
use crate::proto::{
    ExportLogsServiceRequest, ExportLogsServiceResponse, ExportMetricsServiceRequest,
    ExportMetricsServiceResponse, ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use crate::{
    FORMAT_OTLP_LOGS, FORMAT_OTLP_METRICS, FORMAT_OTLP_TRACES, TAG_LOGS, TAG_METRICS, TAG_TRACES,
};
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_tracing::prelude::*;
use prost::Message;
use std::sync::Arc;

fn parse<M: Message + Default>(body: &[u8], signal: Signal) -> Result<M, OtelError> {
    M::decode(body).map_err(|e| OtelError::Parse {
        signal,
        message: format!("decoding {}: {}", signal.as_str(), e),
    })
}

fn signal_tag(signal: Signal) -> &'static str {
    match signal {
        Signal::Logs => TAG_LOGS,
        Signal::Metrics => TAG_METRICS,
        Signal::Traces => TAG_TRACES,
    }
}

fn signal_format(signal: Signal) -> &'static str {
    match signal {
        Signal::Logs => FORMAT_OTLP_LOGS,
        Signal::Metrics => FORMAT_OTLP_METRICS,
        Signal::Traces => FORMAT_OTLP_TRACES,
    }
}

/// Generic per-resource block writer. Registers the process + stream (idempotent)
/// then writes one block per resource. All errors carry the signal label so the
/// HTTP response includes useful context.
async fn write_blocks<I>(
    service: &WebIngestionService,
    signal: Signal,
    blocks: I,
) -> Result<usize, OtelError>
where
    I: IntoIterator<Item = crate::block::PreparedBlock>,
{
    let tag = signal_tag(signal).to_string();
    let format = signal_format(signal);
    let mut count = 0usize;

    for prepared in blocks {
        // Register the process row (idempotent).
        let proc_attrs = ProcessFromResource::build(&prepared.resource_attrs, prepared.begin_time);
        service
            .register_otel_process(
                prepared.process_id,
                proc_attrs.exe,
                proc_attrs.username,
                proc_attrs.computer,
                proc_attrs.distro,
                proc_attrs.cpu_brand,
                1_000_000_000,
                proc_attrs.start_time,
                proc_attrs.start_ticks,
                proc_attrs.properties,
            )
            .await
            .map_err(|e| OtelError::from(e).with_signal(signal))?;

        // Register the stream row (idempotent). Empty properties — scope and per-event
        // attrs live on individual rows during materialization, not on the stream.
        service
            .register_otel_stream(
                prepared.stream_id,
                prepared.process_id,
                vec![tag.clone()],
                Vec::new(),
                format,
            )
            .await
            .map_err(|e| OtelError::from(e).with_signal(signal))?;

        // Write the block.
        service
            .insert_block_typed(prepared.block)
            .await
            .map_err(|e| OtelError::from(e).with_signal(signal))?;

        count += 1;
    }

    debug!("wrote {count} OTel {} blocks", signal.as_str());
    Ok(count)
}

/// OTLP/HTTP `POST /v1/logs` handler.
pub async fn ingest_logs(
    service: Arc<WebIngestionService>,
    body: bytes::Bytes,
) -> Result<ExportLogsServiceResponse, OtelError> {
    let req: ExportLogsServiceRequest = parse(&body, Signal::Logs)?;
    if req.resource_logs.is_empty() {
        return Ok(ExportLogsServiceResponse::default());
    }
    let blocks = split_logs(req).map_err(|e| OtelError::Parse {
        signal: Signal::Logs,
        message: format!("split_logs: {e}"),
    })?;
    write_blocks(&service, Signal::Logs, blocks).await?;
    Ok(ExportLogsServiceResponse::default())
}

/// OTLP/HTTP `POST /v1/metrics` handler.
pub async fn ingest_metrics(
    service: Arc<WebIngestionService>,
    body: bytes::Bytes,
) -> Result<ExportMetricsServiceResponse, OtelError> {
    let req: ExportMetricsServiceRequest = parse(&body, Signal::Metrics)?;
    if req.resource_metrics.is_empty() {
        return Ok(ExportMetricsServiceResponse::default());
    }
    let blocks = split_metrics(req).map_err(|e| OtelError::Parse {
        signal: Signal::Metrics,
        message: format!("split_metrics: {e}"),
    })?;
    write_blocks(&service, Signal::Metrics, blocks).await?;
    Ok(ExportMetricsServiceResponse::default())
}

/// OTLP/HTTP `POST /v1/traces` handler.
pub async fn ingest_traces(
    service: Arc<WebIngestionService>,
    body: bytes::Bytes,
) -> Result<ExportTraceServiceResponse, OtelError> {
    let req: ExportTraceServiceRequest = parse(&body, Signal::Traces)?;
    if req.resource_spans.is_empty() {
        return Ok(ExportTraceServiceResponse::default());
    }
    let blocks = split_traces(req).map_err(|e| OtelError::Parse {
        signal: Signal::Traces,
        message: format!("split_traces: {e}"),
    })?;
    write_blocks(&service, Signal::Traces, blocks).await?;
    Ok(ExportTraceServiceResponse::default())
}
