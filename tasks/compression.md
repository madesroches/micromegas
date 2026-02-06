# Response Compression for analytics-web-srv

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/781

## Problem
Corporate proxy kills connections when generating large Perfetto traces (~255 MB body size), causing `net::ERR_CONNECTION_CLOSED`.

## What's done
- `tower-http` `CompressionLayer` with gzip on the router — all endpoints compressed transparently
- Chunk boundary fix in `generateTrace`: `findBinaryDataOffset()` extracts trailing binary data when gzip merges the `binary_start` marker and binary data into one chunk
- `postMessage` console error fix during Perfetto handshake

## What's left — Option B: client-side trace generation

The `generate_trace` endpoint is a server-side proxy: it creates a FlightSQL client, runs `perfetto_trace_chunks()`, and forwards the raw protobuf bytes through a fragile mixed protocol (JSON progress lines → `binary_start` marker → raw binary). The client already has full FlightSQL query capability via `stream_query`. The chunks are already-encoded protobuf — the client just needs to concatenate them.

Eliminate the `generate_trace` endpoint entirely. The client queries `perfetto_trace_chunks()` via `streamQuery`, extracts the `chunk_data` binary column from each Arrow batch, and concatenates into an `ArrayBuffer`.

### Step 1: New client function `fetchPerfettoTrace`

**File**: `analytics-web-app/src/lib/perfetto-trace.ts` (new)

```typescript
import { streamQuery } from './arrow-stream'

export interface FetchPerfettoTraceOptions {
  processId: string
  spanType: 'thread' | 'async' | 'both'
  timeRange: { begin: string; end: string }
  onProgress?: (message: string) => void
  signal?: AbortSignal
}

export async function fetchPerfettoTrace(
  options: FetchPerfettoTraceOptions
): Promise<ArrayBuffer>
```

Implementation:
- Build the SQL: `SELECT chunk_id, chunk_data FROM perfetto_trace_chunks('{processId}', '{spanType}', TIMESTAMP '{begin}', TIMESTAMP '{end}')`
- Call `streamQuery({ sql, begin, end }, signal)`
- For each `batch` result, iterate rows and extract the `chunk_data` binary column values into a `Uint8Array[]`
- Report progress via `onProgress` as bytes accumulate (e.g. `"Downloading trace... 12.3 MB received"`)
- On `done`, concatenate all chunks into a single `ArrayBuffer` and return it
- On `error`, throw with the error message
- If no data received, throw `"No trace data generated"`

**Unit test**: `analytics-web-app/src/lib/__tests__/perfetto-trace.test.ts`
- Mock `streamQuery` to yield schema + batches with binary `chunk_data` column
- Verify chunks are concatenated in order
- Verify progress callback is called with byte counts
- Verify error from stream is thrown
- Verify empty stream throws
- Verify abort signal is forwarded

### Step 2: Update `PerfettoExportCell` to use `fetchPerfettoTrace`

**File**: `analytics-web-app/src/lib/screen-renderers/cells/PerfettoExportCell.tsx`

Changes:
- Replace `import { generateTrace } from '@/lib/api'` with `import { fetchPerfettoTrace } from '@/lib/perfetto-trace'`
- Remove `import type { GenerateTraceRequest, ProgressUpdate } from '@/types'`
- Remove `buildTraceRequest` callback
- In `handleOpenInPerfetto` and `handleDownloadTrace`, replace:
  ```typescript
  const buffer = await generateTrace(processId, buildTraceRequest(), (update) => {
    setProgress(update)
  }, { returnBuffer: true })
  ```
  with:
  ```typescript
  const buffer = await fetchPerfettoTrace({
    processId,
    spanType,
    timeRange,
    onProgress: (message) => setProgress({ type: 'progress', message }),
  })
  ```
- The `progress` state type stays `{ type: 'progress'; message: string } | null` — just inline it or keep `ProgressUpdate` locally if needed

**Update test**: `analytics-web-app/src/lib/screen-renderers/cells/__tests__/PerfettoExportCell.test.tsx`
- Mock `@/lib/perfetto-trace` instead of `@/lib/api`
- Update `mockGenerateTrace` → `mockFetchPerfettoTrace` with the new signature
- Verify `fetchPerfettoTrace` is called with `{ processId, spanType, timeRange, onProgress }` (not the old `GenerateTraceRequest` shape)

### Step 2b: Update `PerformanceAnalysisPage` to use `fetchPerfettoTrace`

