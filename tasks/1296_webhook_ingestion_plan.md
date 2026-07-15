# Header-Described Webhook Ingestion Plan

## Overview

Add a generic `POST /ingestion/webhook` endpoint that lets any header-capable webhook
producer (GitLab, GitHub, generic SaaS) report directly to micromegas with no external
transformer service in between. The endpoint synthesizes an OTLP `Resource` from a small
set of static `X-Micromegas-*` request headers, wraps the **verbatim request body** as a
single log record's `msg`, and reuses the existing OTLP logs identity/block/write path
end-to-end. No per-source logic, no payload schema knowledge server-side; the raw body is
parsed at query time with the existing JSONB UDFs. This obviates the runtime component of
the #1167 transformer recipe.

## Goal (one sentence)

Accept arbitrary webhook JSON directly into `log_entries` by deriving process/stream
identity from request headers and storing the body opaquely, reusing the OTLP logs pipeline.

## Current State

### OTLP logs ingestion path (the pipeline to reuse)

The whole logs path is already factored so that identity synthesis + block splitting +
idempotent writes are decoupled from HTTP framing:

- **Route registration** — `rust/public/src/servers/otlp.rs:216` `otlp_router()` registers
  `POST /ingestion/otlp/v1/logs` (+ metrics/traces). The sub-router layers a 20 MiB
  compressed-body limit (`OTLP_BODY_LIMIT_BYTES`, `otlp.rs:38`), a 300 MiB
  decompressed cap (`OTLP_DECOMPRESSED_BODY_LIMIT_BYTES`, `otlp.rs:45`), and
  `RequestDecompressionLayer` (gzip).
- **HTTP handler** — `otlp.rs:160` `logs_handler` negotiates `Content-Type`, then calls
  `handler::ingest_logs`.
- **Framework-free handler** — `rust/otel-ingestion/src/handler.rs:119` `ingest_logs`
  parses bytes → `ExportLogsServiceRequest`, calls `split_logs`, then `write_blocks`.
- **Per-resource split** — `rust/otel-ingestion/src/block.rs:254` `split_logs` produces one
  `PreparedBlock` per `ResourceLogs`, deriving `process_id` / `stream_id` / `block_id` in
  `build_prepared_block` (`block.rs:180`). It **backfills `observed_time_unix_nano = now`**
  for records missing both timestamps (`block.rs:270-285`) and derives `block_id` from the
  pre-backfill bytes so retries dedup (`block.rs:265-266`).
- **Idempotent writes** — `handler.rs:66` `write_blocks` calls
  `WebIngestionService::register_otel_process` (`web_ingestion_service.rs:349`),
  `register_otel_stream` (`:273`), `insert_block_typed` (`:142`) — all idempotent.
- **Identity** — `rust/otel-ingestion/src/identity.rs`: `process_id_from_resource` (`:184`)
  hashes the resource attribute tuple under `NS_OTEL_PROCESS_V1`;
  `stream_id_from_process_signal` (`:230`); `ProcessFromResource::build` (`block.rs:396`)
  maps `service.namespace`/`service.name` → `exe`, `host.name` → `computer`, etc.

### How a stored `ResourceLogs` becomes `log_entries` rows

`rust/analytics/src/lakehouse/otel/logs_block_processor.rs` prost-decodes the stored
`ResourceLogs` and emits one row per `LogRecord`:

- `target` ← **instrumentation scope name** (`logs_block_processor.rs:99`).
- `msg` ← `LogRecord.body` stringified (`:123-127`); the `msg` column is `Utf8`
  (`log_entries_table.rs:73`, a `StringBuilder` — no length cap), so a full webhook body fits.
- `level` ← `severity_number_to_level(record.severity_number)`.
- `time` ← `time_unix_nano`, else `observed_time_unix_nano`; records with neither are dropped
  (`:101-112`).

This is why storing the body as a log record body, with the target as the scope name,
lands exactly where the header table in the issue says it should — **with no new processor**.

### Auth & body limits

`rust/public/src/servers/ingestion.rs:108` `serve_ingestion` merges `otlp_router()` into
`protected_app` (`:131-135`), then wraps the whole thing in `auth_middleware`
(`:138-142`) — bearer-key auth already covers anything merged there. The parent router
disables the default body limit and sets a 100 MiB cap; each sub-router (like `otlp_router`)
applies its own tighter limit.

### Query-time JSONB (correction vs. the issue examples)

