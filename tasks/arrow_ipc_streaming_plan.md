# Arrow IPC Streaming for Query Endpoint

GitHub Issue: https://github.com/madesroches/micromegas/issues/664

## Summary

Replace JSON serialization in the `/query` endpoint with streaming Arrow IPC to reduce latency, improve efficiency, and enable progressive rendering. The backend already receives Arrow RecordBatches from FlightSQL - currently we convert each cell to JSON which adds overhead and loses type fidelity.

## Current Architecture

```
Browser ──HTTP/JSON──► analytics-web-srv ──gRPC/Arrow Flight──► flight-sql-srv
                       (converts Arrow→JSON)                   (streams RecordBatch)
```

**Problem areas:**
- `arrow_value_to_json()` in `rust/analytics-web-srv/src/main.rs:757-842` converts every cell
- `client.query()` in `flightsql_client.rs:56-67` collects all batches before returning
- Frontend `executeSqlQuery()` in `analytics-web-app/src/lib/api.ts:239-264` waits for entire response
- All data must fit in memory on backend before sending

## Proposed Solution

Stream native Arrow IPC format followed by a JSON termination message:

```
[Arrow IPC Stream - native framing]
  Schema message
  RecordBatch 0
  RecordBatch 1
  ...
  EOS marker (0xFFFFFFFF 0x00000000)
{"type":"done"}\n
```

On error (either before streaming or mid-stream):
```
[Arrow IPC Stream - partial or empty]
  Schema message (if available)
  RecordBatch 0 (partial results)
  ...
  EOS marker
{"type":"error","code":"TIMEOUT","message":"Query exceeded time limit","retryable":true}\n
```

**Key design points:**
- Arrow IPC stream uses standard format - frontend can use `RecordBatchReader.from()` directly
- Terminating JSON message always present - explicit completion signal
- Structured error info preserved when things fail
- Partial results kept on mid-stream errors

Error codes:
| Code | Meaning | Retryable |
|------|---------|-----------|
| `INVALID_SQL` | SQL syntax or semantic error | No |
| `TIMEOUT` | Query exceeded time limit | Yes |
| `CONNECTION_FAILED` | Failed to connect to FlightSQL | Yes |
| `INTERNAL` | Unexpected backend error | No |
| `UNAUTHORIZED` | Token expired mid-stream | No |

Error scenarios:
| Scenario | Backend Response | Frontend Handling |
|----------|------------------|-------------------|
| Auth failure (before stream) | HTTP 401 | Throw, redirect to login |
| Invalid SQL | Empty IPC + error JSON | Show error, no partial data |
| FlightSQL connection failed | Empty IPC + error JSON | Show error with retry option |
| Timeout mid-query | Partial IPC + error JSON | Show error, keep partial data |
| IPC serialization failure | Partial IPC + error JSON | Show error, keep partial data |
| Backend crash | Stream ends without JSON | Detect incomplete, show error |
| Network drop | Fetch throws | Catch exception, show error |
| User cancellation | AbortController.abort() | Clean up, no error shown |

## Implementation Progress

### Phase 1: Backend Streaming Endpoint
- [ ] Create `/query-stream` endpoint with streaming response
- [ ] Use `StreamWriter` to write Arrow IPC format directly
- [ ] Stream schema and batches from FlightSQL
- [ ] Write EOS marker after all batches
- [ ] Append JSON termination message (done or error)
- [ ] Handle errors before and during streaming
- [ ] Add integration tests

### Phase 2: Frontend Arrow Dependency
- [ ] Add `apache-arrow` package to analytics-web-app
- [ ] Verify bundle size impact (~1.5MB, tree-shakeable)

### Phase 3: Frontend Stream Consumer
- [ ] Implement `streamQuery()` async generator in `lib/api.ts`
- [ ] Use `RecordBatchReader.from(response)` for Arrow IPC parsing
- [ ] Read trailing JSON for completion status
- [ ] Handle error and done messages
- [ ] Add error handling for incomplete streams

### Phase 4: Frontend Hook Integration
- [ ] Create `useStreamQuery` hook for streaming queries
- [ ] Support progressive data accumulation
- [ ] Provide loading/streaming/complete states
- [ ] Handle cancellation (AbortController)

## Technical Details

### Backend Implementation

**File: `rust/analytics-web-srv/src/stream_query.rs`**

