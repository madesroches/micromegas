# Kinesis Firehose HTTP Endpoint Delivery for OTLP Metrics Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1299

## Overview

Add an ingestion endpoint that speaks the **Amazon Kinesis Data Firehose HTTP Endpoint
Delivery** protocol so a CloudWatch Metric Stream can push metrics into micromegas as
**Metric Stream → Firehose → micromegas**, with no Lambda, no Kinesis Data Stream, and no
collector process in between. Firehose becomes a dumb managed pipe.

Because Metric Streams can emit records in **OpenTelemetry 1.0.0** output format — where each
delivered record is an OTLP `ExportMetricsServiceRequest` protobuf that our metrics path
already decodes — the only genuinely new code is a thin **Firehose envelope adapter** in front
of the existing decode, plus a small auth shim (Firehose carries its credential in a
non-standard header).

## Goal (one sentence)

Expose `POST /ingestion/otlp/v1/metrics/firehose` that authenticates via
`X-Amz-Firehose-Access-Key`, unwraps the Firehose JSON envelope (gzip-aware, base64 records),
feeds each record into the existing OTLP metrics decode/split/write path, and returns the
Firehose ack response shape.

## Current State

### OTLP metrics ingestion path (the pipeline to reuse)

- `rust/public/src/servers/otlp.rs` — `otlp_router()` registers `POST /ingestion/otlp/v1/metrics`
  (`metrics_handler`, `otlp.rs:156`). The handler negotiates `Content-Type` (proto/JSON) and
  calls `handler::ingest_metrics`.
- `rust/otel-ingestion/src/handler.rs:141` — `ingest_metrics(service, body: Bytes, encoding)`:
  `parse::<ExportMetricsServiceRequest>` → `split_metrics` → `write_blocks`. Returns early with
  a default response when `resource_metrics` is empty (`handler.rs:147`). This is the exact
  function each Firehose record will be fed into with `Encoding::Protobuf`.
- `rust/otel-ingestion/src/block.rs:350` — `split_metrics` computes a **content-addressed**
  `block_id = block_id_from_payload(rm.encode_to_vec())` (UUIDv5 over the resource-metrics
  payload bytes, `identity.rs:237`). The wall-clock `Utc::now()` fallback in
  `build_prepared_block` (`block.rs:187`) only affects the block's time *bounds*, never the
  payload bytes, so `block_id` is a pure function of the metric payload. **Consequence:**
  identical records dedup on write — the property that makes Firehose retries safe (see
  *Idempotency & partial-batch retries*).

### Auth & body limits (the constraint that shapes the design)

- `rust/public/src/servers/ingestion.rs:108` — `serve_ingestion` builds `protected_app` by
  merging `register_routes` + `otlp_router()` + `webhook_router()`, then wraps the **whole**
  thing in the global auth middleware (`ingestion.rs:141`):
  `middleware::from_fn(move |req, next| auth_middleware(provider.clone(), req, next))`.
- `rust/auth/src/axum.rs:41` — `auth_middleware` extracts `HttpRequestParts` and calls
  `AuthProvider::validate_request`. The API-key provider (`auth/src/api_key.rs:93`) reads the
  credential via `RequestParts::bearer_token()` → `Authorization: Bearer <key>`
  (`auth/src/types.rs:45`) and does a constant-time keyring compare.
- **The problem:** Firehose cannot send an `Authorization: Bearer` header. Its only credential
  channel is the `X-Amz-Firehose-Access-Key` header (configured on the delivery stream). A
  Firehose request routed through the existing global middleware would be rejected 401 before
  reaching any handler. So the Firehose route **must not** sit under the global Bearer
  middleware — it needs its own auth step reading the Firehose header.
- `rust/auth/src/types.rs:61` — `HttpRequestParts { headers, method, uri }` is public with
  public fields, so the Firehose auth step can synthesize an `Authorization: Bearer <key>`
  header from `X-Amz-Firehose-Access-Key` and reuse `AuthProvider::validate_request` verbatim
  (constant-time check + `AuthContext`), keeping AWS specifics out of the generic auth crate.