Registered JSONB UDFs (`rust/datafusion-extensions/src/jsonb/`): `jsonb_parse`,
`jsonb_get` (single key, **not** dotted-path), `jsonb_as_string`, `jsonb_as_i64`,
`jsonb_as_f64`, `jsonb_array_length`, `jsonb_object_keys`, `jsonb_path_query`,
`jsonb_path_query_first`, `jsonb_format_json`. The issue's `jsonb_as_i` and
`jsonb_extract_path` do **not** exist — the docs example must use `jsonb_as_i64` and either
nested `jsonb_get` or `jsonb_path_query_first('$.object_attributes.iid')` for nested access.

## Design

### Principle: build a synthetic OTLP logs request, then reuse everything

The webhook endpoint constructs one `ExportLogsServiceRequest` in memory and feeds it to the
existing `split_logs` + `write_blocks` path. No new identity function, no new block
processor, no new stream/process model — this is the open/closed win.

```
POST /ingestion/webhook
  headers: X-Micromegas-Service-Name, X-Micromegas-Service-Namespace, X-Micromegas-Target
  body:    <opaque JSON bytes>
      │
      ▼   (public/src/servers/webhook.rs)  parse headers → resource attrs + target string
      ▼   (otel-ingestion handler::ingest_webhook)  build ExportLogsServiceRequest:
              Resource{ service.namespace, service.name }
              └ ScopeLogs{ scope.name = target }
                  └ LogRecord{ body = <body as string>, severity_number = INFO,
                               time/observed = 0 (split_logs backfills observed = now) }
      ▼   split_logs  → PreparedBlock  (process_id / stream_id / block_id derived)
      ▼   write_blocks → register_otel_process / register_otel_stream / insert_block_typed
      ▼   analytics OtelLogsBlockProcessor → log_entries row (target, msg=body, time=now)
```

### Header convention

| Header | Maps to | Result |
|---|---|---|
| `X-Micromegas-Service-Name` | resource `service.name` | `processes.exe` / `log_entries.exe` |
| `X-Micromegas-Service-Namespace` | resource `service.namespace` | folded into `exe` as `ns/name`, and into `process_id` |
| `X-Micromegas-Target` | instrumentation scope name | `log_entries.target` filter |

- Missing headers → empty attribute (same as an OTLP resource that omits them). If
  `service.name` is absent, `exe` is empty and identity collapses toward a degenerate
  resource; the endpoint should still accept but the server logs a `debug!` (reuse the
  existing `is_degenerate_resource` warning path in `build_prepared_block`).
- Header values are ASCII per HTTP; decode with `.to_str()`, skip non-decodable headers.

### New handler entry point (otel-ingestion crate)

Add to `rust/otel-ingestion/src/handler.rs`:

```rust
/// Generic webhook → single-log-record ingestion.
/// Builds a synthetic ExportLogsServiceRequest (one resource, one scope, one record whose
/// body is the verbatim request body) and reuses the OTLP logs split/write path.
pub async fn ingest_webhook(
    service: Arc<WebIngestionService>,
    resource_attrs: Vec<KeyValue>,   // built from X-Micromegas-Service-* headers
    target: String,                  // X-Micromegas-Target → scope name
    body: bytes::Bytes,
) -> Result<(), OtelError> {
    let req = build_webhook_request(resource_attrs, target, &body);
    let blocks = split_logs(req).map_err(|e| OtelError::Parse {
        signal: Signal::Logs,
        message: format!("split_logs (webhook): {e}"),
    })?;
    write_blocks(&service, Signal::Logs, blocks).await?;
    Ok(())
}
```

`build_webhook_request` (private helper, unit-testable): construct `Resource { attributes }`,
one `ScopeLogs { scope: InstrumentationScope { name: target, .. }, log_records: vec![rec] }`,
where `rec.body = Some(AnyValue { value: Some(StringValue(body_string)) })` and
`rec.severity_number = SeverityNumber::Info as i32`. Leave `time_unix_nano` /
`observed_time_unix_nano` at 0 so the existing `split_logs` backfill stamps ingestion time.

Body → string: `String::from_utf8_lossy(&body).into_owned()`. Webhook bodies are JSON
(UTF-8); lossy conversion avoids rejecting a rare malformed byte and keeps the endpoint
tolerant. An empty body → 400 (nothing to store).

