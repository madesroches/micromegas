# Reliable Ingestion Service - Error Code Plan

## Status: FULLY IMPLEMENTED

## Goal

Make the ingestion service return proper HTTP error codes when errors occur, enabling clients to implement reliable retry logic.

## Part 1: Server-Side Error Codes (Implemented)

### Changes Made

1. **Created `IngestionServiceError` in `rust/ingestion/src/web_ingestion_service.rs`**:
   - `ParseError` - for malformed input (maps to 400)
   - `DatabaseError` - for SQL failures (maps to 500)
   - `StorageError` - for blob storage failures (maps to 500)

2. **Created `IngestionError` in `rust/public/src/servers/ingestion.rs`**:
   - `BadRequest` - returns HTTP 400
   - `Internal` - returns HTTP 500
   - Implements `IntoResponse` for Axum
   - Implements `From<IngestionServiceError>` for automatic conversion

3. **Updated all handlers to return `Result<(), IngestionError>`**:
   - `insert_process_request`
   - `insert_stream_request`
   - `insert_block_request`

### Server Error Code Mapping

| Scenario | HTTP Code | Error Type |
|----------|-----------|------------|
| Success | 200 OK | - |
| Empty body | 400 Bad Request | `IngestionError::BadRequest` |
| CBOR parse failure | 400 Bad Request | `IngestionServiceError::ParseError` |
| DateTime parse failure | 400 Bad Request | `IngestionServiceError::ParseError` |
| Database error | 500 Internal Server Error | `IngestionServiceError::DatabaseError` |
| Blob storage error | 500 Internal Server Error | `IngestionServiceError::StorageError` |

### Files Modified (Server)

- `rust/ingestion/Cargo.toml` - Added `thiserror` dependency
- `rust/ingestion/src/web_ingestion_service.rs` - Added `IngestionServiceError`, updated method signatures
- `rust/public/src/servers/ingestion.rs` - Added `IngestionError`, updated handlers

## Part 2: Client-Side Retry Logic (Implemented)

### Changes Made

1. **Created `IngestionClientError` in `rust/telemetry-sink/src/http_event_sink.rs`**:
   - `Transient(String)` - should retry (network issues, 5xx responses)
   - `Permanent(String)` - should NOT retry (4xx responses, encoding errors)
   - `into_retry()` method to convert to `tokio_retry2::RetryError`

2. **Updated all push methods to check HTTP status codes**:
   - `push_process` - checks response status, returns `Result<(), IngestionClientError>`
   - `push_stream` - checks response status, returns `Result<(), IngestionClientError>`
   - `push_block` - checks response status, returns `Result<(), IngestionClientError>`

3. **Simplified `handle_sink_event`**:
   - No longer returns `Result<()>`, errors are logged internally
   - Distinguishes between transient and permanent errors in log messages

### Client Error Handling

| Response Code | Action | Retry? |
|---------------|--------|--------|
| 2xx | Success | No |
| 4xx | Log warning, mark as permanent error | No - data is malformed |
| 5xx | Log debug, mark as transient error | Yes - with exponential backoff |
| Network error | Mark as transient error | Yes - with exponential backoff |

### Implementation Pattern

```rust
let response = client.execute(request).await.map_err(|e| {
    IngestionClientError::Transient(format!("network error: {e}")).into_retry()
})?;

let status = response.status();
match status.as_u16() {
    200..=299 => Ok(()),
    400..=499 => {
        let body = response.text().await.unwrap_or_default();
        warn!("client error ({status}): {body}");
        Err(IngestionClientError::Permanent(body).into_retry())
    }
    _ => {
        let body = response.text().await.unwrap_or_default();
        debug!("server error ({status}): {body}");
        Err(IngestionClientError::Transient(format!("{status}: {body}")).into_retry())
    }
}
```

### Files Modified (Client)

- `rust/telemetry-sink/src/http_event_sink.rs` - Added `IngestionClientError`, updated all push methods

## Summary

The ingestion system now has end-to-end reliable error handling:

1. **Server** returns proper HTTP status codes (400 for client errors, 500 for server errors)
2. **Client** checks response status and:
   - Retries on 5xx/network errors (transient)
   - Drops data on 4xx errors (permanent - malformed data won't succeed on retry)
   - Uses `tokio_retry2` with explicit `Transient`/`Permanent` error classification