- `rust/public/src/servers/ingestion_limits.rs` — `apply_ingestion_body_limits(router)` applies
  the shared gzip decompression + 20 MiB wire / 300 MiB decompressed limits used by both
  `otlp_router` and `webhook_router`. The Firehose route reuses this (handles
  `Content-Encoding: gzip` for free).
- When `auth_provider` is `None` (dev mode) every ingestion route runs open
  (`ingestion.rs:144`). The Firehose route matches this: auth is applied only when a provider
  is present.

### Webhook precedent (#1298 — the shape to mirror)

`rust/public/src/servers/webhook.rs` + `handler::ingest_webhook`/`build_webhook_request`
(`handler.rs:199,254`) established the pattern: a pure, unit-testable request-shaping helper in
`otel-ingestion`, a thin route in `public` that builds its own (non-OTLP) response, maps
`OtelError` via the public `http_status()`/`is_retryable()`/`public_message()` accessors, and
logs the full error server-side. Tests live in `otel-ingestion/tests/webhook_tests.rs` (pure
shape assertions, no DB). This plan follows the same split.

## Firehose HTTP Endpoint Delivery contract

Fixed and documented by AWS; identical to the OTel Collector `awsfirehosereceiver`.

**Request** (`POST`):
- `Content-Type: application/json`, optional `Content-Encoding: gzip`.
- Headers: `X-Amz-Firehose-Request-Id`, `X-Amz-Firehose-Access-Key` (shared-secret configured
  on the delivery stream), optional `X-Amz-Firehose-Common-Attributes`.
- Body:
  ```json
  { "requestId": "...", "timestamp": 1578090901599,
    "records": [ { "data": "<base64>" }, ... ] }
  ```
  In OpenTelemetry 1.0.0 output mode each record's decoded `data` is an
  `ExportMetricsServiceRequest` protobuf.

**Success response**: HTTP 200, `Content-Type: application/json`, body
`{ "requestId": "<echoed>", "timestamp": <response-time-ms> }`. AWS requires `requestId` to
echo the `X-Amz-Firehose-Request-Id` header.

**Failure response**: any non-200 status with body
`{ "requestId": "<echoed>", "timestamp": <ms>, "errorMessage": "..." }`. Firehose retries on
any non-2xx and eventually spills to its configured S3 backup bucket, so no data is silently
lost. (`Retry-After` is not part of the Firehose contract — retry cadence is Firehose's own;
we do not emit it here.)

## Design

### Principle: unwrap the envelope, then reuse everything

The Firehose adapter is purely an envelope + auth + response-shape concern. Once a record's
bytes are extracted, they are the exact protobuf the existing `ingest_metrics` already handles.
No new identity, block, split, or write logic.

### New logic in `otel-ingestion` (pure, unit-testable)

Add to `rust/otel-ingestion/src/handler.rs` (and add `base64.workspace = true` to
`rust/otel-ingestion/Cargo.toml` `[dependencies]`, alphabetically before `bytes`):

