# Arrow IPC Streaming for Query Endpoint

GitHub Issue: https://github.com/madesroches/micromegas/issues/664

## Summary

Replace JSON serialization in the `/query` endpoint with streaming Arrow IPC to reduce latency, improve efficiency, and enable progressive rendering. The backend passes through FlightData from FlightSQL with minimal transformation (just adding JSON framing) - no deserialization/reserialization of Arrow data.

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

Use a JSON-framed protocol that enables true streaming:

```
{"type":"schema","size":512}\n
[512 bytes of raw Arrow IPC schema message]
{"type":"batch","size":4096}\n
[4096 bytes of raw Arrow IPC batch message]
{"type":"batch","size":2048}\n
[2048 bytes of raw Arrow IPC batch message]
{"type":"done"}\n
```

On error:
```
{"type":"schema","size":512}\n
[512 bytes of schema]
{"type":"batch","size":1024}\n
[1024 bytes - partial batch message]
{"type":"error","code":"TIMEOUT","message":"Query exceeded time limit"}\n
```

**Key design points:**
- Each frame starts with a JSON line (newline-terminated)
- All binary data uses consistent size-prefixed framing (no base64)
- Schema sent once; batch frames contain only raw batch bytes (no schema repetition)
- Frontend uses `RecordBatchReader.from()` with a ReadableStream adapter
- Arrow reader parses schema once, maintains dictionary state automatically
- True streaming: frontend processes each batch as it arrives
- No buffering required to find message boundaries
- Errors can occur at any point, partial results preserved
- Dictionary state maintained across batches for correct dictionary-encoded columns

### Frame Types

| Type | Fields | Followed By |
|------|--------|-------------|
| `schema` | `size` (byte count) | Raw schema IPC bytes (no EOS) |
| `batch` | `size` (byte count) | Raw batch IPC bytes (no schema, no EOS) |
| `done` | None | Nothing (stream complete) |
| `error` | `code`, `message` | Nothing (stream complete) |

**Note:** Frontend creates a ReadableStream adapter that yields raw IPC bytes to `RecordBatchReader`, which handles schema caching and dictionary state automatically.

### Error Codes

| Code | Meaning | Retryable |
|------|---------|-----------|
| `INVALID_SQL` | SQL syntax or semantic error | No |
| `TIMEOUT` | Query exceeded time limit | Yes |
| `CONNECTION_FAILED` | Failed to connect to FlightSQL | Yes |
| `INTERNAL` | Unexpected backend error | No |

### Error Scenarios

| Scenario | Response | Frontend Handling |
|----------|----------|-------------------|
| Auth failure | HTTP 401 | Redirect to login |
| Invalid SQL | Schema + error frame | Show error, no data |
| FlightSQL down | Error frame (no schema) | Show error with retry |
| Timeout mid-query | Partial batches + error | Show error, keep partial |
| Success | Schema + batches + done | Display complete results |
| Network drop | Fetch throws | Catch, show error |
| User cancel | AbortController | Clean up, no error |

## Implementation Progress

### Phase 1: Backend Streaming Endpoint
- [ ] Add `query_flight_data()` method to FlightSQL client (returns raw FlightData stream)
- [ ] Create `/query-stream` endpoint with streaming response
- [ ] Convert FlightData to IPC bytes (passthrough, no deserialization)
- [ ] Send JSON frame headers with size prefixes
- [ ] Handle errors before and during streaming
- [ ] Add integration tests

### Phase 2: Frontend Arrow Dependency
- [ ] Add `apache-arrow` package to analytics-web-app
- [ ] Verify bundle size impact (~1.5MB, tree-shakeable)

### Phase 3: Frontend Stream Consumer
- [ ] Implement framed stream reader in `lib/arrow-stream.ts`
- [ ] Parse JSON headers, read binary payloads by size
- [ ] Use `RecordBatchReader` for IPC deserialization
- [ ] Yield batches as async generator
- [ ] Handle all error scenarios

