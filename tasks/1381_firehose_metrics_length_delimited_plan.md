# Firehose Metrics Length-Delimited Record Decoding Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1381

## Overview

`ingest_firehose_metrics` (`rust/otel-ingestion/src/handler.rs:323`) decodes each Kinesis
Firehose record as if it were exactly one unframed `ExportMetricsServiceRequest` protobuf
message. Per AWS's CloudWatch Metric Streams OpenTelemetry 1.0.0 output format, a record
actually carries **one or more** length-delimited messages: each is prefixed with an
`UnsignedVarInt32` byte length, back-to-back in the record body. Once a real delivery
batches more than a trivial amount of data (the common case), the leading length-prefix
byte of the second message gets misread as a protobuf tag by a fresh `Message::decode`
call, and the batch fails to ingest with errors like `invalid wire type value: 7` or
`unexpected end group tag`.

The fix unpacks each record into its constituent length-delimited messages before handing
them to the existing split/write pipeline — no new identity, block, split, or write logic,
matching the "unwrap the envelope, then reuse everything" principle the Firehose metrics
route was built on (`tasks/completed/1299_firehose_otlp_metrics_ingestion_plan.md`).

## Current State

- `rust/otel-ingestion/src/handler.rs:141-157` — `ingest_metrics(service, body, encoding)`
  parses `body` as a single `ExportMetricsServiceRequest` via the generic `parse` helper
  (`handler.rs:35-50`), which for `Encoding::Protobuf` calls `M::decode(body)` — a plain,
  single-message decode with no length framing.
- `rust/otel-ingestion/src/handler.rs:320-331` — `ingest_firehose_metrics(service, records)`
  loops over each Firehose record's raw bytes and feeds the **whole record** into
  `ingest_metrics` with `Encoding::Protobuf`, i.e. one `M::decode` call per record. This is
  the exact assumption issue #1381 reports as wrong.
