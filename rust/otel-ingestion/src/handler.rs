//! Glue between OTLP request bytes and the micromegas ingestion service.
//!
//! Stays free of HTTP framework types so the same path works under axum, integration
//! tests, or anything else that hands us a buffer of bytes. Errors map onto the OTLP/HTTP
//! response surface in the server crate.

use crate::block::{ProcessFromResource, split_logs, split_metrics, split_traces};
use crate::cloudwatch_metrics::rewrite_cloudwatch_metric_streams;
use crate::error::{OtelError, Signal};
use crate::identity::IdentityContext;
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
use base64::Engine as _;
use bytes::Buf;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_ingestion::write_audience::WriteAudience;
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

/// Decode the next length-delimited protobuf message from `buf`, advancing `buf` past it.
/// Returns `Ok(None)` once `buf` is exhausted (no more messages in this record).
/// CloudWatch Metric Streams' OpenTelemetry 1.0.0 output format packs one-or-more
/// `[varint32 length][message bytes]` entries per record, not a single unframed message
/// (see AWS's CloudWatch metric streams OpenTelemetry format docs).
pub fn decode_next_length_delimited<M: Message + Default>(
    buf: &mut &[u8],
    signal: Signal,
) -> Result<Option<M>, OtelError> {
    if !buf.has_remaining() {
        return Ok(None);
    }
    let message = M::decode_length_delimited(buf).map_err(|e| OtelError::Parse {
        signal,
        message: format!(
            "decoding {} (length-delimited protobuf): {e}",
            signal.as_str()
        ),
    })?;
    Ok(Some(message))
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
pub(crate) async fn write_blocks(
    service: &WebIngestionService,
    signal: Signal,
    blocks: Vec<crate::block::PreparedBlock>,
    audience: &WriteAudience,
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
                audience,
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
                audience,
            )
            .await
            .map_err(|e| OtelError::from_ingestion(e, signal))?;

        // Write the block.
        service
            .insert_block_typed(prepared.block, audience)
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
    audience: &WriteAudience,
) -> Result<ExportLogsServiceResponse, OtelError> {
    let req: ExportLogsServiceRequest = parse(&body, Signal::Logs, encoding)?;
    if req.resource_logs.is_empty() {
        return Ok(ExportLogsServiceResponse::default());
    }
    let ctx = IdentityContext {
        audience: audience.id_namespace(service.default_audience()),
        extra_hash_input: &[],
    };
    let blocks = split_logs(req, ctx).map_err(|e| OtelError::Parse {
        signal: Signal::Logs,
        message: format!("split_logs: {e}"),
    })?;
    write_blocks(&service, Signal::Logs, blocks, audience).await?;
    Ok(ExportLogsServiceResponse::default())
}

/// Splits and writes an already-decoded `ExportMetricsServiceRequest`. Factored out of
/// `ingest_metrics` so `ingest_firehose_metrics` can reuse the split/write half per
/// length-delimited message it decodes, without re-serializing back to bytes just to
/// re-decode via `ingest_metrics`.
async fn ingest_parsed_metrics(
    service: &WebIngestionService,
    req: ExportMetricsServiceRequest,
    audience: &WriteAudience,
) -> Result<(), OtelError> {
    if req.resource_metrics.is_empty() {
        return Ok(());
    }
    let ctx = IdentityContext {
        audience: audience.id_namespace(service.default_audience()),
        extra_hash_input: &[],
    };
    let blocks = split_metrics(req, ctx).map_err(|e| OtelError::Parse {
        signal: Signal::Metrics,
        message: format!("split_metrics: {e}"),
    })?;
    write_blocks(service, Signal::Metrics, blocks, audience).await?;
    Ok(())
}

/// OTLP/HTTP `POST /v1/metrics` handler.
pub async fn ingest_metrics(
    service: Arc<WebIngestionService>,
    body: bytes::Bytes,
    encoding: Encoding,
    audience: &WriteAudience,
) -> Result<ExportMetricsServiceResponse, OtelError> {
    let req: ExportMetricsServiceRequest = parse(&body, Signal::Metrics, encoding)?;
    ingest_parsed_metrics(&service, req, audience).await?;
    Ok(ExportMetricsServiceResponse::default())
}

/// OTLP/HTTP `POST /v1/traces` handler.
pub async fn ingest_traces(
    service: Arc<WebIngestionService>,
    body: bytes::Bytes,
    encoding: Encoding,
    audience: &WriteAudience,
) -> Result<ExportTraceServiceResponse, OtelError> {
    let req: ExportTraceServiceRequest = parse(&body, Signal::Traces, encoding)?;
    if req.resource_spans.is_empty() {
        return Ok(ExportTraceServiceResponse::default());
    }
    let ctx = IdentityContext {
        audience: audience.id_namespace(service.default_audience()),
        extra_hash_input: &[],
    };
    let blocks = split_traces(req, ctx).map_err(|e| OtelError::Parse {
        signal: Signal::Traces,
        message: format!("split_traces: {e}"),
    })?;
    write_blocks(&service, Signal::Traces, blocks, audience).await?;
    Ok(ExportTraceServiceResponse::default())
}

