# Arrow IPC Streaming for Query Endpoint

**Status: In Progress (3 components remaining)**

GitHub Issue: https://github.com/madesroches/micromegas/issues/664

## Summary

Replace JSON serialization in the `/query` endpoint with streaming Arrow IPC to reduce latency, improve efficiency, and enable progressive rendering. The backend streams RecordBatches from FlightSQL and encodes them to Arrow IPC format with JSON framing for the frontend to parse.

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
{"type":"error","code":"INTERNAL","message":"Connection lost during query"}\n
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
| `CONNECTION_FAILED` | Failed to connect to FlightSQL | Yes |
| `INTERNAL` | Unexpected backend error | No |
| `FORBIDDEN` | Blocked function (destructive operation) | No |

### Error Scenarios

| Scenario | Response | Frontend Handling |
|----------|----------|-------------------|
| Auth failure | HTTP 401 | Redirect to login |
| Invalid SQL | Error frame | Show error, no data |
| FlightSQL down | Error frame (no schema) | Show error with retry |
| Blocked function | Error frame (HTTP 403) | Show error, no retry |
| Success | Schema + batches + done | Display complete results |
| Network drop | Fetch throws | Catch, show error |
| User cancel | AbortController | Clean up, no error |

## Implementation Progress

### Phase 1: Backend Streaming Endpoint
- [x] Add `query_flight_data()` method to FlightSQL client (returns raw FlightData stream)
  - Note: Changed approach to use RecordBatch streaming with IPC encoding instead of raw FlightData passthrough
  - The arrow-flight API doesn't expose raw FlightData from FlightSqlServiceClient
- [x] Create `/query-stream` endpoint with streaming response
  - Implemented in `rust/analytics-web-srv/src/stream_query.rs`
- [x] Convert RecordBatch to IPC bytes using `IpcDataGenerator`
- [x] Send JSON frame headers with size prefixes
- [x] Handle errors before and during streaming
- [x] Add integration tests
  - Unit tests in `rust/analytics-web-srv/tests/stream_query_tests.rs`
  - Tests for `contains_blocked_function`, `substitute_macros`, `encode_schema`, `encode_batch`

### Phase 2: Frontend Arrow Dependency
- [x] Add `apache-arrow` package to analytics-web-app
- [x] Verify bundle size impact (~1.5MB, tree-shakeable)

### Phase 3: Frontend Stream Consumer
- [x] Implement framed stream reader in `lib/arrow-stream.ts`
- [x] Parse JSON headers, read binary payloads by size
- [x] Use `RecordBatchReader` for IPC deserialization
- [x] Yield batches as async generator
- [x] Handle all error scenarios

### Phase 4: Frontend Hook Integration
- [x] Create `useStreamQuery` hook
- [x] Support progressive data accumulation
- [x] Provide loading/streaming/complete states
- [x] Handle cancellation (AbortController)

## Technical Details

### Backend Implementation

**File: `rust/analytics-web-srv/src/stream_query.rs`**

The implementation uses RecordBatch streaming with Arrow IPC encoding. Key functions:

```rust
/// Encode a schema to Arrow IPC format
fn encode_schema(schema: &Schema) -> Result<Vec<u8>, String> {
    let mut buffer = Vec::new();
    let data_gen = IpcDataGenerator::default();
    let options = IpcWriteOptions::default();
    let mut tracker = DictionaryTracker::new(false);

    let encoded = data_gen.schema_to_bytes_with_dictionary_tracker(schema, &mut tracker, &options);
    write_message(&mut buffer, encoded, &options)?;
    Ok(buffer)
}

/// Encode a RecordBatch to Arrow IPC format
fn encode_batch(
    batch: &RecordBatch,
    tracker: &mut DictionaryTracker,
    compression: &mut CompressionContext,
) -> Result<Vec<u8>, String> {
    let mut buffer = Vec::new();
    let data_gen = IpcDataGenerator::default();
    let options = IpcWriteOptions::default();

    let (encoded_dicts, encoded_batch) = data_gen.encode(batch, tracker, &options, compression)?;

    // Write dictionary batches first (if any)
    for dict in encoded_dicts {
        write_message(&mut buffer, dict, &options)?;
    }

    // Write the main batch
    write_message(&mut buffer, encoded_batch, &options)?;

    Ok(buffer)
}
```

The handler streams RecordBatches from FlightRecordBatchStream, encodes each to IPC format, and sends with JSON frame headers.

