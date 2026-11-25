# Query Audit Output Bytes Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1803

## Overview

The FlightSQL query audit record (`QueryAuditRecord`, log target `flightsql_query_audit`) reports
`output_rows` and `bytes_scanned`, but nothing about how much data went back to the client. Bulk
export callers are judged by their egress, not by what they scan, and today that can only be
estimated. This plan adds `output_bytes` (Arrow IPC wire size of every `FlightData` message sent)
and `output_messages` (number of `FlightData` messages sent) to the record, counted in the stream
wrapper that already sees every message, and a `query_output_bytes` metric next to
`query_peak_memory_bytes`.

## Current State

- `rust/public/src/servers/query_audit.rs` defines `QueryAuditRecord` (line ~80) and
  `ScanMetrics`/`aggregate_scan_metrics`. The record's last fields are `sql_hash`, the
  scan-estimate fields, `admission_cost` and `queue_wait_ms`.
- `rust/public/src/servers/flight_sql_service_impl.rs`:
  - `QueryAuditState` (line ~316) holds per-query audit data; `emit(&self, status, error,
    error_class)` (line ~399) builds the record and emits `imetric!("query_peak_memory_bytes", ...)`.
    It is called on setup failures (`fail`/`fail_with_class`), from `QueueWaitGuard::drop`
    (cancelled while queued), and from `CompletionTrackedStream` on `ok`, `error`, and drop
    (`incomplete`).
  - `CompletionTrackedStream<S>` (line ~478) wraps a
    `Stream<Item = Result<FlightData, Status>>` and owns `audit: Option<QueryAuditState>`, taken on
    the terminal arm. `poll_next` passes `Ok` items through untouched today.
  - The state is built once in `execute_query` (line ~827).
  - Two `CompletionTrackedStream::new` call sites: `execute_query` (line ~1089, fed by either of the
    two `FlightDataEncoderBuilder` branches at ~1050/~1054) and `execute_view_ddl` (line ~1270).
    These are the "three encoder call sites" the issue mentions; all of them go through
    `CompletionTrackedStream`, so counting there covers them all.
- Tests: `rust/public/tests/query_audit_tests.rs` covers record serialization (`full_record`,
  `query_audit_record_omits_absent_optionals`) and `aggregate_scan_metrics`.
  `CompletionTrackedStream` and `QueryAuditState` are private and have no unit tests.
- Docs: `mkdocs/docs/query-guide/query-audit-log.md` has the field table (line ~228) and example
  queries.

## Design

### Counter type (`query_audit.rs`)

Add a small public value type so the counting rule can be unit tested from the integration test
crate:

```rust
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OutputStats {
    pub bytes: u64,
    pub messages: u64,
}

impl OutputStats {
    pub fn record(&mut self, data: &arrow_flight::FlightData) {
        self.bytes += (data.data_header.len() + data.data_body.len()) as u64;
        self.messages += 1;
    }
}
```

`output_bytes` is the Arrow IPC size (`data_header` + `data_body`), before gRPC/HTTP2 framing and
transport compression. `app_metadata` and `flight_descriptor` are not counted: the encoder leaves
them empty for query results, and the issue defines the metric as header + body.

### Record fields (`QueryAuditRecord`)

Appended **last**, after `queue_wait_ms`, so positional/JSON consumers are unaffected:

```rust
/// Arrow IPC bytes sent to the client (`data_header` + `data_body` of every `FlightData`
/// message), before gRPC framing and transport compression. 0 when no message was sent.
pub output_bytes: u64,
/// Number of `FlightData` messages sent, including the schema message.
pub output_messages: u64,
```

Both always present (plain `u64`, no `skip_serializing_if`), matching `bytes_scanned`: 0 is a real
answer for a setup failure.

### State and stream wiring (`flight_sql_service_impl.rs`)

- `QueryAuditState` gains `output: OutputStats`, initialised to `OutputStats::default()` in
  `execute_query`.
- `CompletionTrackedStream::poll_next`: on `Poll::Ready(Some(Ok(ref data)))`, call
  `state.output.record(data)` on `self.audit.as_mut()` before returning the item. The record is
  emitted later from the same state, so every terminal path (`ok`, `error`, `incomplete` on drop)
  reports what was actually yielded. A message counted here was handed to tonic; on `incomplete`
  it may not have reached the client, which the docs note.
