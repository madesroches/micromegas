# Multi-stage build for analytics-web-srv (includes frontend + WASM engine)

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

# Stage 3: Build Rust backend
FROM rust:1-bookworm AS rust-builder

WORKDIR /build
COPY rust/ ./rust/

WORKDIR /build/rust
RUN cargo build --release --bin analytics-web-srv

# Stage 4: Runtime
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=rust-builder /build/rust/target/release/analytics-web-srv /usr/local/bin/
COPY --from=frontend-builder /app/dist /app/frontend

EXPOSE 3000
ENTRYPOINT ["analytics-web-srv"]
CMD ["--port", "3000", "--frontend-dir", "/app/frontend"]