### Phase 4: Frontend Hook Integration
- [ ] Create `useStreamQuery` hook
- [ ] Support progressive data accumulation
- [ ] Provide loading/streaming/complete states
- [ ] Handle cancellation (AbortController)

## Technical Details

### Backend Implementation

**File: `rust/analytics-web-srv/src/stream_query.rs`**

```rust
use arrow_flight::FlightData;
use arrow_ipc::CONTINUATION_MARKER;
use async_stream::stream;
use axum::body::Body;
use axum::response::IntoResponse;
use bytes::Bytes;
use futures::StreamExt;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidSql,
    Timeout,
    ConnectionFailed,
    Internal,
}

/// Schema and batch frames use identical structure - size-prefixed binary
#[derive(Serialize)]
struct DataHeader {
    #[serde(rename = "type")]
    frame_type: &'static str,
    size: usize,
}

#[derive(Serialize)]
struct DoneFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
}

#[derive(Serialize)]
struct ErrorFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    code: ErrorCode,
    message: String,
}

/// Calculates total IPC message size without allocating the full buffer.
///
/// IPC streaming format per message:
/// - Continuation marker (0xFFFFFFFF): 4 bytes
/// - Metadata length (unpadded, little-endian i32): 4 bytes
/// - Metadata flatbuffer: N bytes
/// - Padding to 8-byte alignment: P bytes where (N + P) % 8 == 0
/// - Body buffers: M bytes
fn ipc_message_size(flight_data: &FlightData) -> usize {
    let header_len = flight_data.data_header.len();
    let padding = (8 - (header_len % 8)) % 8;
    4 + 4 + header_len + padding + flight_data.data_body.len()
}

/// Creates the IPC metadata portion (continuation marker, length, header, padding).
/// This is small (~100-500 bytes) so allocation is fine. The body is yielded
/// separately to avoid copying the large payload data.
///
/// Note: The length field contains the UNPADDED metadata size. The IPC reader
/// calculates padding separately: it reads `length` bytes, then skips to the
/// next 8-byte boundary before reading the body.
fn ipc_metadata_bytes(flight_data: &FlightData) -> Bytes {
    let header = &flight_data.data_header;
    let header_len = header.len();
    let padding = (8 - (header_len % 8)) % 8;

    let mut buffer = Vec::with_capacity(4 + 4 + header_len + padding);
    buffer.extend_from_slice(&CONTINUATION_MARKER);
    buffer.extend_from_slice(&(header_len as i32).to_le_bytes());
    buffer.extend_from_slice(header);
    buffer.extend(std::iter::repeat(0u8).take(padding));
    Bytes::from(buffer)
}


fn json_line<T: Serialize>(value: &T) -> Bytes {
    let mut json = serde_json::to_string(value).expect("serialization failed");
    json.push('\n');
    Bytes::from(json)
}

pub async fn stream_query_handler(
    State(state): State<AppState>,
    claims: Claims,
    Json(request): Json<SqlQueryRequest>,
) -> impl IntoResponse {
    let stream = stream! {
        // Create FlightSQL client
        let mut client = match create_flight_client(&state, &claims).await {
            Ok(c) => c,
            Err(e) => {
                yield Ok::<_, std::io::Error>(json_line(&ErrorFrame {
                    frame_type: "error",
                    code: ErrorCode::ConnectionFailed,
                    message: e.to_string(),
                }));
                return;
            }
        };

        // Start streaming query - get raw FlightData stream
        let time_range = parse_time_range(&request);
        let mut flight_stream = match client.query_flight_data(&request.sql, time_range).await {
            Ok(s) => s,
            Err(e) => {
                yield Ok(json_line(&ErrorFrame {
                    frame_type: "error",
                    code: ErrorCode::InvalidSql,
                    message: e.to_string(),
                }));
                return;
            }
        };

        // FlightSQL protocol: first FlightData contains Schema message (empty body).
        // We label it "schema" for human readability, but the frontend's
        // RecordBatchReader handles message type detection automatically.
        let schema_data = match flight_stream.next().await {
            Some(Ok(data)) => data,
            Some(Err(e)) => {
                yield Ok(json_line(&ErrorFrame {
                    frame_type: "error",
                    code: error_code_from_flight(&e),
                    message: e.to_string(),
                }));
                return;
            }
            None => {
                yield Ok(json_line(&ErrorFrame {
                    frame_type: "error",
                    code: ErrorCode::Internal,
                    message: "No schema in response".to_string(),
                }));
                return;
            }
        };

        // Send schema: JSON header, then IPC metadata + body (body is zero-copy)
        yield Ok(json_line(&DataHeader {
            frame_type: "schema",
            size: ipc_message_size(&schema_data),
        }));
        yield Ok(ipc_metadata_bytes(&schema_data));
        if !schema_data.data_body.is_empty() {
            yield Ok(schema_data.data_body.clone()); // Bytes::clone is cheap (refcount)
        }

        // Stream remaining FlightData messages as "batch" frames.
        // This includes RecordBatch and DictionaryBatch messages - the frontend's
        // RecordBatchReader handles both transparently. Empty results (zero batches)
        // are valid: the loop simply doesn't yield any batch frames.
        while let Some(result) = flight_stream.next().await {
            match result {
                Ok(flight_data) => {
                    // Send batch: JSON header, then IPC metadata + body (body is zero-copy)
                    yield Ok(json_line(&DataHeader {
                        frame_type: "batch",
                        size: ipc_message_size(&flight_data),
                    }));
                    yield Ok(ipc_metadata_bytes(&flight_data));
                    if !flight_data.data_body.is_empty() {
                        yield Ok(flight_data.data_body.clone()); // Bytes::clone is cheap (refcount)
                    }
                }
                Err(e) => {
                    yield Ok(json_line(&ErrorFrame {
                        frame_type: "error",
                        code: error_code_from_flight(&e),
                        message: e.to_string(),
                    }));
                    return;
                }
            }
        }

        // Success
        yield Ok(json_line(&DoneFrame { frame_type: "done" }));
    };

    (
        [(header::CONTENT_TYPE, "application/x-micromegas-arrow-stream")],
        Body::from_stream(stream)
    ).into_response()
}

fn error_code_from_flight(error: &arrow_flight::error::FlightError) -> ErrorCode {
    use arrow_flight::error::FlightError;
    match error {
        FlightError::Tonic(status) if status.code() == tonic::Code::DeadlineExceeded => {
            ErrorCode::Timeout
        }
        FlightError::Tonic(status) if status.code() == tonic::Code::Unavailable => {
            ErrorCode::ConnectionFailed
        }
        _ => ErrorCode::Internal,
    }
}
```

