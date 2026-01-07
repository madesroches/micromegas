# Reliable Ingestion Service - Error Code Plan

## Status: IMPLEMENTED

## Goal

Make the ingestion service return proper HTTP error codes when errors occur, enabling clients to implement reliable retry logic.

## Implementation Summary

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

### Error Code Mapping

| Scenario | HTTP Code | Error Type |
|----------|-----------|------------|
| Success | 200 OK | - |
| Empty body | 400 Bad Request | `IngestionError::BadRequest` |
| CBOR parse failure | 400 Bad Request | `IngestionServiceError::ParseError` |
| DateTime parse failure | 400 Bad Request | `IngestionServiceError::ParseError` |
| Database error | 500 Internal Server Error | `IngestionServiceError::DatabaseError` |
| Blob storage error | 500 Internal Server Error | `IngestionServiceError::StorageError` |

### Files Modified

- `rust/ingestion/Cargo.toml` - Added `thiserror` dependency
- `rust/ingestion/src/web_ingestion_service.rs` - Added `IngestionServiceError`, updated method signatures
- `rust/public/src/servers/ingestion.rs` - Added `IngestionError`, updated handlers

## Future Work: Client-Side Retry Logic

The HTTP event sink (`rust/telemetry-sink/src/http_event_sink.rs`) already uses `tokio_retry2` for retry with exponential backoff, but it only retries on **connection errors** - it doesn't check HTTP status codes. A 400 or 500 response is currently treated as success.

### Current Behavior

```rust
// Current: retries only on network errors, ignores status codes
client.execute(request).await.with_context(|| "executing request")
```

### Proposed Changes

Add status code checking to distinguish between:
- **2xx**: Success, no retry needed
- **4xx**: Client error (bad data), **don't retry** - data is malformed
- **5xx**: Server error, **retry** with backoff

### Implementation Details

#### Step 1: Create explicit error type for retry decisions

```rust
use tokio_retry2::RetryError;

enum IngestionClientError {
    /// Transient error - should retry (network issues, 5xx responses)
    Transient(String),
    /// Permanent error - should NOT retry (4xx responses)
    Permanent(String),
}

impl From<IngestionClientError> for RetryError<anyhow::Error> {
    fn from(err: IngestionClientError) -> Self {
        match err {
            IngestionClientError::Transient(msg) => {
                RetryError::transient(anyhow::anyhow!(msg))
            }
            IngestionClientError::Permanent(msg) => {
                RetryError::permanent(anyhow::anyhow!(msg))
            }
        }
    }
}
```

#### Step 2: Update push_process, push_stream, push_block

```rust
async fn push_block(...) -> Result<()> {
    tokio_retry2::Retry::spawn(retry_strategy, || async {
        let mut request = client.post(&url).body(encoded_block.clone()).build()
            .map_err(|e| IngestionClientError::Transient(format!("building request: {e}")))?;

        // Decorator inside retry - token refresh may fix auth errors
        if let Err(e) = decorator.decorate(&mut request).await {
            warn!("request decorator: {e:?}");
            return Err(IngestionClientError::Transient(format!("decorating request: {e}")));
        }

        let response = client.execute(request).await
            .map_err(|e| IngestionClientError::Transient(format!("network error: {e}")))?;

        match response.status().as_u16() {
            200..=299 => Ok(()),
            400..=499 => {
                let body = response.text().await.unwrap_or_default();
                warn!("client error ({}): {}", response.status(), body);
                Err(IngestionClientError::Permanent(body))
            }
            _ => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                debug!("server error ({}): {}", status, body);
                Err(IngestionClientError::Transient(format!("{status}: {body}")))
            }
        }
    }).await?;
    Ok(())
}
```

#### Step 3: Handle permanent errors gracefully

For permanent errors (4xx), the data should be logged and dropped rather than causing the whole sink to fail:

```rust
// In handle_sink_event:
match Self::push_block(...).await {
    Ok(()) => {}
    Err(e) if is_permanent_error(&e) => {
        // Log but don't propagate - data was malformed
        warn!("dropping block due to client error: {e}");
    }
    Err(e) => {
        // Transient error after all retries exhausted
        error!("failed to push block after retries: {e}");
    }
}
```

### Retry Strategy Recommendations

The current retry strategies are passed in from the caller. Recommended values:

| Data Type | Strategy | Rationale |
|-----------|----------|-----------|
| Process/Stream metadata | 5 retries, 1s base, 30s max | Critical data, worth waiting for |
| Blocks | 3 retries, 500ms base, 5s max | High volume, can tolerate some loss |

### Files to Modify

- `rust/telemetry-sink/src/http_event_sink.rs` - Add status code checking
- `rust/telemetry-sink/src/lib.rs` - Export new error type if needed

### Testing

1. Mock server returning 400 → verify no retries, data dropped
2. Mock server returning 500 then 200 → verify retry succeeds
3. Mock server returning 500 always → verify retries exhaust then drops
4. Network timeout → verify retries with backoff