- `rust/otel-ingestion/src/handler.rs:295-318` — `decode_firehose_envelope` only unwraps the
  Firehose JSON envelope (base64-decodes each record's `data`); it has no protobuf framing
  knowledge and needs no change — the bug is entirely downstream, in
  `ingest_firehose_metrics`.
- `rust/public/src/servers/firehose_cloudwatch_logs.rs` — the CloudWatch **Logs** Firehose
  route uses a different, non-OTLP, JSON-based record format (`cloudwatch_logs.rs`); it is
  not protobuf-framed and is unaffected by this bug.
- No length-delimited multi-message decode pattern exists anywhere else in the codebase
  (`grep` for `decode_length_delimited` / `encode_length_delimited` across `rust/` returns
  nothing outside the unrelated `WireType::LengthDelimited` constant in
  `rust/perfetto/src/streaming_writer.rs`). `prost` (workspace dependency, `rust/Cargo.toml`,
  version `0.14`) and `bytes` (`1.11.1`) are already dependencies of `otel-ingestion`
  (`rust/otel-ingestion/Cargo.toml`), and `prost::Message` provides
  `decode_length_delimited(buf: impl Buf)` — reads one varint length prefix, decodes exactly
  that many bytes, and advances the buffer past it — which is exactly the primitive needed
  here. `bytes::Buf` is implemented for `&[u8]`, so no allocation is required to iterate.
- Existing tests build Firehose records with a single unframed message per record
  (`req.encode_to_vec()` in Rust, `req.SerializeToString()` in Python) — accidentally
  correct only because a length-delimited stream of exactly one message, decoded without
  framing awareness, happens to look the same as an unframed single message *up to* the
  point framing starts mattering (multiple messages, or the length-prefix byte pattern
  colliding with a valid start-of-message tag). None of the existing tests build a record
  containing two-or-more concatenated length-delimited messages, so this exact bug is
  currently untested:
  - `rust/otel-ingestion/tests/firehose_tests.rs` — pure envelope-decode tests only; not
    affected by this change (see Design).
  - `rust/public/tests/firehose_tests.rs:155-237` —
    `full_multi_record_ingest_succeeds_against_a_live_stack` (`#[ignore]`d, live-stack only)
    builds each record via `req.encode_to_vec()` (unframed).
  - `python/micromegas/tests/test_otlp_e2e.py:641-736` — `_firehose_envelope` plus
    `test_firehose_metrics_e2e` / `test_firehose_multi_record_e2e` build records via
    `req.SerializeToString()` (unframed); "multi record" here means multiple *Firehose
    records* in one batch, not multiple protobuf messages packed into a single record.

## Design

### New helper: decode a record as a stream of length-delimited messages

Add to `rust/otel-ingestion/src/handler.rs`, next to `parse`:

```rust
use bytes::Buf;

/// Decode a Firehose record's bytes as zero-or-more back-to-back length-delimited
/// protobuf messages: CloudWatch Metric Streams' OpenTelemetry 1.0.0 output format packs
/// one-or-more `[varint32 length][message bytes]` entries per record, not a single
/// unframed message (see AWS's CloudWatch metric streams OpenTelemetry format docs).
fn decode_length_delimited_messages<M: Message + Default>(
    mut buf: &[u8],
    signal: Signal,
) -> Result<Vec<M>, OtelError> {
    let mut messages = Vec::new();
    while buf.has_remaining() {
        let message = M::decode_length_delimited(&mut buf).map_err(|e| OtelError::Parse {
            signal,
            message: format!(
                "decoding {} (length-delimited protobuf): {e}",
                signal.as_str()
            ),
        })?;
        messages.push(message);
    }
    Ok(messages)
}
```

A zero-length record decodes to an empty `Vec` (loop body never runs), matching
`ingest_metrics`'s existing no-op behavior for an empty request. Malformed framing (a bad
length prefix, or trailing bytes that don't form a complete message) surfaces as
`OtelError::Parse`, mapped by the caller to a non-200 Firehose response — same
retry-on-failure contract as every other decode error on this path.

### Factor the parsed-request pipeline out of `ingest_metrics`

`ingest_metrics` currently couples "decode bytes" with "split + write". Split those so
`ingest_firehose_metrics` can reuse the split/write half per decoded message without
re-deriving it:

```rust
async fn ingest_parsed_metrics(
    service: &WebIngestionService,
    req: ExportMetricsServiceRequest,
) -> Result<(), OtelError> {
    if req.resource_metrics.is_empty() {
        return Ok(());
    }
    let blocks = split_metrics(req).map_err(|e| OtelError::Parse {
        signal: Signal::Metrics,
        message: format!("split_metrics: {e}"),
    })?;
    write_blocks(service, Signal::Metrics, blocks).await?;
    Ok(())
}

pub async fn ingest_metrics(
    service: Arc<WebIngestionService>,
    body: bytes::Bytes,
    encoding: Encoding,
) -> Result<ExportMetricsServiceResponse, OtelError> {
    let req: ExportMetricsServiceRequest = parse(&body, Signal::Metrics, encoding)?;
    ingest_parsed_metrics(&service, req).await?;
    Ok(ExportMetricsServiceResponse::default())
}
```

`ingest_metrics`'s public signature and behavior are unchanged — this is a pure internal
refactor (the native `/v1/metrics` route, which is never length-delimited-framed on the
wire, keeps using plain `M::decode` via `parse`).

### Fix `ingest_firehose_metrics`

```rust
pub async fn ingest_firehose_metrics(
    service: Arc<WebIngestionService>,
    records: Vec<Vec<u8>>,
) -> Result<(), OtelError> {
    for rec in records {
        let messages: Vec<ExportMetricsServiceRequest> =
            decode_length_delimited_messages(&rec, Signal::Metrics)?;
        for req in messages {
            ingest_parsed_metrics(&service, req).await?;
        }
    }
    Ok(())
}
```

Each Firehose record is now unpacked into however many `ExportMetricsServiceRequest`
messages it actually contains (one, in the simple case; many, in the batched case) before
any of them is split or written — the case this issue reports as broken.

## Implementation Steps

1. **`rust/otel-ingestion/src/handler.rs`** — add `use bytes::Buf;`, add
   `decode_length_delimited_messages`, extract `ingest_parsed_metrics` from `ingest_metrics`,
   and rewrite `ingest_firehose_metrics` to loop record → messages → `ingest_parsed_metrics`
   as shown above.
2. **`rust/otel-ingestion/tests/firehose_tests.rs`** — add unit coverage for the new decode
   path (see Testing). The existing envelope-decode tests are unaffected (they exercise
   `decode_firehose_envelope`, which has no protobuf framing knowledge), so leave them as is.
3. **`rust/public/tests/firehose_tests.rs`** — update
   `full_multi_record_ingest_succeeds_against_a_live_stack`'s `make_record` helper to encode
   each message with `encode_length_delimited_to_vec()` instead of `encode_to_vec()` (real
   Firehose records are always length-delimited-framed, even for a single message), and add
   a case that packs two messages into one record.
4. **`python/micromegas/tests/test_otlp_e2e.py`** — add a small varint-length-prefix helper
   and use it to build every Firehose metrics record's bytes (both `test_firehose_metrics_e2e`
   and `test_firehose_multi_record_e2e` currently pass raw `SerializeToString()` output,
   which is not how real Firehose records are framed); add a new
   `test_firehose_multi_message_record_e2e` that packs two distinct
   `ExportMetricsServiceRequest` messages into a single Firehose record and asserts both
   land in `measures`.
