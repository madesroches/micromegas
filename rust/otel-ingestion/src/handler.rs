//! Glue between OTLP request bytes and the micromegas ingestion service.
//!
//! Stays free of HTTP framework types so the same path works under axum, integration
//! tests, or anything else that hands us a buffer of bytes. Errors map onto the OTLP/HTTP
//! response surface in the server crate.

use crate::block::{
    ProcessFromResource, split_logs, split_logs_with_extra_hash_input, split_metrics, split_traces,
};
use crate::error::{OtelError, Signal};
use crate::proto::{
    AnyValue, ExportLogsServiceRequest, ExportLogsServiceResponse, ExportMetricsServiceRequest,
    ExportMetricsServiceResponse, ExportTraceServiceRequest, ExportTraceServiceResponse,
    InstrumentationScope, KeyValue, LogRecord, Resource, ResourceLogs, ScopeLogs, SeverityNumber,
    any_value,
};
use crate::{
    FORMAT_OTLP_LOGS, FORMAT_OTLP_METRICS, FORMAT_OTLP_TRACES, OTLP_TICKS_PER_SECOND, TAG_LOGS,
    TAG_METRICS, TAG_TRACES,
};
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_tracing::prelude::*;
use prost::Message;
use serde::de::DeserializeOwned;
use std::sync::Arc;

/// Wire encoding negotiated from the request `Content-Type` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Protobuf,
    Json,
}

fn parse<M: Message + Default + DeserializeOwned>(
    body: &[u8],
    signal: Signal,
    encoding: Encoding,
) -> Result<M, OtelError> {
    match encoding {
        Encoding::Protobuf => M::decode(body).map_err(|e| OtelError::Parse {
            signal,
            message: format!("decoding {} (protobuf): {e}", signal.as_str()),
        }),
        Encoding::Json => serde_json::from_slice(body).map_err(|e| OtelError::Parse {
            signal,
            message: format!("decoding {} (json): {e}", signal.as_str()),
        }),
    }
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
async fn write_blocks(
    service: &WebIngestionService,
    signal: Signal,
    blocks: Vec<crate::block::PreparedBlock>,
) -> Result<usize, OtelError> {
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
                OTLP_TICKS_PER_SECOND,
                proc_attrs.start_time,
                proc_attrs.start_ticks,
                proc_attrs.properties,
            )
            .await
            .map_err(|e| OtelError::from_ingestion(e, signal))?;

        // Register the stream row (idempotent).
        service
            .register_otel_stream(
                prepared.stream_id,
                prepared.process_id,
                vec![tag.clone()],
                format,
            )
            .await
            .map_err(|e| OtelError::from_ingestion(e, signal))?;

        // Write the block.
        service
            .insert_block_typed(prepared.block)
            .await
            .map_err(|e| OtelError::from_ingestion(e, signal))?;

        count += 1;
    }

    debug!("wrote {count} OTel {} blocks", signal.as_str());
    Ok(count)
}

