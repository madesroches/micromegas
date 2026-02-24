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

## Testing Strategy

- Run `cargo test` and `cargo clippy` for the Rust changes
- Run `yarn build` in `grafana/` for the Go changes
- Manual test: run a query in the web app that returns >4MB of data and verify it succeeds