```rust
use base64::Engine as _;

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
pub struct FirehoseEnvelope {
    pub request_id: String,
    pub records: Vec<Vec<u8>>,
}

/// Parse the Firehose JSON envelope and base64-decode every record's `data`.
/// (gzip, if any, is already removed by the shared decompression layer.)
/// Malformed JSON or base64 → `OtelError::Parse` (→ 400 → non-200 → Firehose retry).
pub fn decode_firehose_envelope(body: &[u8]) -> Result<FirehoseEnvelope, OtelError> {
    let parsed: FirehoseEnvelopeJson =
        serde_json::from_slice(body).map_err(|e| OtelError::Parse {
            signal: Signal::Metrics,
            message: format!("firehose envelope json: {e}"),
        })?;
    let mut records = Vec::with_capacity(parsed.records.len());
    for (i, rec) in parsed.records.iter().enumerate() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(rec.data.as_bytes())
            .map_err(|e| OtelError::Parse {
                signal: Signal::Metrics,
                message: format!("firehose record[{i}] base64: {e}"),
            })?;
        records.push(bytes);
    }
    Ok(FirehoseEnvelope {
        request_id: parsed.request_id.unwrap_or_default(),
        records,
    })
}

/// Feed each Firehose record (an OTLP `ExportMetricsServiceRequest` protobuf) into the
/// existing metrics decode/split/write path. Reuses `ingest_metrics` per record so identity,
/// content-addressed `block_id`, and idempotent writes are inherited unchanged.
pub async fn ingest_firehose_metrics(
    service: Arc<WebIngestionService>,
    records: Vec<Vec<u8>>,
) -> Result<(), OtelError> {
    for rec in records {
        ingest_metrics(service.clone(), bytes::Bytes::from(rec), Encoding::Protobuf).await?;
    }
    Ok(())
}
```

Notes:
- Zero records → the loop runs zero times → `Ok(())`. This lets the HTTP tests exercise
  gzip + auth + ack shape with an empty-`records` body and **no database** (see Testing).
- `ingest_metrics` already short-circuits an empty `resource_metrics` request, so a record that
  decodes to an empty request is a no-op, not an error.

### New route (`public` crate)

Add `rust/public/src/servers/firehose.rs`. The route is built with its own service extension,
its own Firehose-auth layer, and the shared body-limit layers — deliberately **outside**
`protected_app` so it never hits the global Bearer middleware.

```rust
const HEADER_ACCESS_KEY: &str = "X-Amz-Firehose-Access-Key";
const HEADER_REQUEST_ID: &str = "X-Amz-Firehose-Request-Id";

#[derive(serde::Serialize)]
struct FirehoseResponseBody<'a> {
    #[serde(rename = "requestId")]
    request_id: &'a str,
    timestamp: i64,
    #[serde(rename = "errorMessage", skip_serializing_if = "Option::is_none")]
    error_message: Option<&'a str>,
}

fn firehose_response(status: StatusCode, request_id: &str, error_message: Option<&str>) -> Response {
    let body = FirehoseResponseBody {
        request_id,
        timestamp: Utc::now().timestamp_millis(),
        error_message,
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .body(Body::from(serde_json::to_vec(&body).expect("serializing firehose response")))
        .expect("building firehose response")
}

fn request_id_from(headers: &HeaderMap) -> String {
    headers.get(HEADER_REQUEST_ID).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

/// Firehose-specific auth: read `X-Amz-Firehose-Access-Key`, synthesize an
/// `Authorization: Bearer <key>` header, and validate via the same AuthProvider the rest of
/// the ingestion service uses (reuses the constant-time keyring check verbatim). On failure,
/// return the Firehose error shape (non-200 JSON) so Firehose retries/spills rather than
/// dropping data.
async fn firehose_auth_middleware(
    provider: Arc<dyn AuthProvider>,
    req: Request,
    next: Next,
) -> Response {
    let request_id = request_id_from(req.headers());
    let Some(access_key) = req.headers().get(HEADER_ACCESS_KEY).and_then(|v| v.to_str().ok())
    else {
        return firehose_response(StatusCode::UNAUTHORIZED, &request_id, Some("missing X-Amz-Firehose-Access-Key"));
    };
    let mut headers = req.headers().clone();
    if let Ok(bearer) = HeaderValue::from_str(&format!("Bearer {access_key}")) {
        headers.insert(header::AUTHORIZATION, bearer);
    }
    let parts = HttpRequestParts { headers, method: req.method().clone(), uri: req.uri().clone() };
    match provider.validate_request(&parts as &dyn RequestParts).await {
        Ok(_ctx) => next.run(req).await,
        Err(e) => {
            warn!("[firehose auth_failure] {e}");
            firehose_response(StatusCode::UNAUTHORIZED, &request_id, Some("invalid access key"))
        }
    }
}

async fn firehose_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let mut request_id = request_id_from(&headers);
    let envelope = match handler::decode_firehose_envelope(&body) {
        Ok(e) => e,
        Err(err) => {
            error!("firehose decode error: {err}");
            let status = StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return firehose_response(status, &request_id, Some(&err.public_message()));
        }
    };
    if request_id.is_empty() {
        request_id = envelope.request_id.clone(); // header preferred; body requestId is fallback
    }
    match handler::ingest_firehose_metrics(service, envelope.records).await {
        Ok(()) => firehose_response(StatusCode::OK, &request_id, None),
        Err(err) => {
            error!("firehose ingest error: {err}");
            let status = StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            firehose_response(status, &request_id, Some(&err.public_message()))
        }
    }
}

/// Builds the Firehose sub-router: route + service extension + optional Firehose-auth layer +
/// shared ingestion body limits (gzip + 20 MiB wire / 300 MiB decompressed).
pub fn firehose_router(
    service: Arc<WebIngestionService>,
    auth_provider: Option<Arc<dyn AuthProvider>>,
) -> Router {
    let mut router = Router::new()
        .route("/ingestion/otlp/v1/metrics/firehose", post(firehose_handler))
        .layer(Extension(service));
    if let Some(provider) = auth_provider {
        router = router.layer(middleware::from_fn(move |req, next| {
            firehose_auth_middleware(provider.clone(), req, next)
        }));
    }
    apply_ingestion_body_limits(router)
}
```

