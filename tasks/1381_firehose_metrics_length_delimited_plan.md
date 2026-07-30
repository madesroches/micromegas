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

### New helper: decode one length-delimited message at a time

Add to `rust/otel-ingestion/src/handler.rs`, next to `parse`. This decodes and returns a
single message per call instead of collecting a whole record into a `Vec` first — the
caller (`ingest_firehose_metrics`, below) writes each message immediately after it decodes,
so a malformed message later in a record can't retroactively discard already-decoded,
already-written messages that precede it:

```rust
use bytes::Buf;

/// Decode the next length-delimited protobuf message from `buf`, advancing `buf` past it.
/// Returns `Ok(None)` once `buf` is exhausted (no more messages in this record).
/// CloudWatch Metric Streams' OpenTelemetry 1.0.0 output format packs one-or-more
/// `[varint32 length][message bytes]` entries per record, not a single unframed message
/// (see AWS's CloudWatch metric streams OpenTelemetry format docs).
fn decode_next_length_delimited<M: Message + Default>(
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
```

A zero-length (or fully-consumed) record returns `Ok(None)` on the first call, matching
`ingest_metrics`'s existing no-op behavior for an empty request. Malformed framing (a bad
length prefix, or trailing bytes that don't form a complete message) surfaces as
`OtelError::Parse` from that call — mapped by the caller to a non-200 Firehose response,
same retry-on-failure contract as every other decode error on this path — but only after
every message earlier in the same record has already been decoded and written by the
caller's loop.

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
        let mut buf: &[u8] = &rec;
        while let Some(req) = decode_next_length_delimited::<ExportMetricsServiceRequest>(
            &mut buf,
            Signal::Metrics,
        )? {
            ingest_parsed_metrics(&service, req).await?;
        }
    }
    Ok(())
}
```

Each Firehose record's messages are now decoded and written one at a time: as soon as a
message decodes, it's split and written via `ingest_parsed_metrics` before the next
message's length prefix is even read. If message *N* in a record is malformed, messages
`1..N-1` in that same record — already decoded and already written — are unaffected; only
the remainder of that record is lost, preserving the partial-batch-retry-safety guarantee
this route relies on (content-addressed `block_id` dedup, per
`tasks/completed/1299_firehose_otlp_metrics_ingestion_plan.md`'s "Idempotency & partial-batch
retries" section).

## Implementation Steps

1. **`rust/otel-ingestion/src/handler.rs`** — add `use bytes::Buf;`, add
   `decode_next_length_delimited`, extract `ingest_parsed_metrics` from `ingest_metrics`,
   and rewrite `ingest_firehose_metrics` to loop record → decode one message → immediately
   call `ingest_parsed_metrics` → repeat, as shown above. Also update
   `ingest_firehose_metrics`'s doc comment (currently "Reuses `ingest_metrics` per
   record...") to describe the length-delimited, multi-message-per-record decode-and-write
   path via `ingest_parsed_metrics` instead.
2. **`rust/public/src/servers/firehose.rs`** — update the module doc comment (lines ~6-16),
   which currently states each delivered record is "an OTLP `ExportMetricsServiceRequest`
   protobuf" (singular) and that "no new identity, block, split, or write logic" is needed
   referencing reuse of `handler::ingest_metrics`; correct it to describe a record as
   one-or-more length-delimited messages, decoded via `handler::ingest_firehose_metrics`.
4. **`rust/otel-ingestion/tests/firehose_tests.rs`** — add unit coverage for the new decode
   path (see Testing). The existing envelope-decode tests are unaffected (they exercise
   `decode_firehose_envelope`, which has no protobuf framing knowledge), so leave them as is.
5. **`rust/public/tests/firehose_tests.rs`** — update
   `full_multi_record_ingest_succeeds_against_a_live_stack`'s `make_record` helper to encode
   each message with `encode_length_delimited_to_vec()` instead of `encode_to_vec()` (real
   Firehose records are always length-delimited-framed, even for a single message), and add
   a case that packs two messages into one record.
6. **`python/micromegas/tests/test_otlp_e2e.py`** — add a small varint-length-prefix helper
   and use it to build every Firehose metrics record's bytes (both `test_firehose_metrics_e2e`
   and `test_firehose_multi_record_e2e` currently pass raw `SerializeToString()` output,
   which is not how real Firehose records are framed); add a new
   `test_firehose_multi_message_record_e2e` that packs two distinct
   `ExportMetricsServiceRequest` messages into a single Firehose record and asserts both
   land in `measures`.
7. **`mkdocs/docs/otlp/index.md:399-403`** — correct the description from "delivers each
   record as an OTLP `ExportMetricsServiceRequest` protobuf" to reflect that a record carries
   one-or-more length-delimited messages. Also fix the "Buffering hints" bullet at
   `mkdocs/docs/otlp/index.md:428-430`, which independently asserts "one JSON record per
   underlying Metric Stream record" — the same false 1:1 assumption in different words —
   to instead note that a JSON record's data may itself pack multiple length-delimited
   messages.
8. **CI** — `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
   `python3 build/rust_ci.py`.