All proto types needed (`KeyValue`, `AnyValue`, `any_value`, `InstrumentationScope`,
`ScopeLogs`, `LogRecord`, `ResourceLogs`, `Resource`, `SeverityNumber`,
`ExportLogsServiceRequest`) are already re-exported from `rust/otel-ingestion/src/proto.rs`.

### New route (public crate)

Add `rust/public/src/servers/webhook.rs`:

```rust
pub fn webhook_router() -> Router {
    Router::new()
        .route("/ingestion/webhook", post(webhook_handler))
        .layer(RequestDecompressionLayer::new().gzip(true))
        .layer(RequestBodyLimitLayer::new(WEBHOOK_BODY_LIMIT_BYTES))
        .layer(DefaultBodyLimit::max(WEBHOOK_DECOMPRESSED_BODY_LIMIT_BYTES))
}
```

`webhook_handler(Extension<Arc<WebIngestionService>>, HeaderMap, Bytes)`:
1. Read `X-Micromegas-Service-Name` / `-Service-Namespace` → `Vec<KeyValue>`
   (only push a `KeyValue` for headers actually present and decodable).
2. Read `X-Micromegas-Target` → `String` (empty if absent → empty `target`).
3. Reject empty body with 400.
4. Call `handler::ingest_webhook`, map `OtelError` → HTTP. Reuse the existing
   `OtelError` → status mapping already used by the OTLP handlers rather than inventing a
   new error surface; a plain JSON/text body is fine here (the OTLP `google.rpc.Status`
   proto response shape is spec-mandated for OTLP only, not for this endpoint).

**Body-limit DRY:** the 20 MiB / 300 MiB constants and the three layers are identical to
`otlp_router`. Extract a shared helper (e.g. `fn ingestion_body_limit_layers(router) ->
Router` or shared `pub(crate) const`s in a small `servers` module) and use it from both
`otlp_router` and `webhook_router` instead of copy-pasting the constants.

Register in `serve_ingestion` (`ingestion.rs:132`):

```rust
let mut protected_app = register_routes(Router::new())
    .merge(super::otlp::otlp_router())
    .merge(super::webhook::webhook_router())   // ← new; inherits bearer auth
    ...
```

Add `pub mod webhook;` to `rust/public/src/servers/mod.rs`.

### Content-Type

The endpoint does **not** negotiate content type — the body is opaque bytes stored verbatim.
Producers typically send `application/json`; we neither require nor parse it. (Contrast with
`otlp_router`, which must switch proto/JSON decoding.)

### Idempotency / dedup

Because timestamps are left at 0 (backfilled only after `block_id` is computed from the
pre-backfill bytes, `block.rs:265`), two identical deliveries — same headers **and** same
body — hash to the same `block_id` and dedup on retry. This is the desirable behavior for
webhook re-deliveries. Two genuinely distinct events with byte-identical bodies would also
dedup; acceptable and noted (rare, and the alternative — injecting wall-clock into the
payload — would break retry idempotency).

## Implementation Steps

1. **otel-ingestion handler** — add `build_webhook_request` (private) + `ingest_webhook`
   (public) to `rust/otel-ingestion/src/handler.rs`. Import the needed proto types.
2. **Unit tests (otel-ingestion)** — add `rust/otel-ingestion/tests/webhook_tests.rs`:
   - `build_webhook_request` produces one resource / one scope / one record; scope name ==
     target; body preserved verbatim; severity == Info.
   - `split_logs` on the synthetic request yields exactly one `PreparedBlock` with a
     backfilled timestamp and a `process_id` matching `process_id_from_resource` for the
     same attrs.
   - Identical (attrs, target, body) → identical `block_id`; differing body → different id.
3. **public webhook route** — add `rust/public/src/servers/webhook.rs` with
   `webhook_router()` + `webhook_handler`, header parsing, empty-body 400, `OtelError`
   mapping. Add `pub mod webhook;` to `servers/mod.rs`.
4. **Body-limit DRY** — factor the shared body-limit/decompression layers (and 20/300 MiB
   constants) out of `otlp.rs` and reuse in both routers.
5. **Wire into server** — `.merge(super::webhook::webhook_router())` in
   `serve_ingestion` (`ingestion.rs:132`).
6. **Integration test** — extend the existing ingestion server test harness (same one that
   exercises the OTLP routes) to POST a sample GitLab-shaped JSON body with the three
   headers and assert a `log_entries` row appears with the expected `target`, `exe`, and
   `msg == body`.