```rust
use arrow_ipc::writer::StreamWriter;
use async_stream::stream;
use axum::body::Body;
use axum::response::IntoResponse;
use bytes::Bytes;
use futures::StreamExt;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ErrorCode {
    InvalidSql,
    Timeout,
    ConnectionFailed,
    Internal,
    Unauthorized,
}

impl ErrorCode {
    fn is_retryable(&self) -> bool {
        matches!(self, ErrorCode::Timeout | ErrorCode::ConnectionFailed)
    }
}

fn done_message() -> Bytes {
    Bytes::from("{\"type\":\"done\"}\n")
}

fn error_message(code: ErrorCode, message: &str) -> Bytes {
    let json = serde_json::json!({
        "type": "error",
        "code": code,
        "message": message,
        "retryable": code.is_retryable()
    });
    Bytes::from(format!("{}\n", json))
}

async fn stream_query_arrow(
    State(state): State<AppState>,
    Json(request): Json<SqlQueryRequest>,
) -> impl IntoResponse {
    let stream = stream! {
        // Create client and start query
        let mut client = match create_flight_client(&state).await {
            Ok(c) => c,
            Err(e) => {
                // No Arrow data to send, just error message
                yield Ok::<_, std::io::Error>(error_message(
                    ErrorCode::ConnectionFailed,
                    &e.to_string()
                ));
                return;
            }
        };

        let time_range = parse_time_range(&request);
        let mut batch_stream = match client.query_stream(request.sql.clone(), time_range).await {
            Ok(s) => s,
            Err(e) => {
                yield Ok(error_message(ErrorCode::InvalidSql, &e.to_string()));
                return;
            }
        };

        // Get schema - required for IPC stream
        let schema = match batch_stream.schema() {
            Some(s) => s,
            None => {
                yield Ok(error_message(ErrorCode::Internal, "No schema available"));
                return;
            }
        };

        // Write Arrow IPC stream to buffer, yield chunks
        let mut ipc_buffer = Vec::new();
        let mut writer = match StreamWriter::try_new(&mut ipc_buffer, &schema) {
            Ok(w) => w,
            Err(e) => {
                yield Ok(error_message(ErrorCode::Internal, &e.to_string()));
                return;
            }
        };

        let mut error_occurred: Option<(ErrorCode, String)> = None;

        while let Some(result) = batch_stream.next().await {
            match result {
                Ok(batch) => {
                    if let Err(e) = writer.write(&batch) {
                        error_occurred = Some((ErrorCode::Internal, e.to_string()));
                        break;
                    }
                    // Yield accumulated IPC data periodically
                    if ipc_buffer.len() > 64 * 1024 {
                        yield Ok(Bytes::from(std::mem::take(&mut ipc_buffer)));
                    }
                }
                Err(e) => {
                    error_occurred = Some((ErrorCode::from_flight_error(&e), e.to_string()));
                    break;
                }
            }
        }

        // Finish IPC stream (writes EOS marker)
        if let Err(e) = writer.finish() {
            if error_occurred.is_none() {
                error_occurred = Some((ErrorCode::Internal, e.to_string()));
            }
        }

        // Yield remaining IPC data
        if !ipc_buffer.is_empty() {
            yield Ok(Bytes::from(ipc_buffer));
        }

        // Yield termination message
        match error_occurred {
            Some((code, msg)) => yield Ok(error_message(code, &msg)),
            None => yield Ok(done_message()),
        }
    };

    (
        [(header::CONTENT_TYPE, "application/vnd.apache.arrow.stream")],
        Body::from_stream(stream)
    ).into_response()
}
```

### Frontend Implementation

**File: `analytics-web-app/src/lib/arrow-stream.ts`**