## Files to Modify

- `rust/otel-ingestion/src/handler.rs` — decode fix (core change), plus updating the
  `ingest_firehose_metrics` doc comment to drop the now-false "Reuses `ingest_metrics` per
  record" description.
- `rust/public/src/servers/firehose.rs` — correct the module doc comment's
  single-message-per-record assumption and its "no new identity, block, split, or write
  logic" / `handler::ingest_metrics`-reuse framing.
- `rust/otel-ingestion/tests/firehose_tests.rs` — new unit tests for the length-delimited
  decode path.
- `rust/public/tests/firehose_tests.rs` — length-delimited-frame the live-stack test's
  fixture; add a packed-multi-message-record case.
- `python/micromegas/tests/test_otlp_e2e.py` — length-delimited-frame existing Firehose
  metrics record fixtures; add a multi-message-per-record e2e test.
- `mkdocs/docs/otlp/index.md` — correct the record-framing description (lines 399-403) and
  the "Buffering hints" bullet's matching single-message-per-record assumption (lines
  428-430).

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

`mkdocs/docs/otlp/index.md:428-430` — the "Buffering hints" bullet currently claims "one
JSON record per underlying Metric Stream record", the same single-message assumption in
different words; update it to clarify that a buffered HTTP POST can carry multiple JSON
records, and each JSON record's data may itself pack multiple length-delimited OTLP
messages.

`rust/public/src/servers/firehose.rs` (module doc, lines ~6-16) and
`rust/otel-ingestion/src/handler.rs` (`ingest_firehose_metrics`'s doc comment) both currently
describe the old single-message-per-record, reuse-`ingest_metrics` behavior; both need their
prose updated to describe the length-delimited, multi-message-per-record decode path via
`ingest_parsed_metrics` instead.

## Testing Strategy

- **Unit (`rust/otel-ingestion/tests/firehose_tests.rs`, no DB):**
  - A record containing a **single** length-delimited message: the first call to
    `decode_next_length_delimited` returns it (byte-identical to the source), and the
    second call returns `Ok(None)`.
  - A record containing **two concatenated** length-delimited messages: two successive
    calls each return one message, in order, before a third call returns `Ok(None)` — this
    is the exact scenario issue #1381 reports as broken.
  - A **zero-length** record: the first call returns `Ok(None)` (no-op, matching
    empty-`records` behavior).
  - A record with a **valid message followed by malformed framing**: the first call
    returns the valid message successfully, and only the second call errors —
    demonstrating that a later malformed message can't retroactively discard an earlier,
    validly-decoded one.
  - Malformed framing on the first message (e.g. a length prefix longer than the remaining
    bytes) → parse error on the first call.
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