### Frontend Implementation

**File: `analytics-web-app/src/lib/arrow-stream.ts`**

```typescript
import { RecordBatch, RecordBatchReader, Schema } from 'apache-arrow';

type ErrorCode = 'INVALID_SQL' | 'TIMEOUT' | 'CONNECTION_FAILED' | 'INTERNAL';

// IPC end-of-stream marker: continuation marker (0xFFFFFFFF) + zero length
const EOS_MARKER = new Uint8Array([0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00]);

interface DataHeader {
  type: 'schema' | 'batch';
  size: number;
}

interface DoneFrame {
  type: 'done';
}

interface ErrorFrame {
  type: 'error';
  code: ErrorCode;
  message: string;
}

type Frame = DataHeader | DoneFrame | ErrorFrame;

export interface StreamError {
  code: ErrorCode;
  message: string;
  retryable: boolean;
}

// Discriminated union for cleaner type handling
export type StreamResult =
  | { type: 'schema'; schema: Schema }
  | { type: 'batch'; batch: RecordBatch }
  | { type: 'done' }
  | { type: 'error'; error: StreamError };

function isRetryable(code: ErrorCode): boolean {
  return code === 'TIMEOUT' || code === 'CONNECTION_FAILED';
}

/**
 * Buffered reader for processing streaming responses.
 * Handles chunk boundaries transparently for both line and binary reads.
 */
class BufferedReader {
  private chunks: Uint8Array[] = [];
  private offset = 0;
  private decoder = new TextDecoder();

  constructor(private reader: ReadableStreamDefaultReader<Uint8Array>) {}

  /**
   * Reads a newline-terminated line. Returns null if stream ends.
   */
  async readLine(): Promise<string | null> {
    let line = '';

    while (true) {
      if (this.chunks.length > 0) {
        const chunk = this.chunks[0];
        const newlineIdx = chunk.indexOf(10, this.offset); // 10 = '\n'

        if (newlineIdx !== -1) {
          line += this.decoder.decode(chunk.slice(this.offset, newlineIdx));
          this.offset = newlineIdx + 1;
          this.consumeIfExhausted();
          return line;
        }

        line += this.decoder.decode(chunk.slice(this.offset));
        this.chunks.shift();
        this.offset = 0;
      }

      const { done, value } = await this.reader.read();
      if (done) {
        return line.length > 0 ? line : null;
      }
      this.chunks.push(value);
    }
  }

  /**
   * Reads exactly `size` bytes.
   */
  async readBytes(size: number): Promise<Uint8Array> {
    const result = new Uint8Array(size);
    let written = 0;

    while (written < size) {
      if (this.chunks.length > 0) {
        const chunk = this.chunks[0];
        const available = chunk.length - this.offset;
        const needed = size - written;
        const toCopy = Math.min(available, needed);

        result.set(chunk.slice(this.offset, this.offset + toCopy), written);
        written += toCopy;
        this.offset += toCopy;
        this.consumeIfExhausted();
        continue;
      }

      const { done, value } = await this.reader.read();
      if (done) {
        throw new Error(`Unexpected end of stream, expected ${size - written} more bytes`);
      }
      this.chunks.push(value);
    }

    return result;
  }

  private consumeIfExhausted(): void {
    if (this.chunks.length > 0 && this.offset >= this.chunks[0].length) {
      this.chunks.shift();
      this.offset = 0;
    }
  }

  release(): void {
    this.reader.releaseLock();
  }
}

/**
 * Streams query results as Arrow RecordBatches.
 *
 * Uses RecordBatchReader.from() with a ReadableStream adapter that:
 * 1. Reads JSON frame headers internally (for size and error handling)
 * 2. Yields raw IPC bytes to the Arrow reader
 * 3. Arrow reader parses schema once, maintains dictionary state automatically
 */
export async function* streamQuery(
  sql: string,
  params: Record<string, string>,
  signal?: AbortSignal
): AsyncGenerator<StreamResult> {
  const response = await fetch(`${getApiUrl()}/query-stream`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...getAuthHeaders(),
    },
    body: JSON.stringify({ sql, ...params }),
    signal,
  });

  if (!response.ok) {
    if (response.status === 401) {
      throw new Error('Unauthorized');
    }
    const text = await response.text();
    throw new Error(`HTTP ${response.status}: ${text}`);
  }

  const bufferedReader = new BufferedReader(response.body!.getReader());

  // Captured error from error frame (reported after reader finishes)
  let capturedError: StreamError | null = null;

  // Create a ReadableStream that strips JSON framing and yields raw IPC bytes.
  // This lets RecordBatchReader handle schema caching and dictionary state.
  const ipcStream = new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        const line = await bufferedReader.readLine();
        if (line === null) {
          controller.close();
          return;
        }

        let frame: Frame;
        try {
          frame = JSON.parse(line);
        } catch {
          capturedError = {
            code: 'INTERNAL',
            message: `Invalid frame: ${line.slice(0, 100)}`,
            retryable: false,
          };
          controller.enqueue(EOS_MARKER);
          controller.close();
          return;
        }

        switch (frame.type) {
          case 'schema':
          case 'batch': {
            const bytes = await bufferedReader.readBytes(frame.size);
            controller.enqueue(bytes);
            break;
          }
          case 'done': {
            controller.enqueue(EOS_MARKER);
            controller.close();
            break;
          }
          case 'error': {
            capturedError = {
              code: frame.code,
              message: frame.message,
              retryable: isRetryable(frame.code),
            };
            controller.enqueue(EOS_MARKER);
            controller.close();
            break;
          }
        }
      } catch (e) {
        controller.error(e);
      }
    },
  });

  try {
    // RecordBatchReader.from() parses schema once and tracks dictionary state
    const reader = await RecordBatchReader.from(ipcStream);

    // Yield schema first
    yield { type: 'schema', schema: reader.schema };

    // Yield batches as they arrive
    for await (const batch of reader) {
      yield { type: 'batch', batch };
    }

    // Report error if one was captured, otherwise done
    if (capturedError) {
      yield { type: 'error', error: capturedError };
    } else {
      yield { type: 'done' };
    }
  } finally {
    bufferedReader.release();
  }
}

// These would be imported from existing api.ts
declare function getApiUrl(): string;
declare function getAuthHeaders(): Record<string, string>;
```