```typescript
import { RecordBatchReader, Table, Schema, RecordBatch } from 'apache-arrow';

type ErrorCode = 'INVALID_SQL' | 'TIMEOUT' | 'CONNECTION_FAILED' | 'INTERNAL' | 'UNAUTHORIZED';

interface TerminationMessage {
  type: 'done' | 'error';
  code?: ErrorCode;
  message?: string;
  retryable?: boolean;
}

export interface StreamError {
  code: ErrorCode;
  message: string;
  retryable: boolean;
}

export interface StreamResult {
  schema: Schema | null;
  batch: RecordBatch | null;
  done: boolean;
  error?: StreamError;
}

export async function* streamQuery(
  sql: string,
  params: Record<string, string>,
  signal?: AbortSignal
): AsyncGenerator<StreamResult> {
  const response = await fetch(`${getApiUrl()}/query-stream`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...getAuthHeaders() },
    body: JSON.stringify({ sql, ...params }),
    signal,
  });

  if (!response.ok) {
    const text = await response.text();
    throw new Error(`HTTP ${response.status}: ${text}`);
  }

  // Split response into Arrow IPC stream + trailing JSON
  const { arrowData, terminationJson } = await splitResponse(response);

  // Parse Arrow IPC using standard library
  if (arrowData.byteLength > 0) {
    const reader = await RecordBatchReader.from(arrowData);
    yield { schema: reader.schema, batch: null, done: false };

    for await (const batch of reader) {
      yield { schema: reader.schema, batch, done: false };
    }
  }

  // Parse termination message
  const termination = parseTermination(terminationJson);

  if (termination.type === 'error') {
    yield {
      schema: null,
      batch: null,
      done: true,
      error: {
        code: termination.code ?? 'INTERNAL',
        message: termination.message ?? 'Unknown error',
        retryable: termination.retryable ?? false,
      },
    };
  } else {
    yield { schema: null, batch: null, done: true };
  }
}

async function splitResponse(response: Response): Promise<{
  arrowData: Uint8Array;
  terminationJson: string;
}> {
  const buffer = await response.arrayBuffer();
  const bytes = new Uint8Array(buffer);

  // Find last newline - JSON message is after it
  let lastNewline = bytes.length - 1;
  while (lastNewline > 0 && bytes[lastNewline] !== 10) {
    lastNewline--;
  }

  // Find start of JSON line (second-to-last newline or start)
  let jsonStart = lastNewline - 1;
  while (jsonStart > 0 && bytes[jsonStart] !== 10) {
    jsonStart--;
  }
  if (bytes[jsonStart] === 10) jsonStart++;

  const arrowData = bytes.slice(0, jsonStart);
  const terminationJson = new TextDecoder().decode(bytes.slice(jsonStart));

  return { arrowData, terminationJson };
}

function parseTermination(json: string): TerminationMessage {
  try {
    return JSON.parse(json.trim());
  } catch {
    return { type: 'error', code: 'INTERNAL', message: 'Invalid termination message', retryable: true };
  }
}
```

**File: `analytics-web-app/src/hooks/useStreamQuery.ts`**

```typescript
import { useState, useCallback, useRef } from 'react';
import { Table, Schema, RecordBatch } from 'apache-arrow';
import { streamQuery, StreamError } from '@/lib/arrow-stream';

interface StreamQueryState {
  schema: Schema | null;
  batches: RecordBatch[];
  isStreaming: boolean;
  isComplete: boolean;
  error: StreamError | null;
  rowCount: number;
}

export function useStreamQuery() {
  const [state, setState] = useState<StreamQueryState>({
    schema: null,
    batches: [],
    isStreaming: false,
    isComplete: false,
    error: null,
    rowCount: 0,
  });

  const abortRef = useRef<AbortController | null>(null);

  const execute = useCallback(async (sql: string, params: Record<string, string>) => {
    abortRef.current?.abort();
    abortRef.current = new AbortController();

    setState({
      schema: null,
      batches: [],
      isStreaming: true,
      isComplete: false,
      error: null,
      rowCount: 0,
    });

    try {
      for await (const result of streamQuery(sql, params, abortRef.current.signal)) {
        if (result.error) {
          setState(s => ({ ...s, error: result.error!, isStreaming: false, isComplete: true }));
          return;
        }
        if (result.schema) {
          setState(s => ({ ...s, schema: result.schema }));
        }
        if (result.batch) {
          setState(s => ({
            ...s,
            batches: [...s.batches, result.batch!],
            rowCount: s.rowCount + result.batch!.numRows,
          }));
        }
        if (result.done) {
          setState(s => ({ ...s, isStreaming: false, isComplete: true }));
        }
      }
    } catch (e) {
      if (e instanceof Error && e.name !== 'AbortError') {
        setState(s => ({
          ...s,
          error: { code: 'INTERNAL', message: e.message, retryable: true },
          isStreaming: false,
          isComplete: true,
        }));
      }
    }
  }, []);

  const cancel = useCallback(() => {
    abortRef.current?.abort();
    setState(s => ({ ...s, isStreaming: false }));
  }, []);

  const retry = useCallback((sql: string, params: Record<string, string>) => {
    if (state.error?.retryable) {
      execute(sql, params);
    }
  }, [state.error, execute]);

  // Combine batches into single Table when needed
  const getTable = useCallback((): Table | null => {
    if (state.batches.length === 0) return null;
    return new Table(state.batches);
  }, [state.batches]);

  return { ...state, execute, cancel, retry, getTable };
}
```