/// OTLP/HTTP `POST /v1/logs` handler.
pub async fn ingest_logs(
    service: Arc<WebIngestionService>,
    body: bytes::Bytes,
    encoding: Encoding,
) -> Result<ExportLogsServiceResponse, OtelError> {
    let req: ExportLogsServiceRequest = parse(&body, Signal::Logs, encoding)?;
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
    encoding: Encoding,
) -> Result<ExportMetricsServiceResponse, OtelError> {
    let req: ExportMetricsServiceRequest = parse(&body, Signal::Metrics, encoding)?;
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
    encoding: Encoding,
) -> Result<ExportTraceServiceResponse, OtelError> {
    let req: ExportTraceServiceRequest = parse(&body, Signal::Traces, encoding)?;
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

/// Builds a synthetic `ExportLogsServiceRequest` carrying a single resource, single
/// scope, single log record whose body is the webhook request body, stored as
/// `StringValue`. Valid-UTF8 bodies (the common case: JSON payloads from
/// GitLab/GitHub/etc.) are stored verbatim; a non-UTF8 body is stored via lossy
/// UTF-8 conversion (invalid byte sequences become U+FFFD) rather than rejected or
/// stored as opaque binary — there is no header to describe an alternate codec, so
/// there is no way to decode it losslessly. `time_unix_nano` / `observed_time_unix_nano`
/// are left at 0 so `split_logs`'s existing backfill stamps ingestion time.
///
/// Leaving both timestamps at 0 means `split_logs` backfills on every single webhook
/// delivery (never just the rare real-OTLP case), so its "only re-encode when mutated"
/// optimization never applies here: every webhook request pays for two full
/// `ResourceLogs::encode_to_vec()` calls (pre- and post-backfill) instead of one. This is
/// an accepted, bounded tradeoff — bounded by the ~300 MiB decompressed body cap in
/// `rust/public/src/servers/ingestion_limits.rs` — required to keep `block_id` both
/// content-addressed (hashed from the pre-backfill bytes, so retried deliveries dedup) and
/// independent of wall-clock ingestion time (see `split_logs`'s doc comment and
/// `tasks/1296_webhook_ingestion_plan.md`'s "Idempotency / dedup" section). Stamping a real
/// timestamp here instead of 0 would avoid the double encode but would break dedup: the
/// pre-backfill bytes hashed into `block_id` would then include a live, ever-changing
/// timestamp, so identical retried bodies would hash to different `block_id`s.
///
/// Public (rather than private) so `tests/webhook_tests.rs` can assert its shape directly.
pub fn build_webhook_request(
    resource_attrs: Vec<KeyValue>,
    target: String,
    body: &[u8],
) -> ExportLogsServiceRequest {
    let body_str = String::from_utf8_lossy(body).into_owned();
    let record = LogRecord {
        time_unix_nano: 0,
        observed_time_unix_nano: 0,
        severity_number: SeverityNumber::Info as i32,
        severity_text: String::new(),
        body: Some(AnyValue {
            value: Some(any_value::Value::StringValue(body_str)),
        }),
        attributes: vec![],
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: vec![],
        span_id: vec![],
        event_name: String::new(),
    };
    let scope_logs = ScopeLogs {
        scope: Some(InstrumentationScope {
            name: target,
            version: String::new(),
            attributes: vec![],
            dropped_attributes_count: 0,
        }),
        log_records: vec![record],
        schema_url: String::new(),
    };
    let resource_logs = ResourceLogs {
        resource: Some(Resource {
            attributes: resource_attrs,
            dropped_attributes_count: 0,
            entity_refs: vec![],
        }),
        scope_logs: vec![scope_logs],
        schema_url: String::new(),
    };
    ExportLogsServiceRequest {
        resource_logs: vec![resource_logs],
    }
}

/// Generic webhook → single-log-record ingestion.
/// Builds a synthetic `ExportLogsServiceRequest` (one resource, one scope, one record whose
/// body is the request body, stored verbatim for valid UTF-8 or via lossy conversion
/// otherwise) and reuses the OTLP logs split/write path.
///
/// `header_hash_input` is the caller's canonicalized encoding of the *full* incoming HTTP
/// header set (see `webhook::canonical_header_bytes`), folded into `block_id` alongside the
/// synthetic request bytes — see `split_logs_with_extra_hash_input` for why this matters:
/// only 3 headers become resource attrs, so without this, unrecognized headers would be
/// invisible to the dedup hash.
pub async fn ingest_webhook(
    service: Arc<WebIngestionService>,
    resource_attrs: Vec<KeyValue>,
    target: String,
    body: bytes::Bytes,
    header_hash_input: &[u8],
) -> Result<(), OtelError> {
    let req = build_webhook_request(resource_attrs, target, &body);
    let blocks =
        split_logs_with_extra_hash_input(req, header_hash_input).map_err(|e| OtelError::Parse {
            signal: Signal::Logs,
            message: format!("split_logs (webhook): {e}"),
        })?;
    write_blocks(&service, Signal::Logs, blocks).await?;
    Ok(())
}
