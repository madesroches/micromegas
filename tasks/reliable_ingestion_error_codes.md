# Reliable Ingestion Service - Error Code Plan

## Goal

Make the ingestion service return proper HTTP error codes when errors occur, enabling clients to implement reliable retry logic.

## Current State

The ingestion service handlers in `rust/public/src/servers/ingestion.rs` currently:
- Return `void` (implicit 200 OK always)
- Log errors but don't communicate them to clients
- All three endpoints (`insert_process`, `insert_stream`, `insert_block`) have this issue

```rust
// Current pattern - always returns 200 OK
pub async fn insert_process_request(...) {
    if let Err(e) = service.insert_process(body).await {
        error!("Error in insert_process_request: {:?}", e);
    }
}
```

## Why This Matters

Since the service is already **idempotent** (duplicate inserts are handled gracefully), returning proper error codes allows clients to:
- Retry on 5xx errors (server-side failures)
- Stop retrying on 4xx errors (client-side issues like malformed data)
- Implement exponential backoff strategies

## Proposed Error Codes

| Scenario | HTTP Code | When |
|----------|-----------|------|
| Success | 200 OK | Insert succeeded |
| Empty body | 400 Bad Request | Request body is empty |
| CBOR parse failure | 400 Bad Request | Malformed request data |
| Database error | 500 Internal Server Error | SQL insert failed |
| Blob storage error | 500 Internal Server Error | Object store write failed |

## Implementation Steps

### Step 1: Create IngestionError type

Add an error enum in `rust/public/src/servers/ingestion.rs` following the pattern from `http_gateway.rs`:

```rust
#[derive(Error, Debug)]
pub enum IngestionError {
    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Internal server error: {0}")]
    Internal(String),
}

impl IntoResponse for IngestionError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            IngestionError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            IngestionError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, message).into_response()
    }
}
```

### Step 2: Update WebIngestionService error handling

Modify `rust/ingestion/src/web_ingestion_service.rs` to return more specific error types that can be mapped to HTTP codes. Options:
- Return a custom error enum instead of `anyhow::Result`
- Add error categorization (client error vs server error)

### Step 3: Update handlers to return Result

Change handlers to return `Result<(), IngestionError>`:

```rust
pub async fn insert_process_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    body: bytes::Bytes,
) -> Result<(), IngestionError> {
    service.insert_process(body).await.map_err(|e| {
        if is_parse_error(&e) {
            IngestionError::BadRequest(e.to_string())
        } else {
            IngestionError::Internal(e.to_string())
        }
    })
}
```

### Step 4: Update client retry logic (optional)

The HTTP event sink in `rust/telemetry-sink/src/http_event_sink.rs` may need updates to:
- Check response status codes
- Implement retry logic for 5xx errors
- Log and skip on 4xx errors

## Files to Modify

1. `rust/public/src/servers/ingestion.rs` - Add error types, update handlers
2. `rust/ingestion/src/web_ingestion_service.rs` - Better error categorization
3. `rust/telemetry-sink/src/http_event_sink.rs` - Client-side retry handling (optional)

## Testing

1. Unit tests for error type mapping
2. Integration tests:
   - Send malformed CBOR → expect 400
   - Simulate DB failure → expect 500
   - Valid request → expect 200
