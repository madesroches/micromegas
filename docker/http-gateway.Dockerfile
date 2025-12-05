# Multi-stage build for http-gateway-srv
FROM rust:1-bookworm AS builder

WORKDIR /build
COPY rust/ ./rust/

WORKDIR /build/rust
RUN cargo build --release --bin http-gateway-srv

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/rust/target/release/http-gateway-srv /usr/local/bin/

EXPOSE 8080
ENTRYPOINT ["http-gateway-srv"]
