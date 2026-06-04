# OTLP/JSON Content-Type Support Plan

## Overview

The ingestion server currently accepts only `application/x-protobuf` on its three
OTLP/HTTP routes (`/ingestion/otlp/v1/logs`, `/metrics`, `/traces`); any other
content type returns `415`. This plan adds support for the OTLP/JSON encoding
(`application/json`) so tools that emit OTLP/JSON natively — and, specifically,
AWS EventBridge **API Destinations** with input transformers — can POST directly
to the endpoint with no Lambda translation layer. Requests are deserialized via the
canonical OTLP/JSON mapping already implemented by `opentelemetry-proto`'s
`with-serde` feature, and the response is returned in the same encoding as the
request (JSON in → JSON out, proto in → proto out).

Issue: https://github.com/madesroches/micromegas/issues/1115

## Current State

### Routing and content-type validation
`rust/public/src/servers/otlp.rs` registers the three routes and validates the
request before handing the body to the framework-agnostic handler crate:

- `check_content_type` (`otlp.rs:53`) parses the `Content-Type` header (tolerating
  parameters like `; charset=utf-8`), lower-cases the media type, and accepts only
  `application/x-protobuf` (`CONTENT_TYPE_PROTOBUF`, `otlp.rs:49`). Anything else →
  `OtlpHttpError::WrongContentType` → `415` (`otlp.rs:84`).
- Each handler (`logs_handler` `otlp.rs:142`, `metrics_handler`, `traces_handler`)
  runs `check_content_type`, calls the matching `handler::ingest_*`, and on success
  builds a protobuf response via `proto_response` (`otlp.rs:130`). Errors flow through
  `OtlpHttpError::into_otlp_response` (`otlp.rs:82`).
- `build_error_response` (`otlp.rs:110`) always emits a protobuf-encoded
  `google.rpc.Status` body with `Content-Type: application/x-protobuf`.
- `proto_response` (`otlp.rs:130`) always emits `application/x-protobuf`.

### Handler / parsing
`rust/otel-ingestion/src/handler.rs` is deliberately HTTP-framework-free:

- `parse<M: Message + Default>` (`handler.rs:22`) decodes protobuf via
  `prost::Message::decode`.
- `ingest_logs` / `ingest_metrics` / `ingest_traces` (`handler.rs:101`, `118`, `135`)
  take `(Arc<WebIngestionService>, bytes::Bytes)`, parse the proto request, early-return
  an empty response if there are no resources, split into per-resource blocks
  (`split_logs` etc.), write them, and return a typed `Export*ServiceResponse`.

### Proto types
`rust/otel-ingestion/src/proto.rs` re-exports the `Export*ServiceRequest/Response`
types from `opentelemetry-proto` and defines a hand-rolled `Status` struct
(`proto.rs:35`) used as the error body. `Status` currently derives only
`prost::Message`.

### Dependency configuration
- `rust/Cargo.toml:64`:
  `opentelemetry-proto = { version = "0.31", default-features = false, features = ["gen-tonic-messages", "logs", "metrics", "trace"] }`
  — note: **`with-serde` is not enabled.**
- `rust/otel-ingestion/Cargo.toml` depends on `opentelemetry-proto`, `prost`,
  `bytes`, etc. It has **no** `serde` / `serde_json` dependency.
- Workspace root already defines `serde` and `serde_json` (`rust/Cargo.toml:74-75`).

### Key finding: `opentelemetry-proto` already implements canonical OTLP/JSON
The crate's `with-serde` feature (`opentelemetry-proto` 0.31, verified against the
vendored source under `~/.cargo/registry/.../opentelemetry-proto-0.31.0/`) adds
`serde::{Serialize, Deserialize}` to every message with attributes that match the
OTLP/JSON canonical mapping:

- `#[serde(rename_all = "camelCase")]` on every struct (`timeUnixNano`,
  `severityNumber`, `resourceLogs`, …).
- `#[serde(default)]` so missing fields are tolerated.
- `trace_id` / `span_id` use hex string (de)serializers
  (`deserialize_from_hex_string`).
- 64-bit ints (`time_unix_nano`, `observed_time_unix_nano`, …) use
  `serialize_u64_to_string` / `deserialize_string_to_u64`.
- `AnyValue` uses a custom value (de)serializer.