### Frontend Implementation

**File: `analytics-web-app/src/lib/arrow-stream.ts`**

The implementation uses a BufferedReader to parse JSON frame headers and read binary IPC data:

```typescript
/**
 * Streams query results as Arrow RecordBatches.
 * Parses JSON-framed protocol and yields schema, batches, done/error.
 */
export async function* streamQuery(
  params: StreamQueryParams,
  signal?: AbortSignal
): AsyncGenerator<StreamResult> {
  // ... fetch and error handling ...

  const bufferedReader = new BufferedReader(response.body!.getReader());
  const ipcChunks: Uint8Array[] = [];

  while (true) {
    const line = await bufferedReader.readLine();
    if (line === null) break;

    const frame: Frame = JSON.parse(line);

    switch (frame.type) {
      case 'schema':
      case 'batch': {
        const bytes = await bufferedReader.readBytes(frame.size);
        ipcChunks.push(bytes);
        // Parse and yield schema/batch using tableFromIPC or RecordBatchReader
        break;
      }
      case 'done':
        yield { type: 'done' };
        return;
      case 'error':
        yield { type: 'error', error: { code: frame.code, message: frame.message, ... } };
        break;
    }
  }
}
```

**File: `analytics-web-app/src/hooks/useStreamQuery.ts`**

React hook for streaming query with progressive loading:

```typescript
export function useStreamQuery(): UseStreamQueryReturn {
  const [state, setState] = useState<StreamQueryState>({ ... });
  const batchesRef = useRef<RecordBatch[]>([]);

  const execute = useCallback(async (params: StreamQueryParams) => {
    // ... setup AbortController, reset state ...

    for await (const result of streamQuery(params, abortRef.current.signal)) {
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
        // ... done, error handling ...
      }
    }
  }, []);

  return { ...state, execute, cancel, retry, getTable, getBatches };
}
```

## File Changes Summary

### New Files

**Backend:**
- `rust/analytics-web-srv/src/stream_query.rs` - Streaming query endpoint with framed protocol
- `rust/analytics-web-srv/tests/stream_query_tests.rs` - Unit tests (20 tests)

**Frontend:**
- `analytics-web-app/src/lib/arrow-stream.ts` - Framed stream parser
- `analytics-web-app/src/lib/__tests__/arrow-stream.test.ts` - Unit tests (16 tests)
- `analytics-web-app/src/hooks/useStreamQuery.ts` - Streaming query hook
- `analytics-web-app/src/hooks/__tests__/useStreamQuery.test.ts` - Unit tests (16 tests)

### Modified Files

**Backend:**
- `rust/analytics-web-srv/src/main.rs` - Add `/query-stream` route, add `stream_query` module
- `rust/analytics-web-srv/src/lib.rs` - Export `stream_query` module for testing
- `rust/analytics-web-srv/Cargo.toml` - Add `arrow-ipc`, `arrow-flight`, `tonic` dependencies
- `rust/Cargo.toml` - Add `arrow-ipc` to workspace dependencies

**Frontend:**
- `analytics-web-app/package.json` - Add `apache-arrow` dependency
- `analytics-web-app/src/test-setup.ts` - Add polyfills for TextEncoder/TextDecoder, ReadableStream

### Unchanged Files
- Existing `/query` endpoint - kept for backward compatibility

## Dependencies

### Backend
- `arrow-flight` - For FlightRecordBatchStream (already used for FlightSQL client)
- `arrow-ipc` - For IpcDataGenerator, IpcWriteOptions, write_message, DictionaryTracker, CompressionContext
- `async-stream` - For streaming (already in workspace)
- `tonic` - For gRPC types

### Frontend (new)
- `apache-arrow` (v21.1.0) - Arrow JavaScript library for IPC parsing

## Testing Strategy

### Backend Tests ✅ Implemented

**File: `rust/analytics-web-srv/tests/stream_query_tests.rs`** (20 tests)

1. Unit tests for `encode_schema` and `encode_batch`:
   - [x] Verify IPC bytes can be parsed by Arrow reader
   - [x] Test with various schema types (strings, integers, timestamps, etc.)
   - [x] Test dictionary encoding across batches
   - [x] Test empty schema and empty batch handling
2. Unit tests for helper functions:
   - [x] `contains_blocked_function` - blocked and allowed queries
   - [x] `substitute_macros` - basic substitution, SQL injection prevention
