# Multi-stage build for flight-sql-srv
FROM rust:1-bookworm AS builder

WORKDIR /build
COPY rust/ ./rust/

WORKDIR /build/rust
RUN cargo build --release --bin flight-sql-srv

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/rust/target/release/flight-sql-srv /usr/local/bin/

EXPOSE 50051
ENTRYPOINT ["flight-sql-srv"]
