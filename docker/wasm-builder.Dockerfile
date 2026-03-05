# Standalone WASM builder for DataFusion query engine
# Built as a dependency by analytics-web.Dockerfile and all-in-one.Dockerfile
#
# Manual build:
#   docker build -f docker/wasm-builder.Dockerfile -t micromegas-wasm-builder:latest .

FROM rust:1-bookworm

RUN apt-get update && \
    apt-get install -y --no-install-recommends clang curl ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Install recent binaryen from GitHub (Debian's version is too old for externref)
ARG BINARYEN_VERSION=126
ARG BINARYEN_SHA256_X86_64=e487e0eac1f02a6739816c617270b033e5d3f8ca90439301fd0286460322fd76
ARG BINARYEN_SHA256_AARCH64=4013cbcee8928abca015884e3f89d01804f6e1d9f40a4ea01dcdd0aba3e609f5
RUN ARCH=$(dpkg --print-architecture) && \
    if [ "$ARCH" = "arm64" ]; then BINARYEN_ARCH="aarch64"; EXPECTED_SHA256="$BINARYEN_SHA256_AARCH64"; \
    else BINARYEN_ARCH="x86_64"; EXPECTED_SHA256="$BINARYEN_SHA256_X86_64"; fi && \
    curl -fsSL "https://github.com/WebAssembly/binaryen/releases/download/version_${BINARYEN_VERSION}/binaryen-version_${BINARYEN_VERSION}-${BINARYEN_ARCH}-linux.tar.gz" \
         -o /tmp/binaryen.tar.gz && \
    echo "${EXPECTED_SHA256}  /tmp/binaryen.tar.gz" | sha256sum -c - && \
    tar xzf /tmp/binaryen.tar.gz -C /usr/local --strip-components=1 && \
    rm /tmp/binaryen.tar.gz

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
    wasm-opt pkg/micromegas_datafusion_wasm_bg.wasm -Os --enable-reference-types -o pkg/micromegas_datafusion_wasm_bg.wasm

# Write package.json for the WASM package (keep in sync with WASM_PACKAGE_JSON in build.py)
RUN printf '{\n  "name": "micromegas-datafusion-wasm",\n  "version": "0.1.0",\n  "type": "module",\n  "main": "micromegas_datafusion_wasm.js",\n  "types": "micromegas_datafusion_wasm.d.ts"\n}\n' > pkg/package.json