5. **`mkdocs/docs/otlp/index.md:399-403`** — correct the description from "delivers each
   record as an OTLP `ExportMetricsServiceRequest` protobuf" to reflect that a record carries
   one-or-more length-delimited messages.
6. **CI** — `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
   `python3 build/rust_ci.py`.

## Files to Modify

- `rust/otel-ingestion/src/handler.rs` — decode fix (core change).
- `rust/otel-ingestion/tests/firehose_tests.rs` — new unit tests for the length-delimited
  decode path.
- `rust/public/tests/firehose_tests.rs` — length-delimited-frame the live-stack test's
  fixture; add a packed-multi-message-record case.
- `python/micromegas/tests/test_otlp_e2e.py` — length-delimited-frame existing Firehose
  metrics record fixtures; add a multi-message-per-record e2e test.
- `mkdocs/docs/otlp/index.md` — correct the record-framing description.

## Trade-offs

- **Loop `decode_length_delimited` over a slice vs. a bespoke varint-length parser.** `prost`
  already implements the varint-length-prefix + bounded-decode primitive this format needs
  (`Message::decode_length_delimited`); reusing it avoids hand-rolling varint parsing for a
  format `prost` already understands correctly, and keeps the fix's own logic to "loop until
  the buffer is empty."
- **Extract `ingest_parsed_metrics` vs. leaving `ingest_metrics` as the sole entry point and
  re-decoding.** Splitting decode from split/write means `ingest_firehose_metrics` calls
  split/write directly per already-decoded message, instead of re-serializing each decoded
  message back to bytes just to call `ingest_metrics` again (which would also incorrectly
  re-apply single-message decode semantics to already-decoded data). The refactor is
  internal-only; `ingest_metrics`'s public behavior for the native `/v1/metrics` route is
  unchanged.

## Documentation

`mkdocs/docs/otlp/index.md:399-403` — update the CloudWatch Metric Streams section to state
that a delivered record contains one-or-more length-delimited `ExportMetricsServiceRequest`
messages (not a single unframed message), and that the Firehose route decodes all of them.

## Testing Strategy

- **Unit (`rust/otel-ingestion/tests/firehose_tests.rs`, no DB):**
  - A record containing a **single** length-delimited message decodes to one
    `ExportMetricsServiceRequest`, byte-identical to the source.
  - A record containing **two concatenated** length-delimited messages decodes to two
    messages, in order — this is the exact scenario issue #1381 reports as broken.
  - A **zero-length** record decodes to zero messages (no-op, matching empty-`records`
    behavior).
  - Malformed framing (e.g. a length prefix longer than the remaining bytes) → parse error.
- **HTTP (`rust/public/tests/firehose_tests.rs`, `#[ignore]`d, live stack):** update
  `full_multi_record_ingest_succeeds_against_a_live_stack` to length-delimited-frame its
  fixture records and add a case with two messages packed into a single record, asserting
  both land in `measures`.
- **Integration (Python e2e, `test_otlp_e2e.py`):** length-delimited-frame the existing
  Firehose metrics fixtures; add `test_firehose_multi_message_record_e2e` packing two
  `ExportMetricsServiceRequest` messages into one record and asserting both land.
- **CI:** `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
  `python3 build/rust_ci.py`.

## Open Questions

None blocking — the fix is a self-contained decode-path correction with no API or schema
change.
