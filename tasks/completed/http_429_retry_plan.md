# Retry HTTP 429 in the Telemetry Client Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/698

## Overview
The telemetry HTTP sink treats every 4xx response as a permanent failure, so a `429 Too Many Requests` drops the payload instead of backing off and retrying. Make 429 a transient error so the existing exponential backoff handles it. The ingestion server does not rate limit today; this is defensive hardening for deployments behind a proxy or load balancer that does.

## Current State
`rust/telemetry-sink/src/http_event_sink.rs` has three near-identical response-handling `match status.as_u16()` blocks in `push_process`, `push_stream` and `push_block`. Each maps `200..=299` to success, `400..=499` to `IngestionClientError::Permanent` (logged with `warn!`), `500..=599` to `Transient` (logged with `debug!`), and anything else to `Permanent`. `Permanent` maps to `RetryError::permanent`, so `tokio_retry2::Retry::spawn` stops immediately. No code anywhere in the Rust tree handles 429.

## Design
Add a 429 arm ahead of the `400..=499` arm in each of the three blocks, returning `Transient(format!("{status}: {body}"))` and logging with `debug!` (consistent with the file's logging strategy: transient infrastructure noise stays at `debug!`, `warn!` is reserved for client bugs).

The three blocks differ only in the operation name used in log messages. Rather than add the same arm three times, extract the shared mapping into one private helper:

```text
async fn response_to_result(op: &str, response: reqwest::Response) -> Result<(), RetryError<IngestionClientError>>
```

It reads the body where needed and applies the status mapping, including the new 429 arm. The three call sites become `response_to_result("insert_process", response).await`. The status-to-class decision itself is split into a tiny pure function (`classify_status(u16) -> StatusClass` with `Success`, `Transient`, `Permanent`) so it can be unit tested without HTTP.

The existing log wording and Permanent message formats are preserved: `response_to_result` derives them from the status range (`400..=499` is `warn!` "client error" with `Permanent(body)`; any other unclassified status is `warn!` "unexpected status" with `Permanent("{status}: {body}")`), since `StatusClass` alone cannot express that distinction.

`Retry-After` is not honored: the existing backoff strategy (`RetryStrategy`, per-priority) already bounds retries, and parsing the header is speculative until a server actually sends it.

The doc comment on `IngestionClientError` and the `Transient` variant is updated to say permanent errors are "4xx except 429" (logged with `warn!`) and that transient errors include 429.

## Implementation Steps
1. In `rust/telemetry-sink/src/http_event_sink.rs`, add `classify_status` and `response_to_result`, with 429 classed as transient.
2. Replace the status `match` in `push_process`, `push_stream` and `push_block` with a call to `response_to_result`.
3. Update the `IngestionClientError` doc comments to mention 429.
4. Add unit tests (below).
5. Add a `CHANGELOG.md` entry under `Unreleased`.

## Files to Modify
- `rust/telemetry-sink/src/http_event_sink.rs`
- `CHANGELOG.md`

## Trade-offs
- **Add the arm in each of the three blocks** vs. **extract a helper**: the helper removes triplicated logic and keeps the status policy in one place, at the cost of a slightly larger diff.
- **Also retry 408 / honor `Retry-After`**: out of scope for the issue; can follow if a real deployment needs it.

## Documentation
No user-facing docs describe the client's retry policy; none to update.

## Testing Strategy
Unit tests in `http_event_sink.rs` on `classify_status`: 200 and 204 are success, 429 is transient, 400/401/404 are permanent, 500/503 are transient, and an out-of-range status (e.g. 302) is permanent. `classify_status` is a pure function, so no live service or mock server is needed.