3. [x] ErrorCode serialization to SCREAMING_SNAKE_CASE

### Frontend Tests ✅ Implemented

**File: `analytics-web-app/src/lib/__tests__/arrow-stream.test.ts`** (16 tests)

1. Error handling tests:
   - [x] HTTP 401 throws AuthenticationError
   - [x] HTTP 403 returns error frame with FORBIDDEN
   - [x] HTTP 500 throws with error message
   - [x] Missing response body throws
2. Frame parsing tests:
   - [x] Invalid JSON yields INTERNAL error
   - [x] Done frame completes stream
   - [x] Error frames with correct retryable flag
3. Request parameter tests:
   - [x] Correct request body sent
   - [x] Abort signal passed through
4. Error code retryability:
   - [x] CONNECTION_FAILED is retryable
   - [x] INVALID_SQL, INTERNAL, FORBIDDEN are not retryable

**File: `analytics-web-app/src/hooks/__tests__/useStreamQuery.test.ts`** (16 tests)

1. Initial state tests:
   - [x] Correct initial state values
   - [x] All functions provided
2. Execute tests:
   - [x] Updates schema when received
   - [x] Accumulates batches and counts
   - [x] Sets isComplete on done
   - [x] Sets error on error result
   - [x] Handles thrown errors
   - [x] Resets state on new execute
3. Cancel tests:
   - [x] Sets isStreaming to false
4. Retry tests:
   - [x] Retries with last params if retryable
   - [x] Does not retry if not retryable
5. getTable/getBatches tests:
   - [x] Returns null/empty when no batches
   - [x] Returns Table from batches
   - [x] Returns batches array

## Migration Strategy

1. [x] Deploy backend with new `/query-stream` endpoint
2. [x] Add frontend Arrow dependency
3. [x] Implement frontend stream parser
4. [x] Create `useStreamQuery` hook
5. [x] Migrate components incrementally to new hook
   - [x] `ProcessesPage` - uses `useStreamQuery`, iterates directly on Arrow Table rows
   - [x] `ProcessPage` - uses `useStreamQuery` with 3 hooks (process, stats, properties)
   - [x] `ProcessLogPage` - uses `useStreamQuery` for log entries
   - [x] `ProcessMetricsPage` - uses `useStreamQuery` with 3 hooks (discovery, data, process)
   - [ ] `PerformanceAnalysisPage` - 5 mutations using `executeSqlQuery` (spans, block counts, measures)
   - [ ] `usePropertyTimeline` hook - timeline of property values
   - [ ] `usePropertyKeys` hook - property key listing
6. [x] Keep existing `/query` endpoint for compatibility
7. [ ] Remove old `/query` endpoint and `executeSqlQuery` after full migration

## Benefits

- **True streaming**: Process batches as they arrive over network (no buffering entire result)
- **Progressive rendering**: UI can update with each batch
- **Lower latency**: First results appear before query completes
- **Smaller payload**: Arrow IPC more compact than JSON (~2-5x smaller)
- **Type preservation**: Timestamps, large integers stay native (no JSON precision loss)
- **Frontend efficiency**: Arrow's columnar format enables zero-copy access
- **Simple framing**: JSON headers are human-readable, easy to debug
- **Lower backend CPU**: IPC encoding is cheaper than JSON serialization per cell

## Performance Expectations

| Metric | Current (JSON) | Proposed (Framed Arrow) |
|--------|----------------|-------------------------|
| Time to first row | After full query | After first batch |
| Payload size (1M rows) | ~100MB JSON | ~20-40MB Arrow IPC |
| Backend CPU | High (JSON per cell) | Lower (IPC encoding) |
| Frontend CPU | High (JSON parse) | Low (Arrow columnar) |
| Type fidelity | Lossy | Lossless |
| Memory (backend) | Full result buffered | Stream-through |

## Open Questions

1. **Batch size control**: Should we expose a parameter to control RecordBatch size from FlightSQL?
2. **Compression**: Should we add optional gzip compression via Accept-Encoding?

## References

- [Arrow IPC Format](https://arrow.apache.org/docs/format/Columnar.html#ipc-streaming-format)
- [Apache Arrow JavaScript](https://arrow.apache.org/docs/js/)
- [tableFromIPC](https://arrow.apache.org/docs/js/functions/Arrow.dom.tableFromIPC.html)
