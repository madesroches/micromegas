# Multi-stage build for analytics-web-srv (includes frontend + WASM engine)

# Stage 1: Build WASM query engine
FROM rust:1-bookworm AS wasm-builder

RUN rustup target add wasm32-unknown-unknown && \
    apt-get update && apt-get install -y --no-install-recommends binaryen && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build/rust/datafusion-wasm

# Copy Cargo.lock first to cache wasm-bindgen-cli installation
COPY rust/datafusion-wasm/Cargo.lock ./
RUN WASM_BINDGEN_VERSION=$(grep -A1 'name = "wasm-bindgen"' Cargo.lock | grep version | head -1 | sed 's/.*"\(.*\)".*/\1/') && \
    cargo install wasm-bindgen-cli --version "$WASM_BINDGEN_VERSION"

# Copy source and build
COPY rust/datafusion-wasm/ ./
RUN cargo build --target wasm32-unknown-unknown --release

# Generate JS bindings and optimize
RUN mkdir -p pkg && \
    wasm-bindgen target/wasm32-unknown-unknown/release/datafusion_wasm.wasm \
        --out-dir pkg --target web && \
    wasm-opt pkg/datafusion_wasm_bg.wasm -Os -o pkg/datafusion_wasm_bg.wasm

# Write package.json for the WASM package
RUN printf '{\n  "name": "datafusion-wasm",\n  "version": "0.1.0",\n  "type": "module",\n  "main": "datafusion_wasm.js",\n  "types": "datafusion_wasm.d.ts"\n}\n' > pkg/package.json

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
