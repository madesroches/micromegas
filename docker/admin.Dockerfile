# Multi-stage build for telemetry-admin
FROM rust:1-bookworm AS builder

WORKDIR /build
COPY rust/ ./rust/

WORKDIR /build/rust
RUN cargo build --release --bin telemetry-admin

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/rust/target/release/telemetry-admin /usr/local/bin/

ENTRYPOINT ["telemetry-admin"]