**File: `analytics-web-app/src/hooks/useStreamQuery.ts`**

```typescript
import { useState, useCallback, useRef } from 'react';
import { Table, Schema, RecordBatch } from 'apache-arrow';
import { streamQuery, StreamError, StreamResult } from '@/lib/arrow-stream';

interface StreamQueryState {
  schema: Schema | null;
  batchCount: number;
  isStreaming: boolean;
  isComplete: boolean;
  error: StreamError | null;
  rowCount: number;
}

export function useStreamQuery() {
  const [state, setState] = useState<StreamQueryState>({
    schema: null,
    batchCount: 0,
    isStreaming: false,
    isComplete: false,
    error: null,
    rowCount: 0,
  });

  const abortRef = useRef<AbortController | null>(null);
  // Mutable array to avoid O(n²) allocations from spreading
  const batchesRef = useRef<RecordBatch[]>([]);

  const execute = useCallback(async (sql: string, params: Record<string, string> = {}) => {
    abortRef.current?.abort();
    abortRef.current = new AbortController();
    batchesRef.current = [];

    setState({
      schema: null,
      batchCount: 0,
      isStreaming: true,
      isComplete: false,
      error: null,
      rowCount: 0,
    });

    try {
      for await (const result of streamQuery(sql, params, abortRef.current.signal)) {
        switch (result.type) {
          case 'schema':
            setState(s => ({ ...s, schema: result.schema }));
            break;
          case 'batch':
            batchesRef.current.push(result.batch);
            setState(s => ({
              ...s,
              batchCount: batchesRef.current.length,
              rowCount: s.rowCount + result.batch.numRows,
            }));
            break;
          case 'done':
            setState(s => ({ ...s, isStreaming: false, isComplete: true }));
            break;
          case 'error':
            setState(s => ({
              ...s,
              error: result.error,
              isStreaming: false,
              isComplete: true,
            }));
            break;
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

  const retry = useCallback((sql: string, params: Record<string, string> = {}) => {
    if (state.error?.retryable) {
      execute(sql, params);
    }
  }, [state.error, execute]);

  const getTable = useCallback((): Table | null => {
    if (batchesRef.current.length === 0) return null;
    return new Table(batchesRef.current);
  }, []);

  const getBatches = useCallback((): RecordBatch[] => {
    return batchesRef.current;
  }, []);

  return { ...state, execute, cancel, retry, getTable, getBatches };
}
```