The crate ships a `json_serde` test that round-trips the official
`opentelemetry-proto/examples/{trace,logs,metrics}.json` fixtures, so the mapping is
spec-conformant for SDK-emitted payloads.

**Limitation to be aware of** (see Trade-offs): `deserialize_string_to_u64` /
`deserialize_string_to_i64` deserialize into a `String` first, so they accept the
**string** form (`"timeUnixNano": "1700000000000000000"`) but **reject the bare
number form** (`"timeUnixNano": 1700000000000000000`). The canonical OTLP/JSON
mapping *mandates* the string form, so conformant senders are fine; lenient proto3
JSON receivers also accept numbers, which this path will not.

### Documentation
`mkdocs/docs/otlp/index.md` currently states JSON is not supported in several places
(`index.md:17`, `:172`, `:271`).

## Design

### 1. Encoding enum (shared, framework-agnostic)
Add a small `Encoding` enum to the `micromegas-otel-ingestion` crate (it is a protocol
concept used by both parsing in the handler crate and response building in the server):

```rust
// rust/otel-ingestion/src/handler.rs (or a new src/encoding.rs, re-exported from lib)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Protobuf,
    Json,
}
```

### 2. Content-Type → Encoding in the server
Replace `check_content_type` (which returns `Result<(), _>`) with a function that maps
the media type to an `Encoding`:

```rust
// rust/public/src/servers/otlp.rs
const CONTENT_TYPE_PROTOBUF: &str = "application/x-protobuf";
const CONTENT_TYPE_JSON: &str = "application/json";

fn content_type_encoding(headers: &HeaderMap) -> Result<Encoding, OtlpHttpError> {
    // parse media type as today (split ';', trim, to_ascii_lowercase)
    match media.as_str() {
        CONTENT_TYPE_PROTOBUF => Ok(Encoding::Protobuf),
        CONTENT_TYPE_JSON     => Ok(Encoding::Json),
        _ => Err(OtlpHttpError::WrongContentType),
    }
}
```

### 3. Parse dispatch in the handler
Generalize `parse` to handle both encodings. The request types implement **both**
`prost::Message` and `serde::de::DeserializeOwned` once `with-serde` is on:

```rust
fn parse<M: Message + Default + serde::de::DeserializeOwned>(
    body: &[u8],
    signal: Signal,
    encoding: Encoding,
) -> Result<M, OtelError> {
    match encoding {
        Encoding::Protobuf => M::decode(body),                  // existing path
        Encoding::Json      => serde_json::from_slice(body),    // new path
    }
    .map_err(|e| OtelError::Parse { signal, message: format!("decoding {} ({encoding}): {e}", signal.as_str()) })
}
```

Thread `encoding: Encoding` into the three `ingest_*` signatures:

```rust
pub async fn ingest_logs(service: Arc<WebIngestionService>, body: bytes::Bytes, encoding: Encoding)
    -> Result<ExportLogsServiceResponse, OtelError>
```

(The two `map_err` closures differ slightly so the message reflects encoding; keep the
existing `Parse` error variant — no new variant needed since JSON parse failures are
still `400 INVALID_ARGUMENT`, same as proto.)

### 4. Response encoding in the server
Make the success/error response builders encoding-aware so the response mirrors the
request format (per issue scope and the OTLP/HTTP spec, which says JSON requests get
JSON responses).

```rust
fn success_response<M: Message + serde::Serialize>(msg: M, encoding: Encoding) -> Response {
    match encoding {
        Encoding::Protobuf => /* existing proto_response: encode_to_vec, x-protobuf */,
        Encoding::Json      => /* serde_json::to_vec(&msg), application/json */,
    }
}
```

`build_error_response` gains an `encoding` parameter and serializes the `Status` either
as protobuf (today) or JSON. To serialize `Status` as JSON, add `serde::Serialize`
(+`Deserialize` for symmetry/tests) to the hand-rolled `Status` struct in `proto.rs`:

```rust
#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
pub struct Status { pub code: i32, pub message: String }
```

JSON error body shape: `{"code": 3, "message": "..."}` — consistent with
`google.rpc.Status` proto3 JSON. `Retry-After` header handling is unchanged.

**415 (`WrongContentType`) responses**: the request's content type is by definition
unknown/unsupported, so we can't infer the client's preferred encoding. Keep emitting
the protobuf `Status` (the OTLP/HTTP default) for this one case, exactly as today.

