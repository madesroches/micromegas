//! Decoder for the CloudWatch Logs subscription-filter delivery format, layered on top of
//! the generic Firehose HTTP Endpoint Delivery envelope (`handler::decode_firehose_envelope`).
//!
//! Each Firehose record here is CloudWatch's own proprietary, gzip-compressed JSON —
//! not OTLP. Once decoded, it is turned into a synthetic `ExportLogsServiceRequest` and fed
//! into the existing OTLP logs split/write path unchanged (mirrors the webhook precedent in
//! `handler::build_webhook_request` / `handler::ingest_webhook`).

use crate::block::split_logs;
use crate::error::{OtelError, Signal};
use crate::handler::write_blocks;
use crate::proto::{
    AnyValue, ExportLogsServiceRequest, KeyValue, LogRecord, Resource, ResourceLogs, ScopeLogs,
    any_value,
};
use flate2::read::GzDecoder;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use std::io::Read;
use std::sync::Arc;

/// Public (rather than private) so `tests/cloudwatch_logs_tests.rs` can assert decoded
/// shapes directly, matching the `build_webhook_request` precedent in `handler.rs`.
#[derive(Debug, serde::Deserialize)]
pub struct CloudWatchLogEventJson {
    pub id: String,
    pub timestamp: i64, // epoch millis
    pub message: String,
}

/// Public (rather than private) so `tests/cloudwatch_logs_tests.rs` can assert decoded
/// shapes directly, matching the `build_webhook_request` precedent in `handler.rs`.
#[derive(Debug, serde::Deserialize)]
pub struct CloudWatchLogsMessageJson {
    #[serde(rename = "messageType")]
    pub message_type: String,
    pub owner: String,
    #[serde(rename = "logGroup")]
    pub log_group: String,
    #[serde(rename = "logStream")]
    pub log_stream: String,
    #[serde(default)]
    #[serde(rename = "logEvents")]
    pub log_events: Vec<CloudWatchLogEventJson>,
}

/// Upper bound on one Firehose record's decompressed size. CloudWatch Logs subscription-
/// filter Firehose records are small in practice (AWS caps the compressed payload well
/// under this), but the outer Firehose HTTP body limit (`apply_ingestion_body_limits`,
/// 300 MiB decompressed) guards a completely different, independent gzip layer — the
/// outer envelope, not this per-record gzip nested inside one base64 `data` field. Without
/// its own cap, `read_to_end` would happily decompress a crafted record at a plain-DEFLATE
/// ratio into tens of GB before any check fires (a decompression-bomb DoS). Generous
/// relative to real CloudWatch traffic, but bounds worst-case memory.
const MAX_DECOMPRESSED_RECORD_BYTES: u64 = 64 * 1024 * 1024;

/// Gunzips one Firehose record's bytes and parses the CloudWatch Logs subscription-filter
/// JSON. Returns `Ok(None)` for `CONTROL_MESSAGE` records (drop, not an error) or a
/// `DATA_MESSAGE` with no events. Malformed gzip/JSON, or a decompressed size over
/// `MAX_DECOMPRESSED_RECORD_BYTES` → `OtelError::Parse` (→ 400 → Firehose retry, matching
/// `decode_firehose_envelope`'s contract).
///
/// Public (rather than private) so `tests/cloudwatch_logs_tests.rs` can assert its shape
/// directly, matching the `build_webhook_request` precedent in `handler.rs`.
pub fn decode_cloudwatch_logs_record(
    raw: &[u8],
    index: usize,
) -> Result<Option<CloudWatchLogsMessageJson>, OtelError> {
    let mut decompressed = Vec::new();
    // Read one byte past the cap so exceeding it is distinguishable from landing exactly
    // on it: `take` silently truncates rather than erroring, so the length check below is
    // what actually enforces the bound.
    GzDecoder::new(raw)
        .take(MAX_DECOMPRESSED_RECORD_BYTES + 1)
        .read_to_end(&mut decompressed)
        .map_err(|e| OtelError::Parse {
            signal: Signal::Logs,
            message: format!("cloudwatch logs record[{index}] gunzip: {e}"),
        })?;
    if decompressed.len() as u64 > MAX_DECOMPRESSED_RECORD_BYTES {
        return Err(OtelError::Parse {
            signal: Signal::Logs,
            message: format!(
                "cloudwatch logs record[{index}] gunzip: decompressed size exceeds \
                 {MAX_DECOMPRESSED_RECORD_BYTES} byte cap"
            ),
        });
    }
    let msg: CloudWatchLogsMessageJson =
        serde_json::from_slice(&decompressed).map_err(|e| OtelError::Parse {
            signal: Signal::Logs,
            message: format!("cloudwatch logs record[{index}] json: {e}"),
        })?;
    if msg.message_type == "CONTROL_MESSAGE" || msg.log_events.is_empty() {
        return Ok(None);
    }
    Ok(Some(msg))
}