## File Changes Summary

### New Files

**Backend:**
- `rust/analytics-web-srv/src/stream_query.rs` - Streaming query endpoint with framed protocol

**Frontend:**
- `analytics-web-app/src/lib/arrow-stream.ts` - Framed stream parser
- `analytics-web-app/src/hooks/useStreamQuery.ts` - Streaming query hook

### Modified Files

**Backend:**
- `rust/analytics-web-srv/src/main.rs` - Add `/query-stream` route, add module
- `rust/analytics-web-srv/src/flightsql_client.rs` - Add `query_flight_data()` method for raw FlightData access

**Frontend:**
- `analytics-web-app/package.json` - Add `apache-arrow` dependency

### Unchanged Files
- Existing `/query` endpoint - kept for backward compatibility

## Dependencies

### Backend
- `arrow-flight` - Already used for FlightSQL client
- `arrow-ipc` - For CONTINUATION_MARKER constant
- `async-stream` - For streaming (add if needed: `async-stream = "0.3"`)

### Frontend (new)
- `apache-arrow` - Arrow JavaScript library

## Testing Strategy

### Backend Tests
1. Unit tests for `ipc_message_size` and `ipc_metadata_bytes`:
   - Verify size calculation matches actual bytes produced
   - Verify padding aligns body to 8-byte boundary
   - Test with various metadata sizes (edge cases: 0, 1, 7, 8, 9 bytes)