### Request flow (after change)

```
handler reads Content-Type
  ├─ application/x-protobuf → Encoding::Protobuf
  ├─ application/json       → Encoding::Json
  └─ other / missing        → 415 (proto Status body)
        │
        ▼
  ingest_*(service, body, encoding)
        │  parse: prost::decode | serde_json::from_slice
        ▼
  split_* → write_blocks  (unchanged)
        │
        ▼
  success_response(resp, encoding)
        ├─ Protobuf → x-protobuf, encode_to_vec
        └─ Json     → application/json, serde_json (empty resp → {"partialSuccess":null})
```

## Implementation Steps

1. **Enable serde on the proto crate.** In `rust/Cargo.toml:64`, add `"with-serde"` to
   the `opentelemetry-proto` feature list (keep alphabetical/existing order of the list
   as-is; append `with-serde`). This pulls in `serde`, `serde_json`, `base64`,
   `const-hex` transitively for that crate.

2. **Add deps to the handler crate.** In `rust/otel-ingestion/Cargo.toml`, add
   (alphabetically) `serde = { workspace = true }` and `serde_json = { workspace = true }`.

3. **Define `Encoding`.** Add the enum to `rust/otel-ingestion/src/handler.rs` (or a new
   `encoding.rs` module) with a `Display` impl (`"protobuf"` / `"json"`) for error
   messages; re-export from `lib.rs` if placed in its own module.

4. **Generalize `parse` + `ingest_*`.** In `handler.rs`: make `parse` `pub` (required
   so integration tests in `tests/json_tests.rs` can call it directly); add the
   `DeserializeOwned` bound and `encoding` param to `parse`; dispatch prost vs
   `serde_json`; add `encoding` to the three `ingest_*` signatures and pass it through.

5. **Make `Status` serde-serializable.** In `rust/otel-ingestion/src/proto.rs`, add
   `serde::Serialize, serde::Deserialize` to the `Status` derive.

6. **Server: content-type → encoding.** In `rust/public/src/servers/otlp.rs`, replace
   `check_content_type` with `content_type_encoding`; add `CONTENT_TYPE_JSON`.

7. **Server: encoding-aware responses.** Replace `proto_response` with
   `success_response(msg, encoding)`; add `encoding` to `build_error_response` and
   `OtlpHttpError::into_otlp_response` (the `WrongContentType` arm stays protobuf).

8. **Server: wire the handlers.** Each of `logs_handler` / `metrics_handler` /
   `traces_handler`: resolve `encoding` from headers (415 on error), pass `encoding` to
   `ingest_*` and to `success_response`; route `OtelError` through
   `into_otlp_response(encoding)`. Update the `WrongContentType` 415 message to mention
   both accepted types.

9. **Tests** (see Testing Strategy).

10. **Docs.** Update `mkdocs/docs/otlp/index.md` (see Documentation).