### Wiring into `serve_ingestion` (`ingestion.rs`)

`auth_provider` is consumed by the existing `if let Some(provider) = auth_provider` block, so
clone it for Firehose first, then merge the Firehose sub-router into the top-level `app`
alongside `health_router` and `protected_app` (so it gets `observability_middleware` but not
the Bearer `auth_middleware`):

```rust
let firehose_auth = auth_provider.clone();
// ... existing protected_app build + global auth_middleware application ...
let firehose_app = super::firehose::firehose_router(service.clone(), firehose_auth);

let app = health_router
    .merge(protected_app)
    .merge(firehose_app)
    .layer(middleware::from_fn(observability_middleware));
```

(`service` is the `Arc<WebIngestionService>` created at `ingestion.rs:124`; it is currently
moved into `protected_app` via `.layer(Extension(service))` — change that to
`.layer(Extension(service.clone()))` so the Arc is also available to `firehose_router`.)

Add `pub mod firehose;` to `rust/public/src/servers/mod.rs`.

### `X-Amz-Firehose-Common-Attributes`

Ignored in v1. In OpenTelemetry 1.0.0 output mode the resource attributes travel inside the
protobuf record itself, so the common-attributes header (a feature aimed at the raw JSON/CWL
output formats) carries nothing this path needs. Noted, not blocking — see Open Questions.

### Idempotency & partial-batch retries

`block_id` is content-addressed over the resource-metrics payload (`block.rs:362`), so:
- A Firehose **retry** of a previously-succeeded batch re-computes identical `block_id`s and
  dedups on write — no duplicate rows.
- On a **partial** failure (records `0..N` written, record `N` fails), the handler returns
  non-200; Firehose retries the whole batch; records `0..N` dedup and record `N` is retried.
  No data loss, no duplication.

CloudWatch Metric Streams stamp distinct timestamps per scrape, so genuinely distinct data
never collides. (Two byte-identical requests would dedup — the same accepted trade-off as the
webhook path.)

## Implementation Steps

1. **otel-ingestion adapter** — add `decode_firehose_envelope` + `FirehoseEnvelope` +
   `ingest_firehose_metrics` to `rust/otel-ingestion/src/handler.rs`; add `base64` to
   `rust/otel-ingestion/Cargo.toml` `[dependencies]` (and `[dev-dependencies]` for building
   test fixtures).