7. **Docs** — add a "Webhook ingestion" section to `mkdocs/docs/otlp/index.md` (see below).
8. **CI** — `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`, then
   `python3 build/rust_ci.py`.

## Files to Modify / Create

- `rust/otel-ingestion/src/handler.rs` — add `ingest_webhook` + `build_webhook_request`.
- `rust/otel-ingestion/tests/webhook_tests.rs` — **new** unit tests.
- `rust/public/src/servers/webhook.rs` — **new** route + handler.
- `rust/public/src/servers/mod.rs` — `pub mod webhook;`.
- `rust/public/src/servers/otlp.rs` — extract shared body-limit layers (DRY).
- `rust/public/src/servers/ingestion.rs` — merge `webhook_router()`.
- `mkdocs/docs/otlp/index.md` — new "Webhook ingestion" section.
- Integration test file under the existing ingestion server test suite.

## Trade-offs

- **Synthetic OTLP request vs. a bespoke webhook block writer.** Building an
  `ExportLogsServiceRequest` and reusing `split_logs`/`write_blocks` costs one in-memory
  proto build + encode, but reuses identity, backfill, dedup, idempotent writes, and the
  analytics processor unchanged. A bespoke path would duplicate all of that. Chosen: reuse.
- **Opaque body vs. server-side flatten into `properties`.** Storing the raw body preserves
  nested objects and arrays (queryable via JSONB `jsonb_array_length` /
  `jsonb_path_query`), keeps the server free of payload schema, and defers all structure to
  query time. A flatten step would lose array fidelity and bake per-source knowledge in.
- **Single `/ingestion/webhook` + target-in-header vs. `/ingestion/webhook/{source}`.**
  Header-in keeps the route generic and matches the "producer self-describes via headers"
  philosophy — the path stays constant across all producers. Chosen: single endpoint.
- **Ingestion time vs. event time.** Using wall-clock ingestion time as `time` keeps the
  server from parsing the body at all; webhooks deliver within seconds, and the exact event
  timestamp remains recoverable from the body via JSONB when precise math is needed.
- **UTF-8 lossy vs. strict.** Lossy keeps the endpoint tolerant of a stray byte; webhook
  JSON is UTF-8 in practice, so lossiness is effectively never exercised.

## Documentation

`mkdocs/docs/otlp/index.md` — add a **"Webhook ingestion"** section (near the Client
recipes / EventBridge material) covering:

- The endpoint, the three `X-Micromegas-*` headers, and that the body is stored verbatim.
- A GitLab group-webhook configuration example (custom headers set once).
- A **corrected** query example using real UDFs — `jsonb_parse(msg)`, `jsonb_get`,
  `jsonb_as_i64`, `jsonb_array_length`, and `jsonb_path_query_first` for nested access
  (the issue's `jsonb_as_i` / `jsonb_extract_path` / dotted `jsonb_get` do not exist).
- A note that this replaces the runtime transformer of the #1167 recipe: "point the webhook
  at `/ingestion/webhook` with these headers." The #1167 docs task collapses to this
  section (external transformer no longer required at runtime).

## Testing Strategy

- **Unit (otel-ingestion):** `build_webhook_request` shape, verbatim body, scope→target,
  severity; `split_logs` single-block + identity; `block_id` dedup determinism.
- **Route/handler:** empty-body → 400; missing headers tolerated (empty attrs/target);
  `OtelError` mapping.
- **Integration:** POST a sample webhook JSON with the three headers to a running ingestion
  server, materialize, and assert a `log_entries` row with expected `target`, `exe`, and
  `msg == body`; then run a JSONB query against `msg` to prove nested/array access works.
- **CI:** `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
  `python3 build/rust_ci.py`.

## Open Questions

1. **Producer shared-secret verification** (e.g. GitLab `X-Gitlab-Token`) — out of scope for
   v1? The bearer ingestion key already authenticates the request. Recommendation: defer;
   the bearer key is sufficient, and a per-producer secret adds per-source config the design
   deliberately avoids. Revisit if a deployment needs to expose the endpoint without the
   bearer key.
2. **Endpoint shape** — confirm single `/ingestion/webhook` with target-in-header (this
   plan's choice) over `/ingestion/webhook/{source}`.
3. **Default severity** — Info assumed for every webhook record. Acceptable, or should a
   future `X-Micromegas-Severity` header be reserved (not implemented now)?