/// Builds a synthetic `ExportLogsServiceRequest` carrying a single resource, single
/// scope, single log record whose body is the webhook request body, stored as
/// `StringValue`. Valid-UTF8 bodies (the common case: JSON payloads from
/// GitLab/GitHub/etc.) are stored verbatim; a non-UTF8 body is stored via lossy
/// UTF-8 conversion (invalid byte sequences become U+FFFD) rather than rejected or
/// stored as opaque binary — there is no header to describe an alternate codec, so
/// there is no way to decode it losslessly. `time_unix_nano` / `observed_time_unix_nano`
/// are left at 0 on every record: `split_logs` no longer backfills, so the timestamps
/// stay 0 in the stored payload too, and a retried delivery always encodes to the exact
/// same bytes. `block_id` is hashed from those same stored bytes, so the two are
/// intentionally coupled — that's what keeps `block_id` both content-addressed (retried
/// deliveries with the same body dedup) and independent of wall-clock ingestion time (see
/// `split_logs`'s doc comment and `tasks/1296_webhook_ingestion_plan.md`'s "Idempotency /
/// dedup" section). The arrival-time fallback needed by the OTLP spec's "collecting system
/// supplies an observed timestamp" requirement lives in the block's `begin_time`/`end_time`
/// (`logs_bounds` → `build_prepared_block`) and, for materialization, in
/// `OtelLogsBlockProcessor`'s per-record substitution — not in the record itself. Stamping
/// a real timestamp here instead of 0 would break dedup: the bytes hashed into `block_id`
/// would then include a live, ever-changing timestamp, so identical retried bodies would
/// hash to different `block_id`s.
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
/// synthetic request bytes — see [`IdentityContext::extra_hash_input`] for why this matters:
/// only 3 headers become resource attrs, so without this, unrecognized headers would be
/// invisible to the dedup hash.
pub async fn ingest_webhook(
    service: Arc<WebIngestionService>,
    resource_attrs: Vec<KeyValue>,
    target: String,
    body: bytes::Bytes,
    header_hash_input: &[u8],
    audience: &WriteAudience,
) -> Result<(), OtelError> {
    let req = build_webhook_request(resource_attrs, target, &body);
    let ctx = IdentityContext {
        audience: audience.id_namespace(service.default_audience()),
        extra_hash_input: header_hash_input,
    };
    let blocks = split_logs(req, ctx).map_err(|e| OtelError::Parse {
        signal: Signal::Logs,
        message: format!("split_logs (webhook): {e}"),
    })?;
    write_blocks(&service, Signal::Logs, blocks, audience).await?;
    Ok(())
}

#[derive(serde::Deserialize)]
struct FirehoseRecordJson {
    data: String,
}

#[derive(serde::Deserialize)]
struct FirehoseEnvelopeJson {
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(default)]
    records: Vec<FirehoseRecordJson>,
}

/// Decoded Firehose envelope: the echoed request id plus each record's base64-decoded bytes.
#[derive(Debug)]
pub struct FirehoseEnvelope {
    pub request_id: String,
    pub records: Vec<Vec<u8>>,
}

/// Parse the Firehose JSON envelope and base64-decode every record's `data`.
/// (gzip, if any, is already removed by the shared decompression layer.)
/// Malformed JSON or base64 → `OtelError::Parse` (→ 400 → non-200 → Firehose retry).
pub fn decode_firehose_envelope(
    body: &[u8],
    signal: Signal,
) -> Result<FirehoseEnvelope, OtelError> {
    let parsed: FirehoseEnvelopeJson =
        serde_json::from_slice(body).map_err(|e| OtelError::Parse {
            signal,
            message: format!("firehose envelope json: {e}"),
        })?;
    let mut records = Vec::with_capacity(parsed.records.len());
    for (i, rec) in parsed.records.iter().enumerate() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(rec.data.as_bytes())
            .map_err(|e| OtelError::Parse {
                signal,
                message: format!("firehose record[{i}] base64: {e}"),
            })?;
        records.push(bytes);
    }
    Ok(FirehoseEnvelope {
        request_id: parsed.request_id.unwrap_or_default(),
        records,
    })
}

/// Feed each Firehose record into the metrics decode/split/write path. Per AWS's
/// CloudWatch Metric Streams OpenTelemetry 1.0.0 output format, a record is **not** a
/// single unframed protobuf message — it packs one-or-more length-delimited
/// `[varint32 length][message bytes]` entries back to back. Each message is decoded via
/// `decode_next_length_delimited`, then passed through
/// `cloudwatch_metrics::rewrite_cloudwatch_metric_streams` — which partitions a matching
/// CloudWatch Metric Streams resource into one synthetic `ResourceMetrics` per namespace
/// (folding the exporter ARN into `service.instance.id`), and passes any non-matching
/// resource through unchanged — before being immediately split + written via
/// `ingest_parsed_metrics`. Each message is written before the next message's length prefix
/// is even read, so a malformed message later in a record can't retroactively discard
/// already-decoded, already-written messages that precede it in the same record.
/// Content-addressed `block_id` and idempotent writes are otherwise inherited unchanged from
/// the shared split/write path.
///
/// Decode/ingest errors are tagged with `firehose record[{i}] message[{j}]: ...` (`i` the
/// record index, `j` the ordinal of the message within that record) so a failure can be
/// localized to the exact record and message, matching `decode_firehose_envelope`'s
/// existing `firehose record[{i}]` error-tagging style.
pub async fn ingest_firehose_metrics(
    service: Arc<WebIngestionService>,
    records: Vec<Vec<u8>>,
    audience: &WriteAudience,
) -> Result<(), OtelError> {
    for (i, rec) in records.into_iter().enumerate() {
        let mut buf: &[u8] = &rec;
        let mut j = 0usize;
        while let Some(req) =
            decode_next_length_delimited::<ExportMetricsServiceRequest>(&mut buf, Signal::Metrics)
                .map_err(|e| e.with_context(format!("firehose record[{i}] message[{j}]")))?
        {
            let req = rewrite_cloudwatch_metric_streams(req);
            ingest_parsed_metrics(&service, req, audience)
                .await
                .map_err(|e| e.with_context(format!("firehose record[{i}] message[{j}]")))?;
            j += 1;
        }
    }
    Ok(())
}
