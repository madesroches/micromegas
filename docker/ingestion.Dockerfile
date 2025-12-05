# Multi-stage build for telemetry-ingestion-srv
FROM rust:1-bookworm AS builder

WORKDIR /build
COPY rust/ ./rust/

WORKDIR /build/rust
RUN cargo build --release --bin telemetry-ingestion-srv

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/rust/target/release/telemetry-ingestion-srv /usr/local/bin/

EXPOSE 9000
ENTRYPOINT ["telemetry-ingestion-srv"]
CMD ["--listen-endpoint-http", "0.0.0.0:9000"]
