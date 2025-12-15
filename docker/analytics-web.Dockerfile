# Multi-stage build for analytics-web-srv (includes frontend)

# Stage 1: Build frontend
FROM node:20-alpine AS frontend-builder

WORKDIR /app
COPY analytics-web-app/package.json analytics-web-app/yarn.lock ./
RUN yarn install --frozen-lockfile

COPY analytics-web-app/ ./
RUN yarn build

# Stage 2: Build Rust backend
FROM rust:1-bookworm AS rust-builder

WORKDIR /build
COPY rust/ ./rust/

WORKDIR /build/rust
RUN cargo build --release --bin analytics-web-srv

# Stage 3: Runtime
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=rust-builder /build/rust/target/release/analytics-web-srv /usr/local/bin/
COPY --from=frontend-builder /app/dist /app/frontend

EXPOSE 3000
ENTRYPOINT ["analytics-web-srv"]
CMD ["--port", "3000", "--frontend-dir", "/app/frontend"]
