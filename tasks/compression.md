# Compression Feature Handover

## Issue
GitHub issue #781: Add gzip compression to analytics-web-srv responses. A user in a corporate environment gets `net::ERR_CONNECTION_CLOSED` when generating large Perfetto traces (~255 MB). Corporate proxy kills the connection due to body size limit. Protobuf compresses ~10-15x.

## Branch
`compression`

## What Was Done

### 1. HTTP-level gzip compression (WORKING)
Added `tower-http` `CompressionLayer` with gzip to the analytics-web-srv router. This compresses all normal responses (JSON APIs, HTML, static files).

**Files changed:**
- `rust/Cargo.toml` — added `compression-gzip` feature to tower-http
- `rust/analytics-web-srv/Cargo.toml` — added `compression-gzip` feature, added `lz4` dependency
- `rust/analytics-web-srv/src/main.rs` — added `CompressionLayer::new().gzip(true)` to router, imported `CompressionLayer`

### 2. Application-level lz4 compression for Perfetto traces (BROKEN)
The `generate_trace` endpoint uses a mixed framed protocol: JSON progress lines, then `{"type":"binary_start"}\n`, then binary protobuf data. HTTP-level gzip breaks this because it changes chunk boundaries, causing the client to lose data at the JSON/binary transition.

The approach taken: compress binary chunks with lz4 at the application level inside the framed protocol. The endpoint opts out of HTTP compression via `Content-Encoding: identity`.

**Server side** (`rust/analytics-web-srv/src/main.rs`):
- `generate_trace` response has `Content-Encoding: identity` to skip the CompressionLayer
- Each protobuf chunk from FlightSQL is compressed as an independent lz4 frame
- Sent as `[4-byte big-endian length][lz4 frame]` after the `binary_start` marker
- Helper function `lz4_compress_frame()` added at bottom of file

**Client side** (`analytics-web-app/src/lib/api.ts`):
- Added `lz4js` dependency + `@types/lz4js`
- `drainFrames()` parses length-prefixed lz4 frames from a byte buffer, decompresses each immediately, discards compressed data
- `findBinaryDataOffset()` searches raw bytes for `binary_start` marker to handle the case where binary data trails the marker in the same stream chunk
- `appendToFrameBuf()` helper for buffer management

## Current Bug: Perfetto says "Cannot open this file"
The trace data reaching Perfetto is corrupted. The lz4 roundtrip (Rust lz4 crate compress → lz4js decompress) has NOT been verified with an integration test.

### Possible causes (not yet investigated):
1. **lz4 roundtrip incompatibility** — The Rust `lz4` crate (v1.23) encoder defaults may produce frames that `lz4js` (v0.2.0) can't correctly decompress. Specifically:
   - Content checksum is enabled by default (`ContentChecksum::ChecksumEnabled`)
   - Block mode is Linked by default
   - lz4js has a known bug where it reads block checksums BEFORE block data instead of after (but block checksums are disabled by default, so this shouldn't trigger)
   - `decompressBound()` may miscalculate output size for certain frame configurations

2. **Subarray handling** — `frameBuf.subarray()` is passed to `lz4.decompress()`. While this should work (it's a valid TypedArray view), it hasn't been verified that lz4js handles views correctly vs copies.

3. **Chunk boundary / framing issues** — Despite multiple fixes to handle `binary_start` + binary data in the same stream chunk, the frame parsing logic may still have edge cases. The `continue` guard was fixed but the overall flow is complex.

4. **Empty or zero-length decompression** — If `lz4.decompress()` returns empty data silently (e.g., due to missing content size in frame header), the trace would be empty.

### Recommended next steps:
1. **Write an integration test** for the lz4 roundtrip: compress known data with `lz4_compress_frame()` in Rust, save the bytes, then decompress with `lz4js` in a Jest test. This will isolate whether the issue is compression compatibility or framing.
2. **Add browser console logging** in `drainFrames()` to verify frame lengths and decompressed sizes during a real trace download.
3. **Consider the user's suggestion**: move trace generation logic to the client side. The client already has FlightSQL query capability via `stream_query`. Instead of the custom `generate_trace` endpoint with its mixed protocol, the client could query FlightSQL directly and format the Perfetto protobuf client-side. This eliminates the framed protocol entirely and lets HTTP-level gzip compression work naturally.

## Files Modified (full list)
- `rust/Cargo.toml` — workspace deps: added `compression-gzip` to tower-http features
- `rust/analytics-web-srv/Cargo.toml` — added `compression-gzip` to tower-http, added `lz4.workspace = true`
- `rust/analytics-web-srv/src/main.rs` — CompressionLayer, Content-Encoding: identity on generate_trace, lz4 per-chunk compression, `lz4_compress_frame()` helper
- `analytics-web-app/src/lib/api.ts` — lz4js import, incremental frame decompression with `drainFrames()`/`findBinaryDataOffset()`/`appendToFrameBuf()`
- `analytics-web-app/package.json` — added `lz4js`, `@types/lz4js`

## User Preferences
- Prefers lz4 over gzip for application-level compression (faster decompression)
- Wants to keep the framed JSON/binary protocol for error handling and streaming progress
- Suggested moving Perfetto trace logic entirely to the client (execute queries via FlightSQL, format protobuf client-side) which would eliminate the mixed protocol problem altogether
