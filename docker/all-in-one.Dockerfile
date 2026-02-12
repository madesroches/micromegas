# All-in-one image containing all micromegas services
# Useful for dev/test deployments on a single machine

# Stage 1: WASM query engine (pre-built via wasm-builder.Dockerfile)
ARG WASM_IMAGE=micromegas-wasm-builder:latest
FROM ${WASM_IMAGE} AS wasm-builder

# Stage 2: Build frontend
FROM node:20-alpine AS frontend-builder

WORKDIR /app
COPY analytics-web-app/package.json analytics-web-app/yarn.lock ./
# Local file: dependency must exist for yarn to resolve it
COPY analytics-web-app/src/lib/datafusion-wasm/ ./src/lib/datafusion-wasm/
RUN yarn install --frozen-lockfile

COPY analytics-web-app/ ./
COPY --from=wasm-builder /build/rust/datafusion-wasm/pkg/ ./src/lib/datafusion-wasm/
RUN yarn build

# Stage 3: Build all Rust binaries
FROM rust:1-bookworm AS rust-builder

WORKDIR /build
COPY rust/ ./rust/

WORKDIR /build/rust
RUN cargo build --release \
    --bin telemetry-ingestion-srv \
    --bin flight-sql-srv \
    --bin telemetry-admin \
    --bin http-gateway-srv \
    --bin analytics-web-srv

# Stage 4: Runtime with all services
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Copy all binaries
COPY --from=rust-builder /build/rust/target/release/telemetry-ingestion-srv /usr/local/bin/
COPY --from=rust-builder /build/rust/target/release/flight-sql-srv /usr/local/bin/
COPY --from=rust-builder /build/rust/target/release/telemetry-admin /usr/local/bin/
COPY --from=rust-builder /build/rust/target/release/http-gateway-srv /usr/local/bin/
COPY --from=rust-builder /build/rust/target/release/analytics-web-srv /usr/local/bin/

# Copy frontend for analytics-web-srv
COPY --from=frontend-builder /app/dist /app/frontend

# No default entrypoint - specify service when running:
#   docker run micromegas-all telemetry-ingestion-srv --listen-endpoint-http 0.0.0.0:9000
#   docker run micromegas-all flight-sql-srv
#   docker run micromegas-all analytics-web-srv --frontend-dir /app/frontend

EXPOSE 9000 50051 3000 8080
