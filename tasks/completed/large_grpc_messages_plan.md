# Support Large gRPC Messages

## Overview

The web app fails when a FlightSQL query response exceeds 4MB (the default Tonic/gRPC message size limit). The Rust FlightSQL client in `flightsql_client_factory.rs` creates a `Channel` and `FlightSqlServiceClient` without configuring `max_decoding_message_size`, so the default 4MB limit applies. The Grafana Go plugin has the same gap — no `grpc.MaxCallRecvMsgSize` dial option.

The server already sets `max_decoding_message_size(100 * 1024 * 1024)` for incoming messages. The fix is to set a matching limit on the Rust client.

## Current State

**Server** (`rust/flight-sql-srv/src/flight_sql_srv.rs:53-59`):
```rust
let svc = FlightServiceServer::new(FlightSqlServiceImpl::new(...))
    .max_decoding_message_size(100 * 1024 * 1024);
```
The server can receive 100MB messages from clients, but this has no effect on what *clients* can receive back.

**Rust client** (`rust/public/src/client/flightsql_client.rs:27-29`):
```rust
pub fn new(channel: Channel) -> Self {
    let inner = FlightSqlServiceClient::new(channel);
    Self { inner }
}
```
No `max_decoding_message_size` — inherits 4MB default.

**Python client**: Uses PyArrow's `flight.connect()` which appears to have a higher or no default limit, explaining why the issue isn't seen there.

## Implementation Steps

### 1. Fix the Rust FlightSQL client

In `rust/public/src/client/flightsql_client.rs`, set `max_decoding_message_size` on the `FlightSqlServiceClient`:

```rust
pub fn new(channel: Channel) -> Self {
    let inner = FlightSqlServiceClient::new(channel)
        .max_decoding_message_size(100 * 1024 * 1024);
    Self { inner }
}
```

This affects all Rust consumers: `analytics-web-srv`, `http_gateway`, perfetto server, examples.

## Files to Modify

| File | Change |
|------|--------|
| `rust/public/src/client/flightsql_client.rs` | Add `max_decoding_message_size(100 * 1024 * 1024)` to `FlightSqlServiceClient::new()` |

## LZ4 Compression for Arrow IPC Streams (Completed)

The analytics web app downloads uncompressed Arrow IPC data from the web server. A `game_metrics_per_process_per_minute` query returning ~11.7 MB of logical data transferred >125 MB on the wire because string columns are plain `Utf8` and the Arrow IPC stream used no compression. Adding LZ4 Frame compression to the IPC encoding dramatically reduces transfer sizes.

### Changes Made

| File | Change |
|------|--------|
| `rust/Cargo.toml` | Added `lz4` feature to `arrow-ipc` dependency |
| `rust/analytics-web-srv/src/stream_query.rs` | `encode_schema` and `encode_batch` accept `&IpcWriteOptions`; handler creates options with `LZ4_FRAME` compression |
| `rust/analytics-web-srv/tests/stream_query_tests.rs` | Updated all tests to use `IpcWriteOptions` with LZ4 |
| `analytics-web-app/package.json` | Added `lz4js` dependency |
| `analytics-web-app/src/lib/arrow-compression.ts` | Registers LZ4 codec with Apache Arrow's `compressionRegistry` |
| `analytics-web-app/src/lib/arrow-stream.ts` | Side-effect import of `arrow-compression` module |
| `analytics-web-app/src/types/lz4js.d.ts` | Type declarations for `lz4js` package |

## Testing Strategy

- Run `cargo test` and `cargo clippy` for the Rust changes
- Run `yarn build` in `grafana/` for the Go changes
- Run `yarn test` and `yarn build` in `analytics-web-app/` for the frontend changes
- Manual test: run a query in the web app that returns >4MB of data and verify it succeeds
- Manual test: verify network transfer size is dramatically smaller in browser dev tools for the game_metrics query