11. **CI.** Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`,
    `python3 ../build/rust_ci.py` from `rust/`.

## Files to Modify

- `rust/Cargo.toml` — add `with-serde` to `opentelemetry-proto` features.
- `rust/otel-ingestion/Cargo.toml` — add `serde`, `serde_json`.
- `rust/otel-ingestion/src/handler.rs` — `Encoding` enum, `parse` dispatch, `ingest_*`
  signatures.
- `rust/otel-ingestion/src/lib.rs` — re-export `Encoding` (if in its own module).
- `rust/otel-ingestion/src/proto.rs` — serde derive on `Status`.
- `rust/public/src/servers/otlp.rs` — content-type→encoding, encoding-aware responses,
  handler wiring.
- `rust/otel-ingestion/tests/json_tests.rs` — **new**, JSON parse/equivalence tests.
- `mkdocs/docs/otlp/index.md` — document JSON support.

## Trade-offs

- **Reuse `opentelemetry-proto`'s `with-serde` vs. hand-rolled JSON.** Reusing the
  crate's serde derives is DRY and spec-conformant (the crate's own `json_serde` test
  validates against the official OTLP example fixtures). A hand-rolled mapping would be
  large and a maintenance burden. Chosen: reuse.

- **Strict string-encoded 64-bit ints.** The crate's u64/i64 deserializers accept only
  the JSON **string** form and reject bare numbers. The canonical OTLP/JSON mapping
  mandates strings, so conformant senders work; but a lenient proto3 receiver would also
  accept numbers. For the EventBridge use case this is acceptable because the input
  transformer template is author-controlled — timestamps can be templated as quoted
  strings. Writing custom lenient deserializers would mean abandoning the crate's
  derives entirely. Chosen: accept the limitation for v1 and document it. (See Open
  Questions.)

- **`Encoding` location.** Placed in the handler crate rather than the server because
  parsing (handler crate) and response building (server crate) both need it; duplicating
  it would violate DRY. The crate stays framework-agnostic — `Encoding` carries no axum
  types.

- **Response mirrors request.** Per the issue scope and OTLP/HTTP spec. The alternative
  (always proto responses) is simpler but non-conformant and would confuse JSON clients
  parsing the response body.

## Documentation

Update `mkdocs/docs/otlp/index.md`:
- `:17` — change "JSON-encoded OTLP … not supported" to note `application/json` is now
  accepted (string-encoded 64-bit fields per OTLP/JSON spec).
- `:172` (Content-Type table row) — list both `application/x-protobuf` and
  `application/json`.
- `:174` (Success row) — note the response Content-Type mirrors the request.
- `:262` (Limitations bullet) — remove the `**Protobuf only.**` bullet (or replace it
  with a note that gRPC transport is not implemented, which remains true).
- `:271` (troubleshooting `415`) — update to reflect that JSON is now accepted, and that
  the remaining `415` causes are missing/other content types and non-gzip compression.
- Consider a short "OTLP/JSON & EventBridge" subsection describing the API Destination
  use case and the string-encoded-timestamp requirement.

## Testing Strategy

Existing handler-level DB round-trip tests are deferred (per the note in
`tests/split_tests.rs` — they'd need a mock `WebIngestionService` or real Postgres). The
new JSON support can be verified without a DB:

1. **New `rust/otel-ingestion/tests/json_tests.rs`:**
   - **Proto/JSON parse equivalence.** Build a request with the existing `fixtures.rs`
     helpers (`make_logs_request`, etc.), serialize it to JSON with `serde_json`,
     deserialize back with `parse::<_>(..., Encoding::Json)`, and assert it equals the
     original — then assert `split_logs` produces identical `PreparedBlock`s for both
     encodings (block ids are content-addressed, so equality is a strong check).
   - **Official-fixture parse.** Deserialize a canonical OTLP/JSON example
     (logs/traces, mirroring the upstream `examples/*.json`) and assert
     `split_*` succeeds with expected block counts/bounds.
   - **String-encoded timestamps.** Assert `"timeUnixNano": "1700000000000000000"`
     parses and the resulting block bound matches; assert the bare-number form
     (`1700000000000000000`) returns an `OtelError::Parse` (locks in the documented
     limitation so a future dependency change is noticed).
   - **Empty request.** JSON `{}` → empty `Export*ServiceResponse`, no error.

2. **Server-level (`rust/public/src/servers/otlp.rs`):** add unit tests for
   `content_type_encoding` covering `application/x-protobuf`,
   `application/json`, parameters (`application/json; charset=utf-8`), unknown types,
   and missing header. If practical, an axum router test that POSTs a JSON body and
   asserts a `200` with `Content-Type: application/json` and a `{"partialSuccess":null}` body (may require the
   ingestion service; otherwise covered by the handler-crate tests above plus the
   content-type unit tests).

3. Run `cargo test`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`, and
   `python3 ../build/rust_ci.py` from `rust/`.

## Open Questions

1. **Lenient numeric timestamps.** Is rejecting bare-number 64-bit fields acceptable for
   v1, or must we accept both string and number forms? Accepting numbers means custom
   deserializers and abandoning the crate's derives — significantly more work. Plan
   assumes strings-only is acceptable (documented limitation).

2. **EventBridge payload shape.** Does the target EventBridge API Destination emit a
   *full* `ExportLogsServiceRequest` envelope (`{"resourceLogs":[...]}`) via its input
   transformer, or a bare log record that would need server-side wrapping? This plan
   assumes the transformer produces a complete OTLP/JSON `ExportLogsServiceRequest`. If
   not, a thin EventBridge-shape adapter would be a separate follow-up (out of scope
   here).

3. **gzip + JSON.** The existing `RequestDecompressionLayer` is encoding-agnostic, so
   gzipped JSON already works transparently. No action needed — flagged only for
   confirmation.