- `emit()` copies `self.output.bytes` / `self.output.messages` into the record and emits
  `imetric!("query_output_bytes", "bytes", self.output.bytes)` next to `query_peak_memory_bytes`.
  Setup failures and `QueueWaitGuard` naturally report 0.

## Implementation Steps

1. `rust/public/src/servers/query_audit.rs`: add `OutputStats` with `record`; append
   `output_bytes` and `output_messages` to `QueryAuditRecord` with doc comments.
2. `rust/public/src/servers/flight_sql_service_impl.rs`: add `output: OutputStats` to
   `QueryAuditState` and its initialiser; record in `CompletionTrackedStream::poll_next` on `Ok`;
   populate the record and emit `query_output_bytes` in `emit()`.
3. `rust/public/tests/query_audit_tests.rs`: add the new fields to both record literals and the
   new tests below.
4. `mkdocs/docs/query-guide/query-audit-log.md`: add both fields to the table, an example "egress
   by caller" query, and mention `output_bytes` in the `incomplete` note.
5. `CHANGELOG.md`: Unreleased entry (audit record gains two fields; new `query_output_bytes`
   metric; no SQL changes; Rust API gains `OutputStats`).

## Files to Modify

- `rust/public/src/servers/query_audit.rs`
- `rust/public/src/servers/flight_sql_service_impl.rs`
- `rust/public/tests/query_audit_tests.rs`
- `mkdocs/docs/query-guide/query-audit-log.md`
- `CHANGELOG.md`

## Trade-offs

- **`output_messages` instead of the issue's `output_batches`.** The issue defines the field as
  "the number of `FlightData` messages", which includes the schema message (and any dictionary
  messages); calling that "batches" would overcount record batches by at least one and mislead the
  tiny-batch analysis it is meant for. Naming it for what it counts avoids a correction later.
  Counting only record-batch messages would require decoding the IPC header, which is more than a
  counter increment for little gain.
- **Counter on `QueryAuditState` rather than on `CompletionTrackedStream`.** `emit()` already reads
  everything from the state, and the state is what every terminal path holds, so storing the
  counts there needs no new parameter on `emit` and keeps setup-failure paths at 0 for free.
- **A public `OutputStats` type rather than inlining the arithmetic.** It gives the integration
  test crate something to call with constructed `FlightData`, since the stream wrapper and state
  are private and building a `QueryAuditState` needs a memory pool and attribution.

## Decisions

## Documentation

- `mkdocs/docs/query-guide/query-audit-log.md`: field table rows for `output_bytes` and
  `output_messages`, an egress-per-caller example query (`sum(jsonb_as_i64(jsonb_get(j,
  'output_bytes')))` grouped by `email`/`service_account_name`), and the `incomplete` note.

## Testing Strategy

Unit tests in `rust/public/tests/query_audit_tests.rs` (no DB):

- `OutputStats::default()` is zero (the value a setup failure reports).
- `record` over a few constructed `FlightData` values (empty body schema-like message, messages
  with header and body) sums `data_header.len() + data_body.len()` and counts messages; fields
  outside header/body (`app_metadata`) are not counted.
- Encoding a real non-empty `RecordBatch` through `FlightDataEncoderBuilder` and recording every
  message gives `bytes > 0` and `messages >= 2` (schema + batch).
- Serialization: `full_record` sets non-zero values and asserts `output_bytes`/`output_messages`
  appear; the "absent optionals" record sets 0 and asserts both keys are still present with 0.

The one-line wiring in `poll_next`/`emit` is covered by manual verification below.

## Manual Verification

Checks the private stream wiring, which needs a running server; a regression would be obvious
the first time anyone reads the field.

1. `python3 local_test_env/ai_scripts/start_services.py`
2. `micromegas-query "SELECT * FROM log_entries LIMIT 100" --begin 1h` and
   `micromegas-query "SELECT nonexistent_column FROM log_entries" --begin 1h`
3. `micromegas-query "SELECT msg FROM log_entries WHERE target = 'flightsql_query_audit' ORDER BY time DESC LIMIT 2" --begin 10m`
   — the successful query's record has `output_bytes > 0` and `output_messages >= 2`; the failed
   one has `output_bytes: 0`, `output_messages: 0`.
4. `python3 local_test_env/ai_scripts/stop_services.py`

## Open Questions
