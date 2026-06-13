# Multi-stage build for micromegas-monolith
# (single-process image: ingestion + FlightSQL + maintenance + web)

# Stage 1: WASM query engine (pre-built via wasm-builder.Dockerfile)
ARG WASM_IMAGE=micromegas-wasm-builder:latest
FROM ${WASM_IMAGE} AS wasm-builder

# Stage 2: Build frontend
FROM node:20-alpine AS frontend-builder

WORKDIR /app
RUN corepack enable
COPY analytics-web-app/package.json analytics-web-app/yarn.lock analytics-web-app/.yarnrc.yml ./
# Local link: dependency must exist for yarn to create the symlink
COPY analytics-web-app/src/lib/datafusion-wasm/ ./src/lib/datafusion-wasm/
RUN yarn install --immutable

COPY analytics-web-app/ ./
COPY --from=wasm-builder /build/rust/datafusion-wasm/pkg/ ./src/lib/datafusion-wasm/
RUN yarn build

# Stage 3: Build Rust backend (monolith binary)
FROM rust:1-bookworm AS rust-builder

WORKDIR /build
COPY rust/ ./rust/

WORKDIR /build/rust
RUN cargo build --release --bin micromegas-monolith

# Stage 4: Runtime
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=rust-builder /build/rust/target/release/micromegas-monolith /usr/local/bin/
COPY --from=frontend-builder /app/dist /app/frontend

EXPOSE 9000 50051 3000

# Always-required web-role vars; override via -e for real deployments.
ENV MICROMEGAS_WEB_CORS_ORIGIN=http://localhost:3000 \
    MICROMEGAS_BASE_PATH=/

ENTRYPOINT ["micromegas-monolith"]
CMD ["--roles", "all", \
     "--listen-endpoint-http", "0.0.0.0:9000", \
     "--frontend-dir", "/app/frontend"]