2. **Unit tests (otel-ingestion)** — add `rust/otel-ingestion/tests/firehose_tests.rs`
   (see Testing). No DB.
3. **public firehose route** — add `rust/public/src/servers/firehose.rs` with
   `firehose_router` + `firehose_handler` + `firehose_auth_middleware` + `firehose_response`.
   Add `pub mod firehose;` to `servers/mod.rs`.
4. **Wire into server** — clone `auth_provider` before it is consumed, `Extension(service.clone())`,
   and `.merge(super::firehose::firehose_router(...))` in `serve_ingestion`.
5. **HTTP tests (public)** — add `rust/public/tests/firehose_tests.rs` (oneshot; see Testing);
   register a `[[test]]` entry with `required-features = ["server"]` and add `base64` + `flate2`
   to `rust/public/Cargo.toml` `[dev-dependencies]`.
6. **Python e2e** — extend `python/micromegas/tests/test_otlp_e2e.py` (see Testing).
7. **Docs** — add a CloudWatch Metric Streams / Firehose section to `mkdocs/docs/otlp/index.md`.
8. **CI** — `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`, then
   `python3 build/rust_ci.py`.

## Files to Modify / Create

- `rust/otel-ingestion/src/handler.rs` — add `decode_firehose_envelope`, `FirehoseEnvelope`,
  `ingest_firehose_metrics`.
- `rust/otel-ingestion/Cargo.toml` — add `base64` to `[dependencies]` + `[dev-dependencies]`.
- `rust/otel-ingestion/tests/firehose_tests.rs` — **new** pure unit tests.
- `rust/public/src/servers/firehose.rs` — **new** route + handler + auth + response builders.
- `rust/public/src/servers/mod.rs` — `pub mod firehose;`.
- `rust/public/src/servers/ingestion.rs` — clone `auth_provider`/`service`, merge Firehose router.
- `rust/public/Cargo.toml` — `[[test]]` for `firehose_tests` (`required-features=["server"]`) +
  `base64`, `flate2` dev-deps.
- `mkdocs/docs/otlp/index.md` — new CloudWatch Metric Streams / Firehose section.
- `python/micromegas/tests/test_otlp_e2e.py` — extend with a Firehose end-to-end test.

## Trade-offs

- **Dedicated Firehose auth vs. reusing the global Bearer middleware.** Firehose can only send
  its credential in `X-Amz-Firehose-Access-Key`, so the route can't inherit the
  `Authorization: Bearer` middleware. A per-route auth shim that synthesizes a bearer header and
  calls the same `AuthProvider` reuses the constant-time keyring check verbatim while keeping
  AWS-specific header knowledge out of the generic `auth` crate. Alternative — teaching
  `RequestParts::bearer_token` to fall back to the Firehose header — was rejected: it leaks an
  AWS concept into the generic auth trait and widens *every* route's credential surface.
- **Reuse `ingest_metrics` per record vs. a bespoke batch decoder.** Looping `ingest_metrics`
  over records inherits identity, content-addressed `block_id`, idempotent writes, and the
  analytics processor unchanged, at the cost of per-record process/stream re-registration
  (already idempotent and cheap). A bespoke decoder would duplicate all of it.
- **Separate sub-router outside `protected_app`.** Costs a second `Extension(service)` and its
  own body-limit layer application, but is the clean way to give the route different auth
  without special-casing a path inside the shared middleware.
- **Ignore `X-Amz-Firehose-Common-Attributes` in v1.** Resource attributes already ride inside
  the OTLP-1.0.0 protobuf; the header is redundant for this output mode.

## Documentation

`mkdocs/docs/otlp/index.md` — add a **"CloudWatch Metric Streams (Kinesis Firehose)"** section:
- The endpoint `POST /ingestion/otlp/v1/metrics/firehose` and the pipeline
  **Metric Stream → Firehose → micromegas** (no Lambda / collector).
