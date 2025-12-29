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

Use a JSON-framed protocol that enables true streaming:

```
{"type":"schema","arrow_schema":"<base64 IPC schema bytes>"}\n
{"type":"batch","size":4096}\n
[4096 bytes of raw Arrow IPC batch message]
{"type":"batch","size":2048}\n
[2048 bytes of raw Arrow IPC batch message]
{"type":"done"}\n
```

On error:
```
{"type":"schema","arrow_schema":"<base64>"}\n
{"type":"batch","size":1024}\n
[1024 bytes - partial batch message]
{"type":"error","code":"TIMEOUT","message":"Query exceeded time limit"}\n
```

**Key design points:**
- Each frame starts with a JSON line (newline-terminated)
- Schema sent once (base64); batch frames contain only raw batch bytes (no schema repetition)
- Frontend caches schema bytes and reconstructs complete IPC streams locally
- True streaming: frontend processes each batch as it arrives
- No buffering required to find message boundaries
- Errors can occur at any point, partial results preserved
- Dictionary state maintained across batches for correct dictionary-encoded columns

### Frame Types

| Type | Fields | Followed By |
|------|--------|-------------|
| `schema` | `arrow_schema` (base64 IPC schema message, no EOS) | Nothing |
| `batch` | `size` (byte count) | Raw batch IPC bytes (no schema, no EOS) |
| `done` | None | Nothing (stream complete) |
| `error` | `code`, `message` | Nothing (stream complete) |

**Note:** Frontend reconstructs complete IPC streams by combining cached schema bytes + batch bytes + EOS marker.

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
- [ ] Create `/query-stream` endpoint with streaming response
- [ ] Serialize schema to base64 IPC format
- [ ] Serialize each RecordBatch to IPC bytes
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
use arrow::ipc::writer::{IpcDataGenerator, IpcWriteOptions};
use arrow::ipc::{writer::write_message, DictionaryTracker, CONTINUATION_MARKER};
use arrow::record_batch::RecordBatch;
use arrow_schema::Schema;
use async_stream::stream;
use axum::body::Body;
use axum::response::IntoResponse;
use base64::Engine;
use bytes::Bytes;
use futures::StreamExt;
use serde::Serialize;
use std::io::Write;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidSql,
    Timeout,
    ConnectionFailed,
    Internal,
}

#[derive(Serialize)]
struct SchemaFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    arrow_schema: String,
}

