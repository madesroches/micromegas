# Multi-stage build for telemetry-ingestion-srv
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS builder

ARG TARGETARCH

RUN if [ "$TARGETARCH" = "arm64" ]; then \
      apt-get update && \
      apt-get install -y --no-install-recommends \
        g++-aarch64-linux-gnu libc6-dev-arm64-cross && \
      rm -rf /var/lib/apt/lists/*; \
    fi

RUN if [ "$TARGETARCH" = "arm64" ]; then \
      rustup target add aarch64-unknown-linux-gnu; \
    fi

WORKDIR /build
COPY rust/ ./rust/

WORKDIR /build/rust
RUN if [ "$TARGETARCH" = "arm64" ]; then \
      CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
      cargo build --release --target aarch64-unknown-linux-gnu --bin telemetry-ingestion-srv; \
    else \
      cargo build --release --bin telemetry-ingestion-srv; \
    fi

RUN if [ "$TARGETARCH" = "arm64" ]; then ARCH_PATH="aarch64-unknown-linux-gnu/"; else ARCH_PATH=""; fi && \
    cp /build/rust/target/${ARCH_PATH}release/telemetry-ingestion-srv /build/telemetry-ingestion-srv

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/telemetry-ingestion-srv /usr/local/bin/

EXPOSE 9000
ENTRYPOINT ["telemetry-ingestion-srv"]
CMD ["--listen-endpoint-http", "0.0.0.0:9000"]