**File**: `analytics-web-app/src/routes/PerformanceAnalysisPage.tsx`

This page also calls `generateTrace` directly (lines 619, 669) with hardcoded `include_async_spans: true, include_thread_spans: true`.

Changes:
- Replace `import { generateTrace } from '@/lib/api'` with `import { fetchPerfettoTrace } from '@/lib/perfetto-trace'`
- Remove `GenerateTraceRequest` import from `@/types`
- In `handleOpenInPerfetto` (line 619), replace `generateTrace(processId, request, ...)` with:
  ```typescript
  const buffer = await fetchPerfettoTrace({
    processId,
    spanType: 'both',
    timeRange: currentTimeRange,
    onProgress: (message) => setProgress({ type: 'progress', message }),
  })
  ```
- In `handleDownloadTrace` (line 669), same replacement — but note the old code used `generateTrace` without `returnBuffer` (triggers browser download). The new `fetchPerfettoTrace` always returns an `ArrayBuffer`, so add the download trigger inline (same `triggerDownload` pattern as PerfettoExportCell, or extract to a shared helper).

### Step 3: Delete `generateTrace` from `api.ts`

**File**: `analytics-web-app/src/lib/api.ts`

Delete:
- `import { GenerateTraceRequest, ProgressUpdate, BinaryStartMarker } from '@/types'` (line 1)
- `GenerateTraceOptions` interface (lines 98-101)
- `generateTrace` function (lines 103-259) — includes `findBinaryDataOffset`

Keep: `authenticatedFetch`, `getApiBase`, error classes — still used by `arrow-stream.ts` and elsewhere.

### Step 4: Clean up types

**File**: `analytics-web-app/src/types/index.ts`

Delete:
- `GenerateTraceRequest` interface (lines 1-8)
- `BinaryStartMarker` interface (lines 15-17)

Keep or move `ProgressUpdate` — check if anything still uses it. If only `PerfettoExportCell` uses it, inline the type there or just use `{ type: 'progress'; message: string }` directly.

### Step 5: Remove server-side `generate_trace` endpoint

**File**: `rust/analytics-web-srv/src/main.rs`

Delete:
- `GenerateTraceRequest` struct (lines 77-81)
- `ProgressUpdate` struct (lines 90-94)
- `BinaryStartMarker` struct (lines 97-100)
- `ProgressStream` type alias (line 146)
- `generate_trace` handler (lines 598-609)
- `generate_trace_stream` function (lines 611-699)
- Route: `.route(&format!("{base_path}/api/perfetto/{{process_id}}/generate"), post(generate_trace))` (lines 301-303)

Also consider removing (unused by client):
- `get_trace_info` handler (lines 545-595)
- `TraceMetadata` struct (lines 103-108)
- `SpanCounts` struct (lines 111-115)
- Route: `.route(&format!("{base_path}/api/perfetto/{{process_id}}/info"), get(get_trace_info))` (lines 298-299)
- `query_nb_trace_events` import (line 42) — if only used by `get_trace_info`

Clean up imports that become unused:
- `perfetto_trace_client` (line 34) — if only used by `generate_trace_stream`
- `use futures::{Stream, StreamExt}` (line 21) — `Stream` only used for `ProgressStream`, `StreamExt` only in `generate_trace_stream`
- `Pin` from `std::pin::Pin` (line 44) — only used for `ProgressStream`
- `async-stream` in `Cargo.toml` (line 17) — check if `stream_query.rs` still uses it (it does, keep it)

### Step 6: Verify

- `cd analytics-web-app && yarn type-check` — no type errors
- `cd analytics-web-app && yarn test` — all tests pass
- `cd analytics-web-app && yarn lint` — clean
- `cd rust && cargo build` — compiles
- `cd rust && cargo clippy --workspace -- -D warnings` — no warnings
- `cd rust && cargo fmt` — formatted

### Reference
- `stream_query` server protocol: `rust/analytics-web-srv/src/stream_query.rs`
- `BufferedReader`: `analytics-web-app/src/lib/arrow-stream.ts` lines 53-133
- `streamQuery` client parser: `analytics-web-app/src/lib/arrow-stream.ts` lines 151-305
- Perfetto trace chunks SQL: `rust/public/src/client/perfetto_trace_client.rs`
- PerfettoExportCell: `analytics-web-app/src/lib/screen-renderers/cells/PerfettoExportCell.tsx`
- Existing tests: `analytics-web-app/src/lib/screen-renderers/cells/__tests__/PerfettoExportCell.test.tsx`