/// Builds a synthetic `ExportLogsServiceRequest` from one CloudWatch Logs `DATA_MESSAGE`:
/// one `Resource` carrying `logGroup`/`logStream`/`owner` as identifying attributes, one
/// `LogRecord` per `logEvent` (timestamp converted ms → ns, body = raw message, verbatim —
/// CloudWatch does not parse `message`, so neither do we).
///
/// `service.name` = logGroup / `service.instance.id` = logStream so distinct log streams
/// (distinct ECS tasks, Lambda instances, RDS instances) resolve to distinct `process_id`s
/// via the existing `process_id_from_resource` formula — no CloudWatch-specific identity
/// logic needed.
///
/// Public (rather than private) so `tests/cloudwatch_logs_tests.rs` can assert its shape
/// directly, matching the `build_webhook_request` precedent in `handler.rs`.
pub fn build_export_logs_request(msg: &CloudWatchLogsMessageJson) -> ExportLogsServiceRequest {
    fn kv(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            key_strindex: 0,
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue(value.to_string())),
            }),
        }
    }
    let resource_attrs = vec![
        kv("service.name", &msg.log_group),
        kv("service.instance.id", &msg.log_stream),
        kv("cloud.account.id", &msg.owner),
        kv("aws.log.group.name", &msg.log_group),
        kv("aws.log.stream.name", &msg.log_stream),
    ];
    let log_records = msg
        .log_events
        .iter()
        .map(|ev| LogRecord {
            time_unix_nano: (ev.timestamp as u64).saturating_mul(1_000_000),
            observed_time_unix_nano: 0,
            severity_number: 0, // CloudWatch doesn't parse `message` — no severity available.
            severity_text: String::new(),
            body: Some(AnyValue {
                value: Some(any_value::Value::StringValue(ev.message.clone())),
            }),
            attributes: vec![kv("aws.log.event.id", &ev.id)],
            dropped_attributes_count: 0,
            flags: 0,
            trace_id: vec![],
            span_id: vec![],
            event_name: String::new(),
        })
        .collect();
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: resource_attrs,
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

/// Feeds each Firehose record (a gzip-compressed CloudWatch Logs subscription-filter JSON
/// payload) through decode → synthesize → the existing logs split/write path.
/// `CONTROL_MESSAGE`s and empty `DATA_MESSAGE`s are silently skipped, not errors.
pub async fn ingest_cloudwatch_logs_firehose(
    service: Arc<WebIngestionService>,
    records: Vec<Vec<u8>>,
) -> Result<(), OtelError> {
    for (i, rec) in records.iter().enumerate() {
        let Some(msg) = decode_cloudwatch_logs_record(rec, i)? else {
            continue;
        };
        let req = build_export_logs_request(&msg);
        let blocks = split_logs(req).map_err(|e| OtelError::Parse {
            signal: Signal::Logs,
            message: format!("split_logs (cloudwatch): {e}"),
        })?;
        write_blocks(&service, Signal::Logs, blocks).await?;
    }
    Ok(())
}
