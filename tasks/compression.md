# Response Compression for analytics-web-srv

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/781

## Problem
Corporate proxy kills connections when generating large Perfetto traces (~255 MB body size), causing `net::ERR_CONNECTION_CLOSED`.

## What's done
- `tower-http` `CompressionLayer` with gzip on the router — all endpoints compressed transparently
- Chunk boundary fix in `generateTrace`: `findBinaryDataOffset()` extracts trailing binary data when gzip merges the `binary_start` marker and binary data into one chunk
- `postMessage` console error fix during Perfetto handshake

## What's left

The `generate_trace` endpoint uses a fragile mixed protocol: JSON progress lines, then `{"type":"binary_start"}\n`, then all remaining bytes are raw binary. The client detects the transition heuristically. This required the `findBinaryDataOffset` workaround.

Two options to fix this properly:

### Option A: align protocol with stream_query

The `stream_query` endpoint uses a robust size-prefixed protocol (`{"type":"batch","size":N}\n` + N bytes) with a `BufferedReader` that handles arbitrary chunk boundaries. Apply the same pattern to `generate_trace`:

```
{"type":"progress","message":"Connecting..."}\n
{"type":"data","size":8192}\n
[8192 bytes of protobuf]
{"type":"data","size":4096}\n
[4096 bytes of protobuf]
{"type":"done"}\n
```

Errors work even mid-stream: `{"type":"error","message":"..."}\n`

**Server**: `rust/analytics-web-srv/src/main.rs` — rework `generate_trace_stream()` to yield size-prefixed data frames
**Client**: `analytics-web-app/src/lib/api.ts` — rewrite `generateTrace` to use `BufferedReader` from `arrow-stream.ts`, remove `findBinaryDataOffset`

### Option B: move trace generation to client side

The client already has FlightSQL query capability via `stream_query`. Instead of the custom `generate_trace` endpoint, the client could query `perfetto_trace_chunks()` directly via FlightSQL and assemble the protobuf client-side. This eliminates the mixed protocol entirely and the `generate_trace` endpoint can be removed.

### Reference
- `stream_query` server protocol: `rust/analytics-web-srv/src/stream_query.rs`
- `BufferedReader`: `analytics-web-app/src/lib/arrow-stream.ts` lines 53-133
- `streamQuery` client parser: `analytics-web-app/src/lib/arrow-stream.ts` lines 151-305
- Perfetto trace chunks: `rust/public/src/client/perfetto_trace_client.rs`
