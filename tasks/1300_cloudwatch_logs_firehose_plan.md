# CloudWatch Logs Ingestion via Firehose HTTP Endpoint Delivery Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1300
**Depends on**: #1299 (closed/shipped — see `tasks/completed/1299_firehose_otlp_metrics_ingestion_plan.md`)

## Overview

Add a decoder for the **CloudWatch Logs subscription-filter delivery format** so that logs
from AWS services (RDS Postgres logs, ECS `awslogs`, Lambda, etc.) can reach micromegas as
**CloudWatch Logs → subscription filter → Firehose → micromegas**, with no intermediate
consumer. #1299 already taught the ingestion service the generic Firehose HTTP Endpoint
Delivery envelope (JSON wrapper, base64 records, `X-Amz-Firehose-Access-Key` auth, ack shape)
for the metrics case. This plan reuses that envelope machinery for a second, logs-shaped
route and adds the one genuinely new piece: a decoder for CloudWatch's own (non-OTLP,
non-negotiable) per-record payload format.

The design goal is the same one #1299 established: once a record's bytes are unwrapped, feed
them into the **existing OTLP logs pipeline unchanged**. Concretely, each CloudWatch Logs
Firehose record is turned into a synthetic `ExportLogsServiceRequest` (one `ResourceLogs`,
`logGroup`/`logStream`/`owner` as resource attributes, one `LogRecord` per `logEvent`) and
handed to `split_logs` + the existing block writer — the same trick `otel-ingestion`'s webhook
path (`handler::ingest_webhook`) already uses to turn an arbitrary HTTP body into a synthetic
OTLP logs request. **No new stream format, no new `BlockProcessor`, no analytics/read-side
changes at all** — `OtelLogsBlockProcessor` and the `log_entries` view handle these blocks
exactly like any other OTLP logs producer.

## Current State

### The Firehose envelope + auth machinery to reuse (#1299)

- `rust/public/src/servers/firehose.rs` — `firehose_router()` (170) registers
  `POST /ingestion/otlp/v1/metrics/firehose`, layered with `firehose_auth_middleware` (86,
  reads `X-Amz-Firehose-Access-Key`, synthesizes an `Authorization: Bearer` header, reuses the
  shared `AuthProvider`) and `apply_ingestion_body_limits` (handles optional
  `Content-Encoding: gzip` on the *envelope* + size caps). `firehose_response()` (51) builds
  the `{requestId, timestamp, errorMessage?}` ack shape; `request_id_from()` (73) reads
  `X-Amz-Firehose-Request-Id`. All of this is signal-agnostic — it only knows about the
  Firehose transport, not what's inside a record.
- `rust/otel-ingestion/src/handler.rs:295` `decode_firehose_envelope(body: &[u8]) ->
  Result<FirehoseEnvelope, OtelError>` — parses `{requestId, records:[{data}]}` JSON,
  base64-decodes each `data` field. Currently hardcodes `Signal::Metrics` in its error
  messages (298, 306) since it only has one caller today.
- `rust/otel-ingestion/src/handler.rs:320` `ingest_firehose_metrics()` loops
  `envelope.records` and feeds each into `ingest_metrics(..., Encoding::Protobuf)` — the
  per-record loop pattern this plan mirrors for logs.