- Requirement: the Metric Stream must use **OpenTelemetry 1.0.0** output format; records land in
  `measures`, same as native OTLP metrics.
- AWS delivery-stream setup: HTTP endpoint URL, **Access key = a micromegas API key** (the value
  from `MICROMEGAS_API_KEYS`, sent as `X-Amz-Firehose-Access-Key`), content encoding gzip,
  buffering, and S3 backup for failed/all records.
- The ack contract (200 + `{requestId, timestamp}`; non-200 triggers retry then S3 spill) and
  that TLS termination in front of the listener is required for production (same note as the
  Bearer OTLP section).

## Testing Strategy

- **Unit (otel-ingestion, `tests/firehose_tests.rs`, no DB):**
  - `decode_firehose_envelope` on a single-record envelope → one record; bytes round-trip a
    protobuf `ExportMetricsServiceRequest` (build one with `prost`, base64-encode, wrap in JSON,
    decode, assert the decoded bytes parse back and `split_metrics` yields one `PreparedBlock`).
  - **Multi-record batch** → `records.len()` decoded entries, order preserved.
  - Malformed JSON → `OtelError::Parse`; malformed base64 in a record → `OtelError::Parse`.
  - Empty `records` (and absent `records`) → `Ok` with zero records; missing `requestId` →
    empty string.
- **HTTP (public, `tests/firehose_tests.rs`, `tower::ServiceExt::oneshot`, no live DB):** build
  the service with the lazy-pool + `InMemory` pattern from `rust/ingestion/tests/readiness.rs`
  (`PgPool::connect_lazy` + `object_store::memory::InMemory`) and a keyring-backed
  `ApiKeyAuthProvider`.
  - **Access-key rejection:** missing / wrong `X-Amz-Firehose-Access-Key` → non-200 with the
    `{requestId, timestamp, errorMessage}` shape; `requestId` echoes
    `X-Amz-Firehose-Request-Id`. Auth fails before the handler, so no DB call.
  - **Access-key accept + ack shape + gzip:** valid key + gzipped (`flate2` `GzEncoder`,
    `Content-Encoding: gzip`) envelope with **empty `records`** → 200 with
    `{requestId, timestamp}` and no `errorMessage`. Empty records means `ingest_metrics` is never
    called, so this covers decompression + envelope parse + auth + ack shape without a DB.
  - **Dev mode (no provider):** `firehose_router(service, None)` accepts a request with no
    access-key header (open, consistent with other routes).
  - **Full multi-record ingest success** (records actually written) → `#[ignore]`, requires a
    live stack (`MICROMEGAS_SQL_CONNECTION_STRING` + object store), matching the repo convention
    for DB-backed tests.
- **Integration (Python e2e):** extend `python/micromegas/tests/test_otlp_e2e.py` — build an
  OTLP `ExportMetricsServiceRequest` (per-run-unique `service.instance.id` so
  `discover_process_id` in `otlp_helpers.py` finds it), base64-encode it, wrap in a Firehose
  envelope, POST to `/ingestion/otlp/v1/metrics/firehose` with `X-Amz-Firehose-Request-Id` +
  `X-Amz-Firehose-Access-Key`, assert the 200 ack `requestId` echoes the header, then
  `assert_eventually` a `measures` row appears. Add a multi-record and a rejected-key assertion.
- **CI:** `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
  `python3 build/rust_ci.py`.

## Open Questions

None blocking:
- **`X-Amz-Firehose-Common-Attributes`** — ignored in v1 (resource attributes ride inside the
  OTLP-1.0.0 protobuf). Revisit only if a non-OTLP Metric Stream output format is ever supported.
- **Auth providers** — effectively API-key only: an OIDC-only deployment can't produce a Firehose
  access key that validates as a JWT. Expected; operators enabling Firehose configure
  `MICROMEGAS_API_KEYS`. A `Multi` provider that includes API keys works unchanged.