#[derive(Serialize)]
struct BatchFrame {
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

/// Serializes schema to IPC format bytes (schema message only, no EOS).
/// Returns bytes that can be prepended to batch messages to form valid IPC streams.
fn schema_to_ipc_bytes(schema: &Schema) -> Result<Vec<u8>, arrow::error::ArrowError> {
    let gen = IpcDataGenerator::default();
    let options = IpcWriteOptions::default();
    let encoded = gen.schema_to_bytes(schema, &options);
    let mut buffer = Vec::new();
    write_message(&mut buffer, encoded, &options)?;
    Ok(buffer)
}

/// Serializes a batch message (without schema header).
/// Uses provided DictionaryTracker to maintain dictionary state across batches.
fn batch_to_raw_ipc(
    batch: &RecordBatch,
    dict_tracker: &mut DictionaryTracker,
    options: &IpcWriteOptions,
) -> Result<Vec<u8>, arrow::error::ArrowError> {
    let gen = IpcDataGenerator::default();
    let (dicts, encoded) = gen.encoded_batch(batch, dict_tracker, options)?;

    let mut buffer = Vec::new();
    for dict in dicts {
        write_message(&mut buffer, dict, options)?;
    }
    write_message(&mut buffer, encoded, options)?;
    Ok(buffer)
}

/// Writes the IPC end-of-stream marker.
fn write_eos_marker(buffer: &mut Vec<u8>) -> Result<(), arrow::error::ArrowError> {
    buffer.write_all(&CONTINUATION_MARKER)?;
    buffer.write_all(&0i32.to_le_bytes())?;
    Ok(())
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

        // Start streaming query
        let time_range = parse_time_range(&request);
        let mut batch_stream = match client.query_stream(&request.sql, time_range).await {
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

        // Get and serialize schema once
        let schema = match batch_stream.schema() {
            Some(s) => s,
            None => {
                yield Ok(json_line(&ErrorFrame {
                    frame_type: "error",
                    code: ErrorCode::Internal,
                    message: "No schema available".to_string(),
                }));
                return;
            }
        };

        // Serialize schema bytes once - sent to frontend for local IPC reconstruction
        // Note: schema may be Arc<Schema>, use as_ref() to get &Schema
        let schema_bytes = match schema_to_ipc_bytes(schema.as_ref()) {
            Ok(bytes) => bytes,
            Err(e) => {
                yield Ok(json_line(&ErrorFrame {
                    frame_type: "error",
                    code: ErrorCode::Internal,
                    message: e.to_string(),
                }));
                return;
            }
        };

        // Send schema frame with base64-encoded schema bytes (no EOS - frontend will add)
        yield Ok(json_line(&SchemaFrame {
            frame_type: "schema",
            arrow_schema: base64::engine::general_purpose::STANDARD.encode(&schema_bytes),
        }));

        // Maintain dictionary state across all batches for correct dictionary-encoded columns
        let mut dict_tracker = DictionaryTracker::new(false);
        let ipc_options = IpcWriteOptions::default();

        // Stream batches - send only batch bytes, frontend reconstructs IPC locally
        while let Some(result) = batch_stream.next().await {
            match result {
                Ok(batch) => {
                    match batch_to_raw_ipc(&batch, &mut dict_tracker, &ipc_options) {
                        Ok(batch_bytes) => {
                            yield Ok(json_line(&BatchFrame {
                                frame_type: "batch",
                                size: batch_bytes.len(),
                            }));
                            yield Ok(Bytes::from(batch_bytes));
                        }
                        Err(e) => {
                            yield Ok(json_line(&ErrorFrame {
                                frame_type: "error",
                                code: ErrorCode::Internal,
                                message: e.to_string(),
                            }));
                            return;
                        }
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

fn error_code_from_flight(error: &FlightError) -> ErrorCode {
    // Map FlightSQL errors to our error codes
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
import { RecordBatch, Schema, tableFromIPC } from 'apache-arrow';

type ErrorCode = 'INVALID_SQL' | 'TIMEOUT' | 'CONNECTION_FAILED' | 'INTERNAL';

// IPC end-of-stream marker: continuation marker (0xFFFFFFFF) + zero length
const EOS_MARKER = new Uint8Array([0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00]);

interface SchemaFrame {
  type: 'schema';
  arrow_schema: string;
}

interface BatchFrame {
  type: 'batch';
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

type Frame = SchemaFrame | BatchFrame | DoneFrame | ErrorFrame;

export interface StreamError {
  code: ErrorCode;
  message: string;
  retryable: boolean;
}

export interface StreamResult {
  schema?: Schema;
  batch?: RecordBatch;
  done: boolean;
  error?: StreamError;
}

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
      // Check current chunk for newline
      if (this.chunks.length > 0) {
        const chunk = this.chunks[0];
        const newlineIdx = chunk.indexOf(10, this.offset); // 10 = '\n'

        if (newlineIdx !== -1) {
          line += this.decoder.decode(chunk.slice(this.offset, newlineIdx));
          this.offset = newlineIdx + 1;
          this.consumeIfExhausted();
          return line;
        }

        // No newline, consume entire chunk
        line += this.decoder.decode(chunk.slice(this.offset));
        this.chunks.shift();
        this.offset = 0;
      }

      // Read more data
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
 * Decodes base64 to Uint8Array, handling binary data correctly.
 */
function base64ToBytes(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

/**
 * Builds a complete IPC stream from cached schema bytes and batch bytes.
 * This avoids sending schema over the wire for each batch.
 */
function buildIpcStream(schemaBytes: Uint8Array, batchBytes: Uint8Array): Uint8Array {
  const result = new Uint8Array(schemaBytes.length + batchBytes.length + EOS_MARKER.length);
  result.set(schemaBytes, 0);
  result.set(batchBytes, schemaBytes.length);
  result.set(EOS_MARKER, schemaBytes.length + batchBytes.length);
  return result;
}

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
  // Cache schema bytes for reconstructing IPC streams locally (avoids schema-per-batch over wire)
  let cachedSchemaBytes: Uint8Array | null = null;

  try {
    while (true) {
      const line = await bufferedReader.readLine();
      if (line === null) {
        yield {
          done: true,
          error: {
            code: 'INTERNAL',
            message: 'Stream ended unexpectedly',
            retryable: true,
          },
        };
        return;
      }

      let frame: Frame;
      try {
        frame = JSON.parse(line);
      } catch {
        yield {
          done: true,
          error: {
            code: 'INTERNAL',
            message: `Invalid frame: ${line.slice(0, 100)}`,
            retryable: false,
          },
        };
        return;
      }

      switch (frame.type) {
        case 'schema': {
          // Decode and cache schema bytes for batch reconstruction
          cachedSchemaBytes = base64ToBytes(frame.arrow_schema);
          // Parse schema by building a complete IPC stream (schema + EOS)
          const schemaIpc = buildIpcStream(cachedSchemaBytes, new Uint8Array(0));
          const table = tableFromIPC(schemaIpc);
          yield { schema: table.schema, done: false };
          break;
        }

        case 'batch': {
          if (!cachedSchemaBytes) {
            yield {
              done: true,
              error: {
                code: 'INTERNAL',
                message: 'Received batch before schema',
                retryable: false,
              },
            };
            return;
          }
          // Read batch bytes and reconstruct complete IPC stream locally
          const batchBytes = await bufferedReader.readBytes(frame.size);
          const ipcStream = buildIpcStream(cachedSchemaBytes, batchBytes);
          const table = tableFromIPC(ipcStream);
          for (const batch of table.batches) {
            yield { batch, done: false };
          }
          break;
        }

        case 'done': {
          yield { done: true };
          return;
        }

        case 'error': {
          yield {
            done: true,
            error: {
              code: frame.code,
              message: frame.message,
              retryable: isRetryable(frame.code),
            },
          };
          return;
        }
      }
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
import { streamQuery, StreamError } from '@/lib/arrow-stream';

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
        if (result.error) {
          setState(s => ({
            ...s,
            error: result.error!,
            isStreaming: false,
            isComplete: true,
          }));
          return;
        }
        if (result.schema) {
          setState(s => ({ ...s, schema: result.schema }));
        }
        if (result.batch) {
          batchesRef.current.push(result.batch);
          setState(s => ({
            ...s,
            batchCount: batchesRef.current.length,
            rowCount: s.rowCount + result.batch!.numRows,
          }));
        }
        if (result.done && !result.error) {
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
- `rust/analytics-web-srv/Cargo.toml` - Add `base64` dependency (if not present)

**Frontend:**
- `analytics-web-app/package.json` - Add `apache-arrow` dependency

### Unchanged Files
- Existing `/query` endpoint - kept for backward compatibility

## Dependencies

### Backend
- `arrow` - Already in workspace, provides IPC serialization
- `base64` - For encoding schema (likely already available)
- `async-stream` - For streaming (add if needed: `async-stream = "0.3"`)

### Frontend (new)
- `apache-arrow` - Arrow JavaScript library

## Testing Strategy

### Backend Tests
1. Unit tests for `schema_to_ipc_base64` and `batch_to_ipc_bytes`
2. Integration tests for `/query-stream`:
   - Valid query returns schema + batches + done
   - Invalid SQL returns error frame
   - Empty result returns schema + done (no batches)
3. Verify frame format matches specification

### Frontend Tests
1. Unit tests for `readLine` and `readBytes`:
   - Handle chunks split across reads
   - Handle multiple frames in single chunk
2. Unit tests for `streamQuery`:
   - Parse schema frame, decode base64
   - Parse batch frames, read exact byte count
   - Handle done and error frames
   - Handle unexpected stream end
3. Integration tests with mock fetch responses
4. Hook tests:
   - State updates during streaming
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

- **True streaming**: Process batches as they arrive over network
- **Progressive rendering**: UI can update with each batch
- **Lower latency**: First results appear before query completes
- **Smaller payload**: Arrow IPC more compact than JSON
- **Type preservation**: Timestamps, large integers stay native
- **Less CPU**: No JSON stringify/parse for data
- **Simple framing**: JSON headers are human-readable, easy to debug

## Performance Expectations

| Metric | Current (JSON) | Proposed (Framed Arrow) |
|--------|----------------|-------------------------|
| Time to first row | After full query | After first batch |
| Payload size (1M rows) | ~100MB JSON | ~20-40MB Arrow IPC |
| Backend CPU | High (JSON per cell) | Low (IPC serialization) |
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