- `rust/public/src/servers/ingestion.rs:131-158` `serve_ingestion` merges `firehose_router`'s
  output into the top-level `app` **outside** `protected_app` (so it never hits the global
  Bearer `auth_middleware` — Firehose can't send `Authorization: Bearer`), alongside
  `health_router` and `protected_app`.

### The webhook precedent for "synthesize an OTLP logs request from a non-OTLP body" (#1298)

- `rust/otel-ingestion/src/handler.rs:200` `build_webhook_request(resource_attrs, target,
  body)` builds a synthetic `ExportLogsServiceRequest`: one `Resource` (caller-supplied
  attrs), one `ScopeLogs` (scope name = caller-supplied `target`), one `LogRecord` (body =
  caller-supplied bytes, `time_unix_nano`/`observed_time_unix_nano` left at 0 so `split_logs`
  backfills). `ingest_webhook()` (255) then calls `split_logs_with_extra_hash_input` +
  `write_blocks(&service, Signal::Logs, blocks)` directly — **it does not go through
  `ingest_logs`**, since it already has a parsed request, not raw bytes. This is exactly the
  shape the CloudWatch Logs path needs, except CloudWatch Logs supplies real timestamps
  per-record (no backfill needed) and multiple resource attrs + multiple log records per
  Firehose record (not just one).

### Logs split/write path being reused as-is

- `rust/otel-ingestion/src/block.rs:262` `split_logs()` / `split_logs_with_extra_hash_input()`
  — walks `ExportLogsServiceRequest.resource_logs`, skips resources with zero records (280-284,
  the "fast-path" — meaning a CloudWatch record with an empty `logEvents` needs **no special
  handling**, it's already a no-op here), computes bounds via `logs_bounds()` (41), derives a
  content-addressed `block_id` from the pre-mutation proto bytes (290/296).
- `rust/otel-ingestion/src/handler.rs:71` `write_blocks()` — registers process + stream
  (idempotent) then writes each block. **Currently private** (`async fn`, no `pub`) since its
  only callers today live in the same file.
- `rust/otel-ingestion/src/identity.rs:184` `process_id_from_resource()` hashes an ordered
  tuple of resource attributes (`host.id`, `host.name`, `process.pid`,
  `service.instance.id`/`service.name`/`service.namespace`, etc.) into a `process_id` via
  UUIDv5. `is_degenerate_resource()` (158) warns (at `debug!`) when none of the four primary
  identifying fields are set — CloudWatch Logs records must populate at least one of these to
  avoid collapsing every log stream onto one process.
- `rust/analytics/src/lakehouse/otel/logs_block_processor.rs` `OtelLogsBlockProcessor` decodes
  the stored `ResourceLogs` proto into `log_entries` rows: `target` ← scope name, `msg` ← body
  (`any_value_to_string`), `level` ← `severity_number_to_level()` (`attrs.rs:158`, unspecified/
  out-of-range → Info), row `properties` ← record attributes + scope extras, and — critically —
  **`process_properties` ← every resource attribute, prefixed `otel.resource.*`**
  (`block.rs:467-475` in `ProcessFromResource::build`). This means resource-level attributes
  (where `logGroup`/`logStream`/`owner` will live) are already surfaced per-row with zero new
  analytics code.
- `rust/analytics/src/lakehouse/log_view.rs:41` `log_processors()` maps `FORMAT_TRANSIT` →
  `LogBlockProcessor`, `FORMAT_OTLP_LOGS` → `OtelLogsBlockProcessor`. Since the CloudWatch Logs
  path stores a real `ResourceLogs` proto under `FORMAT_OTLP_LOGS`/`TAG_LOGS` (`lib.rs:24,33`),
  **this map needs no new entry.**

### `OtelError` (the thiserror example — `Signal` needs a third-ish use, no new variant)

- `rust/otel-ingestion/src/error.rs:16` `Signal { Logs, Metrics, Traces }` and `OtelError {
  Parse, Database, Storage }` (each carrying `signal`). CloudWatch-specific decode failures
  (bad gzip, bad JSON) are just another `OtelError::Parse { signal: Signal::Logs, .. }` — no
  new error variant needed, matching the "don't convert the anyhow majority" guidance in
  AI_GUIDELINES.md (there's no new *kind* of error a caller branches on here).

## Design

### Payload format (per issue)

Each Firehose record's `data`, after base64-decode (handled by `decode_firehose_envelope`), is
**gzip-compressed** (independent of any outer `Content-Encoding: gzip` on the whole HTTP
body — CloudWatch always gzips the record; Firehose's own transport gzip is a separate,
optional layer already handled by `apply_ingestion_body_limits`). Decompressed, it is:

```json
{
  "messageType": "DATA_MESSAGE",
  "owner": "123456789012",
  "logGroup": "/ecs/my-service",
  "logStream": "ecs/my-service/abcd1234",
  "subscriptionFilters": ["my-filter"],
  "logEvents": [
    { "id": "...", "timestamp": 1510109208016, "message": "raw log line" }
  ]
}
```

`CONTROL_MESSAGE` records carry no `logEvents` and must be recognized and dropped silently
(not an error) — CloudWatch sends these periodically to verify reachability.

### New module: `rust/otel-ingestion/src/cloudwatch_logs.rs`

Pure, unit-testable, HTTP-framework-free — same split as the webhook/Firehose-metrics code.

```rust
use crate::block::split_logs;
use crate::error::{OtelError, Signal};
use crate::handler::write_blocks; // becomes `pub(crate)` (see below)
use crate::proto::{AnyValue, ExportLogsServiceRequest, KeyValue, LogRecord, Resource,
    ResourceLogs, ScopeLogs, any_value};
use flate2::read::GzDecoder;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use std::io::Read;
use std::sync::Arc;

#[derive(serde::Deserialize)]
struct CloudWatchLogEventJson {
    id: String,
    timestamp: i64, // epoch millis
    message: String,
}

#[derive(serde::Deserialize)]
struct CloudWatchLogsMessageJson {
    #[serde(rename = "messageType")]
    message_type: String,
    owner: String,
    #[serde(rename = "logGroup")]
    log_group: String,
    #[serde(rename = "logStream")]
    log_stream: String,
    #[serde(default)]
    #[serde(rename = "logEvents")]
    log_events: Vec<CloudWatchLogEventJson>,
}

/// Gunzips one Firehose record's bytes and parses the CloudWatch Logs subscription-filter
/// JSON. Returns `Ok(None)` for `CONTROL_MESSAGE` records (drop, not an error) or a
/// `DATA_MESSAGE` with no events. Malformed gzip/JSON → `OtelError::Parse` (→ 400 →
/// Firehose retry, matching `decode_firehose_envelope`'s contract).
fn decode_cloudwatch_logs_record(
    raw: &[u8],
    index: usize,
) -> Result<Option<CloudWatchLogsMessageJson>, OtelError> {
    let mut decompressed = Vec::new();
    GzDecoder::new(raw)
        .read_to_end(&mut decompressed)
        .map_err(|e| OtelError::Parse {
            signal: Signal::Logs,
            message: format!("cloudwatch logs record[{index}] gunzip: {e}"),
        })?;
    let msg: CloudWatchLogsMessageJson = serde_json::from_slice(&decompressed)
        .map_err(|e| OtelError::Parse {
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
fn build_export_logs_request(msg: &CloudWatchLogsMessageJson) -> ExportLogsServiceRequest {
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
```

Notes:
- `write_blocks` moves from private to `pub(crate)` in `handler.rs:71` — the only change
  needed to that function; no logic change.
- No explicit "skip empty `logEvents`" branch is strictly required in `ingest_...` itself
  (`split_logs`'s own fast-path already turns a zero-record `ResourceLogs` into a no-op), but
  checking in `decode_cloudwatch_logs_record` avoids building a `Resource`/proto for nothing
  and gives `CONTROL_MESSAGE` and "empty data message" the same short-circuit.
- `severity_number: 0` (`UNSPECIFIED`) — `severity_number_to_level()` already maps this (and
  any out-of-range value) to Info (`attrs.rs:166`), matching "CloudWatch does not parse the
  message, store as-is; structured parsing is out of scope."
- `scope: None` — there is no CloudWatch equivalent of an OTel instrumentation scope/logger
  name; `target` in `log_entries` will be empty for these rows. `logGroup`/`logStream` (the
  natural filtering dimensions) are queryable via `process_properties` instead (see below).
- `aws.log.event.id` is attached as a **record**-level attribute (lands in the row
  `properties` column via `attrs_to_jsonb`, `logs_block_processor.rs:179`) since it's
  per-event, whereas `logGroup`/`logStream`/`owner` are resource-level (one value per Firehose
  record) and land in `process_properties` automatically via `ProcessFromResource::build`
  (`block.rs:467-475`, prefixed `otel.resource.*`) — satisfies the acceptance criterion
  "logGroup/logStream/owner attached as attributes" with zero new analytics code.
- `time_unix_nano` overflow: `ev.timestamp` (millis) is `i64` in the JSON but always
  non-negative in practice (CloudWatch epoch-ms); `as u64` before the `*1_000_000` multiply
  avoids the twos-complement wraparound a raw negative-looking cast could hit, and
  `saturating_mul` caps at `u64::MAX` instead of silently wrapping for any pathological input
  rather than panicking or producing a bogus small timestamp.

### `decode_firehose_envelope` gets a `Signal` parameter

Its only caller today (`ingest_firehose_metrics`) hardcodes `Signal::Metrics` in error
messages (`handler.rs:298,306`). Once a logs-shaped caller exists, a malformed CloudWatch Logs
envelope would otherwise log as `"OTLP parse error (metrics): ..."`, which is wrong and
confusing to debug. Parameterize:

```rust
pub fn decode_firehose_envelope(body: &[u8], signal: Signal) -> Result<FirehoseEnvelope, OtelError> {
    // ...same body, `Signal::Metrics` literals replaced with `signal`...
}
```

Both call sites (`firehose.rs`'s metrics handler passes `Signal::Metrics`; the new CloudWatch
Logs handler passes `Signal::Logs`) and all seven existing unit tests in
`otel-ingestion/tests/firehose_tests.rs` (which call it positionally) need the new argument —
a one-line change per call site.

### Shared Firehose route plumbing: extract to `firehose_common.rs`

`firehose_auth_middleware`, `firehose_response`, `request_id_from`, and the header name
constants in `rust/public/src/servers/firehose.rs` are entirely signal-agnostic already — they
only know about the Firehose transport (access-key header, ack shape), not what's inside a
record. Rather than duplicating them into a second route file, extract the shared pieces into
`rust/public/src/servers/firehose_common.rs`:

- `HEADER_ACCESS_KEY`, `HEADER_REQUEST_ID`
- `FirehoseResponseBody`, `firehose_response()`
- `request_id_from()`
- `firehose_auth_middleware()`

`firehose.rs` keeps only the metrics-specific handler/router and imports the shared pieces
from `firehose_common`. This is a mechanical extraction (move + `pub(crate)`), not a behavior
change — existing tests in `public/tests/firehose_tests.rs` are unaffected since they only
call `firehose_router()`.

### New route module: `rust/public/src/servers/firehose_cloudwatch_logs.rs`

Mirrors `firehose.rs`'s shape exactly, swapping the metrics decode/ingest calls for the
CloudWatch Logs ones:

```rust
use super::firehose_common::{firehose_auth_middleware, firehose_response, request_id_from};
use super::ingestion_limits::apply_ingestion_body_limits;
use axum::{Extension, Router, extract::Request, http::{HeaderMap, StatusCode}, middleware,
    response::Response, routing::post};
use micromegas_auth::types::AuthProvider;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_otel_ingestion::{Signal, handler, cloudwatch_logs};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

async fn cloudwatch_logs_firehose_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let mut request_id = request_id_from(&headers);
    let envelope = match handler::decode_firehose_envelope(&body, Signal::Logs) {
        Ok(e) => e,
        Err(err) => {
            error!("cloudwatch logs firehose decode error: {err}");
            let status = StatusCode::from_u16(err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return firehose_response(status, &request_id, Some(&err.public_message()));
        }
    };
    if request_id.is_empty() {
        request_id = envelope.request_id.clone();
    }
    match cloudwatch_logs::ingest_cloudwatch_logs_firehose(service, envelope.records).await {
        Ok(()) => firehose_response(StatusCode::OK, &request_id, None),
        Err(err) => {
            error!("cloudwatch logs firehose ingest error: {err}");
            let status = StatusCode::from_u16(err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            firehose_response(status, &request_id, Some(&err.public_message()))
        }
    }
}

pub fn firehose_router(
    service: Arc<WebIngestionService>,
    auth_provider: Option<Arc<dyn AuthProvider>>,
) -> Router {
    let mut router = Router::new()
        .route(
            "/ingestion/cloudwatch/v1/logs/firehose",
            post(cloudwatch_logs_firehose_handler),
        )
        .layer(Extension(service));
    if let Some(provider) = auth_provider {
        router = router.layer(middleware::from_fn(move |req, next| {
            firehose_auth_middleware(provider.clone(), req, next)
        }));
    }
    apply_ingestion_body_limits(router)
}
```

**Route naming**: `/ingestion/cloudwatch/v1/logs/firehose`, not
`/ingestion/otlp/v1/logs/firehose`. Unlike the metrics case (where the record genuinely *is*
an OTLP protobuf and Metric Streams can emit alternative formats), CloudWatch Logs
subscription-filter delivery has exactly one proprietary format — there's no OTLP framing on
the wire, only in how micromegas happens to store the result. Naming the route `otlp/...`
would misleadingly imply the client sends OTLP.

**Per-delivery-stream format selection**: the issue notes "selectable per delivery stream (a
stream carries a single record format)" — this plan satisfies that by giving CloudWatch Logs
its own route path (`/ingestion/cloudwatch/v1/logs/firehose`), distinct from the metrics route;
an operator points a given Firehose delivery stream's HTTP endpoint at whichever route matches
what it carries. No dynamic content-sniffing needed.

**`subscriptionFilters`**: not surfaced as an attribute in v1 — it's an array (usually
single-element) naming the CloudWatch subscription filter, not the log content itself.
Revisit if a use case needs per-filter routing/tagging; trivial to add as another resource
attribute (`aws.log.subscription_filter`) later.

**Auth providers**: same constraint as the metrics Firehose route — effectively API-key only
(`MICROMEGAS_API_KEYS`), since Firehose cannot produce an OIDC JWT.

### Wiring into `serve_ingestion` (`ingestion.rs`)

Same pattern as the metrics Firehose route — a third sub-router merged into `app` outside
`protected_app`. `auth_provider` must be cloned a second time *before* it is moved into the
existing `if let Some(provider) = auth_provider` block (alongside the existing `firehose_auth`
clone), and the existing `firehose::firehose_router(service, firehose_auth)` call must switch to
`service.clone()` since `service` also needs to survive for the new router:

```rust
let firehose_auth = auth_provider.clone();
let cw_logs_firehose_auth = auth_provider.clone(); // new: cloned before `auth_provider` is moved below
// ...
if let Some(provider) = auth_provider {
    // ...unchanged...
}

// existing call changes `service` -> `service.clone()` so `service` remains available below
let firehose_app = super::firehose::firehose_router(service.clone(), firehose_auth);
let cw_logs_firehose_app = super::firehose_cloudwatch_logs::firehose_router(
    service.clone(),
    cw_logs_firehose_auth,
);

let app = health_router
    .merge(protected_app)
    .merge(firehose_app)
    .merge(cw_logs_firehose_app)
    .layer(middleware::from_fn(observability_middleware));
```

Add `pub mod firehose_common;` and `pub mod firehose_cloudwatch_logs;` to
`rust/public/src/servers/mod.rs`. Add `pub mod cloudwatch_logs;` to
`rust/otel-ingestion/src/lib.rs`.

### Why no new format constant / `BlockProcessor`

The tempting alternative — a `FORMAT_CLOUDWATCH_LOGS` constant and a dedicated
`CloudWatchLogsBlockProcessor` that decodes the raw CloudWatch JSON (or a custom binary
encoding) directly at read time — was rejected. It would duplicate everything
`OtelLogsBlockProcessor` already does (severity mapping, property JSONB encoding, dictionary
building) for no benefit: the write side already has a complete, tested proto (`ResourceLogs`)
to represent "one resource, N timestamped text records with attributes." Storing that proto
under `FORMAT_OTLP_LOGS` (the format string just describes *how the block payload is
encoded*, not *who produced it*) means the CloudWatch Logs path needs **zero analytics-crate
changes** — the same reasoning #1299 used to justify reusing `ingest_metrics` verbatim.

## Implementation Steps

1. **`write_blocks` visibility** — change `async fn write_blocks` to `pub(crate) async fn
   write_blocks` in `rust/otel-ingestion/src/handler.rs:71` (no logic change).
2. **`decode_firehose_envelope` signal parameter** — add `signal: Signal` param
   (`handler.rs:295`), replace the two hardcoded `Signal::Metrics` literals (298, 306); update
   its one production call site (`firehose_handler` in `rust/public/src/servers/firehose.rs`,
   passes `Signal::Metrics`) and all seven call sites in
   `rust/otel-ingestion/tests/firehose_tests.rs`.
3. **CloudWatch Logs decoder module** — add `rust/otel-ingestion/src/cloudwatch_logs.rs`
   (`decode_cloudwatch_logs_record`, `build_export_logs_request`,
   `ingest_cloudwatch_logs_firehose`); add `pub mod cloudwatch_logs;` to
   `rust/otel-ingestion/src/lib.rs`.
4. **`flate2` dependency** — add `flate2.workspace = true` to
   `rust/otel-ingestion/Cargo.toml` `[dependencies]` (alphabetically between `chrono` and
   `opentelemetry-proto`); also add to `[dev-dependencies]` if not already covered for building
   gzip test fixtures (it currently has none — add a `[dev-dependencies]` section).
5. **Unit tests (otel-ingestion)** — add `rust/otel-ingestion/tests/cloudwatch_logs_tests.rs`
   (see Testing).
6. **Extract shared Firehose plumbing** — move `firehose_auth_middleware`,
   `firehose_response`, `request_id_from`, and the header constants out of
   `rust/public/src/servers/firehose.rs` into new `rust/public/src/servers/firehose_common.rs`;
   update `firehose.rs` to import from it. Add `pub mod firehose_common;` to `servers/mod.rs`.
7. **New CloudWatch Logs route** — add `rust/public/src/servers/firehose_cloudwatch_logs.rs`
   (`firehose_router` + handler, per Design). Add `pub mod firehose_cloudwatch_logs;` to
   `servers/mod.rs`.
8. **Wire into server** — clone `auth_provider` again, merge
   `firehose_cloudwatch_logs::firehose_router(...)` into `app` in `serve_ingestion`
   (`ingestion.rs`).
9. **HTTP tests (public)** — add `rust/public/tests/firehose_cloudwatch_logs_tests.rs` (see
   Testing); register a `[[test]]` entry (`required-features = ["server"]`) in
   `rust/public/Cargo.toml`.
10. **Python e2e** — add a CloudWatch-Logs-Firehose case (see Testing), likely a new
    `python/micromegas/tests/test_cloudwatch_logs_firehose_e2e.py` alongside the existing OTLP
    e2e tests. #1299 instead extended the shared `test_otlp_e2e.py`; a separate file is
    preferred here because this route is not OTLP-shaped (no `Signal`/OTLP envelope reuse in
    the test helpers) and keeping it isolated avoids growing an already-multi-signal file with
    an unrelated payload format.
11. **Docs** — extend `mkdocs/docs/otlp/index.md` with a "CloudWatch Logs (Kinesis Firehose)"
    section and a new row in the routes table.
12. **CI** — `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`, then
    `python3 build/rust_ci.py`.

## Files to Modify / Create

- `rust/otel-ingestion/src/handler.rs` — `write_blocks` → `pub(crate)`;
  `decode_firehose_envelope` gains a `signal: Signal` parameter.
- `rust/otel-ingestion/src/cloudwatch_logs.rs` — **new**: decode/build/ingest for CloudWatch
  Logs Firehose records.
- `rust/otel-ingestion/src/lib.rs` — `pub mod cloudwatch_logs;`.
- `rust/otel-ingestion/Cargo.toml` — add `flate2` to `[dependencies]` (+ `[dev-dependencies]`).
- `rust/otel-ingestion/tests/firehose_tests.rs` — update 7 call sites for the new
  `decode_firehose_envelope` signature.
- `rust/otel-ingestion/tests/cloudwatch_logs_tests.rs` — **new** unit tests.
- `rust/public/src/servers/firehose_common.rs` — **new**: shared auth/response/request-id
  helpers extracted from `firehose.rs`.
- `rust/public/src/servers/firehose.rs` — trimmed to metrics-specific handler/router; imports
  from `firehose_common`; its one call to `decode_firehose_envelope` passes `Signal::Metrics`.
- `rust/public/src/servers/firehose_cloudwatch_logs.rs` — **new**: CloudWatch Logs route.
- `rust/public/src/servers/mod.rs` — `pub mod firehose_common;`,
  `pub mod firehose_cloudwatch_logs;`.
- `rust/public/src/servers/ingestion.rs` — clone `auth_provider` again, merge the new router.
- `rust/public/Cargo.toml` — `[[test]]` entry for `firehose_cloudwatch_logs_tests`.
- `rust/public/tests/firehose_cloudwatch_logs_tests.rs` — **new** HTTP-level tests.
- `python/micromegas/tests/test_cloudwatch_logs_firehose_e2e.py` — **new** end-to-end test
  (kept separate from `test_otlp_e2e.py` since this route carries no OTLP payload).
- `mkdocs/docs/otlp/index.md` — new section + routes-table row.

## Trade-offs

- **Synthesize an `ExportLogsServiceRequest` vs. a bespoke CloudWatch block/decoder pair.**
  Reuses `split_logs`, content-addressed `block_id`/dedup, `write_blocks`,
  `OtelLogsBlockProcessor`, and every existing `log_entries` column unchanged, at the cost of
  building an in-memory proto per Firehose record instead of a purpose-built structure. Given
  the existing webhook precedent already validated this pattern for exactly this kind of
  "foreign body → synthetic OTLP logs request", a dedicated format would be pure duplication
  for a payload that maps onto `LogRecord` almost one-to-one anyway.
- **`service.name`/`service.instance.id` = `logGroup`/`logStream` for identity.** Piggybacks on
  the existing, well-tested `process_id_from_resource` hash instead of inventing a
  CloudWatch-specific identity scheme. Trade-off: a log group with a single, unstable
  logStream name (unlikely in practice — AWS log stream names are stable per task/instance
  lifetime) would spawn a new "process" each time the stream name changes; accepted as the
  same class of trade-off `is_degenerate_resource` already flags for other producers.
  Separately, `cloud.account.id` (= `owner`) is set as a resource attribute but is **not**
  part of the `process_id_from_resource` hash formula, so two different AWS accounts with the
  same `logGroup`+`logStream` names collapse onto the same `process_id`. This is plausible for
  RDS Postgres logs specifically, since RDS log stream names are user-chosen DB-instance
  identifiers (e.g. `my-prod-db`) rather than random — a shared naming convention across
  accounts/environments can collide. Accepted for v1: `owner` is still queryable per-row via
  `process_properties.otel.resource.cloud.account.id`, so rows aren't ambiguous, only the
  `process_id` grouping is coarser than ideal across accounts; revisit by folding
  `cloud.account.id` into the identity hash if this proves to matter in practice.
- **`aws.log.event.id` as a record attribute vs. dropping it.** CloudWatch's per-event `id` is
  small, stable, and lets an operator correlate a `log_entries` row back to the exact
  CloudWatch event if needed (e.g. cross-referencing against the CloudWatch console). Kept at
  negligible storage cost (JSONB dedup on the properties dictionary).
- **Route path `/ingestion/cloudwatch/v1/logs/firehose` vs. reusing
  `/ingestion/otlp/v1/logs/firehose`.** A CloudWatch-branded path is more honest about what
  the client actually speaks (there is no OTLP on the wire here) and leaves room for a
  hypothetical future OTLP-format logs-over-Firehose route without a naming collision.
- **Extracting `firehose_common.rs` now vs. leaving `firehose.rs` copy-pasted per signal.**
  Two Firehose routes sharing 100%-identical auth/response code is exactly the DRY threshold
  worth crossing; a third future Firehose route (e.g. CloudWatch Logs in a different output
  mode) would otherwise triple the duplication.
- **No new `OtelError` variant for gunzip/JSON failures.** Both are just malformed input from
  the caller's perspective — `Parse { signal: Signal::Logs, .. }` already carries a
  free-form message and maps to 400/non-retryable, matching AI_GUIDELINES's "thiserror only
  when a caller branches on the kind" — nothing here branches differently for "bad gzip" vs.
  "bad JSON."

## Documentation

`mkdocs/docs/otlp/index.md`:
- Add a row to the routes table: `POST /ingestion/cloudwatch/v1/logs/firehose` |
  CloudWatch Logs subscription-filter record per Firehose record | `log_entries` (see
  CloudWatch Logs (Kinesis Firehose)). The existing table's middle column is headed "OTLP
  message", which doesn't literally describe this row's payload (there is no OTLP framing on
  the wire — see Route naming above); reword the header to something signal-agnostic (e.g.
  "Payload") or add a note by the new row clarifying it isn't OTLP-framed.
- New `## CloudWatch Logs (Kinesis Firehose)` section (sibling to the existing `## CloudWatch
  Metric Streams (Kinesis Firehose)` section), covering: the pipeline (CloudWatch Logs →
  subscription filter → Firehose → micromegas, no Lambda/collector), the payload format and
  `CONTROL_MESSAGE` handling, delivery-stream setup (HTTP endpoint URL, access key = a
  micromegas API key, gzip note — record-level gzip is mandatory and handled regardless of the
  delivery stream's own `Content-Encoding` setting), how `logGroup`/`logStream`/`owner` surface
  (`process_properties.otel.resource.*` — same discovery path as any other OTel resource
  attribute), and the same ack/idempotency/TLS notes as the metrics section.

## Testing Strategy

- **Unit (otel-ingestion, `tests/cloudwatch_logs_tests.rs`, no DB):**
  - `decode_cloudwatch_logs_record` on a gzip+JSON `DATA_MESSAGE` with multiple `logEvents` →
    `Some`, correct `logGroup`/`logStream`/`owner`, event count.
  - `CONTROL_MESSAGE` (gzip+JSON, no `logEvents`) → `None`, not an error.
  - `DATA_MESSAGE` with empty `logEvents` → `None`.
  - Malformed gzip → `OtelError::Parse`.
  - Valid gzip, malformed JSON → `OtelError::Parse`.
  - `build_export_logs_request` → resource attrs contain `service.name`=logGroup,
    `service.instance.id`=logStream, `cloud.account.id`=owner,
    `aws.log.group.name`/`aws.log.stream.name`; one `LogRecord` per `logEvent`;
    `time_unix_nano` = `timestamp * 1_000_000` (ms → ns); body = raw `message` verbatim
    (including a JSON-shaped message string, to prove no structured parsing happens).
  - Full pipeline: decode → build request → `split_logs` → one `PreparedBlock`,
    `nb_records` == `logEvents.len()`.
  - Multi-record batch: two gzip `DATA_MESSAGE` records with distinct `logStream`s →
    `split_logs` on each yields distinct `process_id`s (via `process_id_from_resource`).
- **HTTP (public, `tests/firehose_cloudwatch_logs_tests.rs`, `tower::ServiceExt::oneshot`, no
  live DB):** same lazy-pool + `InMemory` + keyring pattern as `firehose_tests.rs`.
  - Missing / wrong access key → non-200 Firehose error shape (auth fails before decode, no
    DB call).
  - Valid key + envelope containing **one gzip `CONTROL_MESSAGE` record** → 200 ack, no
    `errorMessage` — exercises real gunzip + JSON decode + control-message skip end-to-end
    with **no DB write** (since control messages never reach `write_blocks`).
  - Valid key + gzip-enveloped, empty `records: []` (mirrors the existing metrics test) → 200
    ack.
  - Dev mode (`auth_provider: None`) → open access.
  - `#[ignore]`d `full_data_message_ingest_succeeds_against_a_live_stack`: real `DATA_MESSAGE`
    with several `logEvents` → 200 ack; requires `MICROMEGAS_SQL_CONNECTION_STRING` +
    object-store env vars, matching the metrics test's convention.
- **Integration (Python e2e, `test_cloudwatch_logs_firehose_e2e.py`):** build a gzip+base64
  CloudWatch Logs `DATA_MESSAGE` with a per-run-unique `logStream` (so `discover_process_id`
  or an equivalent lookup finds it), wrap in a Firehose envelope, POST to
  `/ingestion/cloudwatch/v1/logs/firehose` with only `X-Amz-Firehose-Request-Id` (no
  `X-Amz-Firehose-Access-Key`, matching the #1299 metrics e2e pattern against the
  `--disable-auth` dev-mode stack), assert the 200 ack, then `assert_eventually` a
  `log_entries` row appears with the expected `msg`/`time` and `process_properties` containing
  `otel.resource.aws.log.group.name`/`otel.resource.aws.log.stream.name`. Add a
  `CONTROL_MESSAGE`-is-ignored assertion (no new row). Key rejection (missing/wrong
  `X-Amz-Firehose-Access-Key`) is covered by the Rust HTTP tests
  (`firehose_cloudwatch_logs_tests.rs`), not the e2e — the e2e stack runs with
  `--disable-auth`, so the access key is ignored and a rejected-key assertion would not be
  able to fail.
- **CI:** `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
  `python3 build/rust_ci.py`.