2. Integration tests for `/query-stream`:
   - Valid query returns schema + batches + done
   - Invalid SQL returns error frame (no schema)
   - Empty result returns schema + done (zero batch frames)
   - Single row result (schema + one batch + done)
   - Dictionary-encoded columns across multiple batches
   - Large result (verify streaming, not buffered)
3. Verify frame format matches specification

### Frontend Tests
1. Unit tests for `BufferedReader.readLine` and `readBytes`:
   - Handle chunks split across reads
   - Handle multiple frames in single chunk
   - Handle exact chunk boundaries
2. Unit tests for `streamQuery`:
   - Parse schema frame, read exact byte count
   - Parse batch frames, read exact byte count
   - Empty result: yields schema, then done (no batches)
   - Handle done and error frames
   - Handle unexpected stream end
   - Dictionary-encoded columns preserve values across batches
3. Integration tests with mock fetch responses
4. Hook tests:
   - State updates during streaming
   - Empty result: schema set, batchCount=0, isComplete=true
   - Cancellation works
   - Retry only for retryable errors

## Migration Strategy

1. Deploy backend with new `/query-stream` endpoint
2. Add frontend Arrow dependency
3. Implement frontend stream parser
4. Create `useStreamQuery` hook
5. Migrate components incrementally to new hook
6. Keep existing `/query` endpoint for compatibility

## Benefits

- **Near-zero backend CPU**: FlightData passthrough, no Arrow deserialization/reserialization
- **True streaming**: Process batches as they arrive over network
- **Progressive rendering**: UI can update with each batch
- **Lower latency**: First results appear before query completes
- **Smaller payload**: Arrow IPC more compact than JSON
- **Type preservation**: Timestamps, large integers stay native
- **Frontend efficiency**: `RecordBatchReader` parses schema once, maintains dictionary state
- **Simple framing**: JSON headers are human-readable, easy to debug

## Performance Expectations

| Metric | Current (JSON) | Proposed (Framed Arrow) |
|--------|----------------|-------------------------|
| Time to first row | After full query | After first batch |
| Payload size (1M rows) | ~100MB JSON | ~20-40MB Arrow IPC |
| Backend CPU | High (JSON per cell) | Near-zero (passthrough) |
| Frontend CPU | High (JSON parse) | Low (Arrow zero-copy) |
| Type fidelity | Lossy | Lossless |
| Memory (backend) | Full result buffered | Stream-through |

## Open Questions

1. **Batch size control**: Should we expose a parameter to control RecordBatch size from FlightSQL?
2. **Compression**: Should we add optional gzip compression via Accept-Encoding?

## References

- [Arrow IPC Format](https://arrow.apache.org/docs/format/Columnar.html#ipc-streaming-format)
- [Apache Arrow JavaScript](https://arrow.apache.org/docs/js/)
- [tableFromIPC](https://arrow.apache.org/docs/js/functions/Arrow.dom.tableFromIPC.html)
