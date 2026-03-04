# Standalone WASM builder for DataFusion query engine
# Built as a dependency by analytics-web.Dockerfile and all-in-one.Dockerfile
#
# Manual build:
#   docker build -f docker/wasm-builder.Dockerfile -t micromegas-wasm-builder:latest .

FROM rust:1-bookworm

RUN apt-get update && \
    apt-get install -y --no-install-recommends clang binaryen && \
    rm -rf /var/lib/apt/lists/*

RUN rustup target add wasm32-unknown-unknown

WORKDIR /build/rust/datafusion-wasm

# Copy Cargo.lock first to cache wasm-bindgen-cli installation
COPY rust/datafusion-wasm/Cargo.lock ./
RUN WASM_BINDGEN_VERSION=$(grep -A1 'name = "wasm-bindgen"' Cargo.lock | grep version | head -1 | sed 's/.*"\(.*\)".*/\1/') && \
    cargo install wasm-bindgen-cli --version "$WASM_BINDGEN_VERSION"

# Copy full Rust source (datafusion-wasm has path deps on workspace crates)
COPY rust/ /build/rust/
RUN cargo build --target wasm32-unknown-unknown --release

# Generate JS bindings and optimize
RUN mkdir -p pkg && \
    wasm-bindgen target/wasm32-unknown-unknown/release/micromegas_datafusion_wasm.wasm \
        --out-dir pkg --target web && \
    wasm-opt pkg/micromegas_datafusion_wasm_bg.wasm -Os -o pkg/micromegas_datafusion_wasm_bg.wasm

# Write package.json for the WASM package (keep in sync with WASM_PACKAGE_JSON in build.py)
RUN printf '{\n  "name": "micromegas-datafusion-wasm",\n  "version": "0.1.0",\n  "type": "module",\n  "main": "micromegas_datafusion_wasm.js",\n  "types": "micromegas_datafusion_wasm.d.ts"\n}\n' > pkg/package.json