## File Changes Summary

### New Files

**Backend:**
- `rust/analytics-web-srv/src/stream_query.rs` - Streaming query endpoint

**Frontend:**
- `analytics-web-app/src/lib/arrow-stream.ts` - Arrow stream parsing
- `analytics-web-app/src/hooks/useStreamQuery.ts` - Streaming query hook

### Modified Files

**Backend:**
- `rust/analytics-web-srv/src/main.rs` - Add `/query-stream` route, add module

**Frontend:**
- `analytics-web-app/package.json` - Add `apache-arrow` dependency

### Unchanged Files
- Existing `/query` endpoint - kept for backward compatibility

## Dependencies

### Backend (already available in workspace)
- `arrow-ipc` - Part of arrow crate, provides `StreamWriter`
- `async-stream = "0.3"` - For streaming

### Frontend (new)
- `apache-arrow` - Arrow JavaScript library

## Testing Strategy

### Backend Tests
1. Integration tests for `/query-stream` endpoint
2. Verify Arrow IPC stream is valid (parseable by arrow-rs)
3. Verify termination message present after IPC data
4. Error handling tests:
   - Invalid SQL returns error JSON (no Arrow data)
   - FlightSQL failure returns error JSON
   - Mid-stream error includes partial Arrow data + error JSON
5. Verify EOS marker present before JSON

### Frontend Tests
1. Unit tests for `splitResponse`:
   - Correctly separates Arrow data from JSON
   - Handles empty Arrow data (error-only response)
2. Unit tests for stream consumer:
   - Successfully parses Arrow batches
   - Correctly handles done message
   - Correctly handles error message with partial data
3. Unit tests for `useStreamQuery` hook:
   - State updates correctly during streaming
   - `getTable()` combines batches correctly
   - Cancellation works
   - Retry only for retryable errors
4. Integration tests with mock responses

## Migration Strategy

1. Deploy backend with new `/query-stream` endpoint (Phase 1)
2. Add frontend Arrow dependency (Phase 2)
3. Implement frontend stream consumer (Phase 3)
4. Create `useStreamQuery` hook (Phase 4)
5. Migrate components to use new hook
6. Keep existing `/query` endpoint for compatibility

## Benefits

- **Standard format**: Arrow IPC is a standard - any Arrow client can consume it
- **Simpler parsing**: Frontend uses `RecordBatchReader.from()` directly
- **Smaller payload**: Arrow IPC more compact than JSON for numeric data
- **Type preservation**: Timestamps, large integers stay typed
- **Less CPU**: No JSON stringify/parse for data, Arrow uses zero-copy
- **Streaming foundation**: Ready for progressive rendering

## Performance Expectations

| Metric | Current (JSON) | Proposed (Arrow IPC) |
|--------|----------------|----------------------|
| Payload size (1M rows, 10 cols) | ~100MB JSON | ~20-40MB Arrow IPC |
| Backend CPU | High (JSON serialization) | Low (pass-through) |
| Frontend CPU | High (JSON parsing) | Low (zero-copy) |
| Type fidelity | Lossy (strings) | Lossless (native types) |
| Backend memory | Full result buffered | Stream-through |

## Open Questions

1. **Batch size control**: Should we expose a parameter to control RecordBatch size from FlightSQL?
2. **Compression**: Should we add optional gzip compression via Accept-Encoding?
3. **Caching**: How does streaming affect React Query caching strategies?

## Future Improvements

### Progressive Rendering
- Update components to render batches as they arrive
- Add streaming progress indicator (row count)
- Show partial results during streaming

## References

- [Arrow IPC Format](https://arrow.apache.org/docs/format/Columnar.html#ipc-streaming-format)
- [Apache Arrow JavaScript](https://arrow.apache.org/docs/js/)
- [RecordBatchReader](https://arrow.apache.org/docs/js/classes/Arrow.dom.RecordBatchReader.html)
