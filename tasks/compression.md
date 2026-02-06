# Response Compression for analytics-web-srv

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/781

## Problem
Corporate proxy kills connections when generating large Perfetto traces (~255 MB body size), causing `net::ERR_CONNECTION_CLOSED`.

## Solution

Added `tower-http` `CompressionLayer` with gzip to the router. All endpoints are compressed transparently, including the `generate_trace` mixed protocol (JSON progress lines then binary protobuf) and `query-stream` (JSON-framed Arrow IPC).

The browser handles gzip decompression transparently for `fetch()` responses, so the client code receives the original bytes regardless of compression.

### Chunk boundary fix
With gzip, the browser may merge the `binary_start` JSON marker and the first binary data into a single decompressed chunk. The original client code discarded this trailing binary data. Fix: `findBinaryDataOffset()` scans the raw bytes and extracts any binary data following the marker in the same chunk.

## Files changed
- `rust/Cargo.toml` — added `compression-gzip` feature to tower-http workspace dep
- `rust/analytics-web-srv/Cargo.toml` — added `compression-gzip` feature
- `rust/analytics-web-srv/src/main.rs` — added `CompressionLayer::new().gzip(true)` to router
- `analytics-web-app/src/lib/api.ts` — added `findBinaryDataOffset()` to handle merged chunks
- `analytics-web-app/src/lib/perfetto.ts` — use `'*'` target origin for PING to suppress console errors

## Future consideration
Move Perfetto trace generation to the client side. The client already has FlightSQL query capability via `stream_query`. Querying FlightSQL directly and formatting protobuf client-side would eliminate the mixed protocol entirely.
